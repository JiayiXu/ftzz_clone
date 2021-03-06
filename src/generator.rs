use std::{cmp::max, fs::create_dir_all, num::NonZeroUsize, path::PathBuf, thread};

use anyhow::{anyhow, Context};
use cli_errors::{CliExitAnyhowWrapper, CliResult};
use derive_builder::Builder;
use num_format::{Locale, ToFormattedString};
use rand::SeedableRng;
use rand_distr::Normal;
use rand_xoshiro::Xoshiro256PlusPlus;

use tracing::{event, Level};

use crate::core::{
    run, FilesAndContentsGenerator, FilesNoContentsGenerator, GeneratorStats,
    OtherFilesAndContentsGenerator,
};

#[derive(Builder, Debug)]
#[builder(build_fn(validate = "Self::validate"))]
pub struct Generator {
    root_dir: PathBuf,
    num_files: NonZeroUsize,
    #[builder(default = "false")]
    files_exact: bool,
    #[builder(default = "0")]
    num_bytes: usize,
    #[builder(default = "false")]
    bytes_exact: bool,
    #[builder(default = "5")]
    max_depth: u32,
    #[builder(default = "self.default_ftd_ratio()")]
    file_to_dir_ratio: NonZeroUsize,
    #[builder(default = "0")]
    seed: u64,
}

impl GeneratorBuilder {
    fn validate(&self) -> Result<(), String> {
        if let Some(ratio) = self.file_to_dir_ratio && let Some(num_files) = self.num_files && ratio > num_files {
            return Err(format!(
                "The file to dir ratio ({}) cannot be larger than the number of files to generate ({}).",
                ratio,
                num_files,
            ));
        }

        Ok(())
    }

    fn default_ftd_ratio(&self) -> NonZeroUsize {
        let r = max(self.num_files.unwrap().get() / 1000, 1);
        unsafe { NonZeroUsize::new_unchecked(r) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_params_succeeds() {
        let g = GeneratorBuilder::default()
            .root_dir(PathBuf::from("abc"))
            .num_files(NonZeroUsize::new(1).unwrap())
            .build()
            .unwrap();

        assert_eq!(g.root_dir, PathBuf::from("abc"));
        assert_eq!(g.num_files.get(), 1);
        assert!(!g.files_exact);
        assert_eq!(g.num_bytes, 0);
        assert!(!g.bytes_exact);
        assert_eq!(g.max_depth, 5);
        assert_eq!(g.file_to_dir_ratio.get(), 1);
        assert_eq!(g.seed, 0);
    }

    #[test]
    fn ratio_greater_than_num_files_fails() {
        let g = GeneratorBuilder::default()
            .root_dir(PathBuf::from("abc"))
            .num_files(NonZeroUsize::new(1).unwrap())
            .file_to_dir_ratio(NonZeroUsize::new(2).unwrap())
            .build();

        assert!(g.is_err());
    }
}

impl Generator {
    pub fn generate(self) -> CliResult<()> {
        let options = validated_options(self)?;
        print_configuration_info(&options);
        print_stats(run_generator(options)?);
        Ok(())
    }
}

#[derive(Debug)]
struct Configuration {
    root_dir: PathBuf,
    files: usize,
    bytes: usize,
    files_exact: bool,
    bytes_exact: bool,
    files_per_dir: f64,
    dirs_per_dir: f64,
    bytes_per_file: f64,
    max_depth: u32,
    seed: u64,

    informational_dirs_per_dir: usize,
    informational_total_dirs: usize,
    informational_bytes_per_files: usize,
}

fn validated_options(generator: Generator) -> CliResult<Configuration> {
    create_dir_all(&generator.root_dir)
        .with_context(|| format!("Failed to create directory {:?}", generator.root_dir))
        .with_code(exitcode::IOERR)?;
    if generator
        .root_dir
        .read_dir()
        .with_context(|| format!("Failed to read directory {:?}", generator.root_dir))
        .with_code(exitcode::IOERR)?
        .count()
        != 0
    {
        return Err(anyhow!(format!(
            "The root directory {:?} must be empty.",
            generator.root_dir,
        )))
        .with_code(exitcode::DATAERR);
    }

    let num_files = generator.num_files.get() as f64;
    let bytes_per_file = generator.num_bytes as f64 / num_files;

    if generator.max_depth == 0 {
        return Ok(Configuration {
            root_dir: generator.root_dir,
            files: generator.num_files.get(),
            bytes: generator.num_bytes,
            files_exact: generator.files_exact,
            bytes_exact: generator.bytes_exact,
            files_per_dir: num_files,
            dirs_per_dir: 0.,
            bytes_per_file,
            max_depth: 0,
            seed: generator.seed,

            informational_dirs_per_dir: 0,
            informational_total_dirs: 1,
            informational_bytes_per_files: bytes_per_file.round() as usize,
        });
    }

    let ratio = generator.file_to_dir_ratio.get() as f64;
    let num_dirs = num_files / ratio;
    // This formula was derived from the following equation:
    // num_dirs = unknown_num_dirs_per_dir^max_depth
    let dirs_per_dir = num_dirs.powf(1f64 / generator.max_depth as f64);

    Ok(Configuration {
        root_dir: generator.root_dir,
        files: generator.num_files.get(),
        bytes: generator.num_bytes,
        files_exact: generator.files_exact,
        bytes_exact: generator.bytes_exact,
        files_per_dir: ratio,
        bytes_per_file,
        dirs_per_dir,
        max_depth: generator.max_depth,
        seed: generator.seed,

        informational_dirs_per_dir: dirs_per_dir.round() as usize,
        informational_total_dirs: num_dirs.round() as usize,
        informational_bytes_per_files: bytes_per_file.round() as usize,
    })
}

fn print_configuration_info(config: &Configuration) {
    let locale = Locale::en;
    println!(
        "{file_count_type} {} {files_maybe_plural} will be generated in approximately \
        {} {directories_maybe_plural} distributed across a tree of maximum depth {} where each \
        directory contains approximately {} other {dpd_directories_maybe_plural}.\
        {bytes_info}",
        config.files.to_formatted_string(&locale),
        config.informational_total_dirs.to_formatted_string(&locale),
        config.max_depth.to_formatted_string(&locale),
        config
            .informational_dirs_per_dir
            .to_formatted_string(&locale),
        file_count_type = if config.files_exact {
            "Exactly"
        } else {
            "About"
        },
        files_maybe_plural = if config.files == 1 { "file" } else { "files" },
        directories_maybe_plural = if config.informational_total_dirs == 1 {
            "directory"
        } else {
            "directories"
        },
        dpd_directories_maybe_plural = if config.informational_dirs_per_dir == 1 {
            "directory"
        } else {
            "directories"
        },
        bytes_info = if config.bytes > 0 {
            format!(
                " Each file will contain {byte_count_type} {} {bytes_maybe_plural} of random data.",
                config
                    .informational_bytes_per_files
                    .to_formatted_string(&locale),
                byte_count_type = if config.bytes_exact {
                    "exactly"
                } else {
                    "approximately"
                },
                bytes_maybe_plural = if config.informational_bytes_per_files == 1 {
                    "byte"
                } else {
                    "bytes"
                },
            )
        } else {
            "".to_string()
        },
    );
}

fn print_stats(stats: GeneratorStats) {
    let locale = Locale::en;
    println!(
        "Created {} {files_maybe_plural}{bytes_info} across {} {directories_maybe_plural}.",
        stats.files.to_formatted_string(&locale),
        stats.dirs.to_formatted_string(&locale),
        files_maybe_plural = if stats.files == 1 { "file" } else { "files" },
        directories_maybe_plural = if stats.dirs == 1 {
            "directory"
        } else {
            "directories"
        },
        bytes_info = if stats.bytes > 0 {
            event!(Level::INFO, bytes = stats.bytes, "Exact bytes written");
            format!(" ({})", bytesize::to_string(stats.bytes as u64, false))
        } else {
            "".to_string()
        }
    );
}

fn run_generator(config: Configuration) -> CliResult<GeneratorStats> {
    let parallelism =
        thread::available_parallelism().unwrap_or(unsafe { NonZeroUsize::new_unchecked(1) });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(parallelism.get())
        .build()
        .context("Failed to create tokio runtime")
        .with_code(exitcode::OSERR)?;

    event!(Level::INFO, config = ?config, "Starting config");
    runtime.block_on(run_generator_async(config, parallelism))
}

async fn run_generator_async(
    config: Configuration,
    parallelism: NonZeroUsize,
) -> CliResult<GeneratorStats> {
    let max_depth = config.max_depth as usize;
    let random = {
        let seed = ((config.files.wrapping_add(max_depth) as f64
            * (config.files_per_dir + config.dirs_per_dir)) as u64)
            .wrapping_add(config.seed);
        event!(Level::DEBUG, seed = ?seed, "Starting seed");
        Xoshiro256PlusPlus::seed_from_u64(seed)
    };
    let num_files_distr = Normal::new(config.files_per_dir, config.files_per_dir * 0.2).unwrap();
    let num_dirs_distr = Normal::new(config.dirs_per_dir, config.dirs_per_dir * 0.2).unwrap();
    let num_bytes_distr = Normal::new(config.bytes_per_file, config.bytes_per_file * 0.2).unwrap();

    macro_rules! run {
        ($generator:expr) => {{
            run(config.root_dir, max_depth, parallelism, $generator).await
        }};
    }

    if config.files_exact || config.bytes_exact {
        run!(OtherFilesAndContentsGenerator::new(
            num_files_distr,
            num_dirs_distr,
            if config.bytes > 0 {
                Some(num_bytes_distr)
            } else {
                None
            },
            random,
            if config.files_exact {
                Some(unsafe { NonZeroUsize::new_unchecked(config.files) })
            } else {
                None
            },
            if config.bytes_exact {
                Some(config.bytes)
            } else {
                None
            },
        ))
    } else if config.bytes > 0 {
        run!(FilesAndContentsGenerator {
            num_files_distr,
            num_dirs_distr,
            num_bytes_distr,
            random,
        })
    } else {
        run!(FilesNoContentsGenerator {
            num_files_distr,
            num_dirs_distr,
            random,
        })
    }
}

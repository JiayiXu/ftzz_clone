use std::{
    cmp::max,
    fs::{create_dir, create_dir_all, File},
    ops::AddAssign,
    path::PathBuf,
};

use anyhow::{anyhow, Context};
use clap_num::si_number;
use derive_new::new;
use futures::{stream::FuturesUnordered, StreamExt};
use log::{debug, info};
use num_format::{SystemLocale, ToFormattedString};
use rand::{distributions::Distribution, RngCore, SeedableRng};
use rand_distr::{LogNormal, Normal};
use rand_xorshift::XorShiftRng;
use structopt::StructOpt;
use tokio::{runtime::Builder, task, task::JoinHandle};

use crate::errors::{CliExitAnyhowWrapper, CliResult};

#[derive(Debug, StructOpt, new)]
pub struct Generate {
    /// The directory in which to generate files (will be created if it does not exist)
    root_dir: PathBuf,

    /// The number of files to generate (this value is probabilistically respected, meaning any
    /// number of files may be generated so long as we attempt to get close to N)
    #[structopt(short = "n", long = "files", parse(try_from_str = num_files_parser))]
    num_files: usize,

    /// The maximum directory tree depth
    #[structopt(short = "d", long = "depth", default_value = "5")]
    max_depth: u32,

    /// The number of files to generate per directory (this value is probabilistically respected,
    /// meaning not all directories will have N files) (default: files / 1000)
    #[structopt(short = "r", long = "ftd_ratio", parse(try_from_str = file_to_dir_ratio_parser))]
    file_to_dir_ratio: Option<usize>,

    /// Add some additional entropy to the starting seed of our PRNG
    #[structopt(long = "entropy", default_value = "0")]
    entropy: u64,
}

fn num_files_parser(s: &str) -> Result<usize, String> {
    let files = lenient_si_number(s)?;
    if files > 0 {
        Ok(files)
    } else {
        Err(String::from("At least one file must be generated."))
    }
}

fn file_to_dir_ratio_parser(s: &str) -> Result<usize, String> {
    let ratio = lenient_si_number(s)?;
    if ratio > 0 {
        Ok(ratio)
    } else {
        Err(String::from("Cannot have no files per directory."))
    }
}

fn lenient_si_number(s: &str) -> Result<usize, String> {
    let mut s = s.replace("K", "k");
    s.remove_matches(",");
    si_number(&s)
}

pub fn generate(options: Generate) -> CliResult<()> {
    let options = validated_options(options)?;
    print_configuration_info(&options);
    print_stats(run_generator(options)?);
    Ok(())
}

#[derive(Debug)]
struct Configuration {
    root_dir: PathBuf,
    files: usize,
    files_per_dir: f64,
    dirs_per_dir: f64,
    max_depth: u32,
    entropy: u64,

    informational_dirs_per_dir: usize,
    informational_total_dirs: usize,
}

#[derive(Debug)]
struct GeneratorStats {
    files: usize,
    dirs: usize,
}

impl AddAssign for GeneratorStats {
    fn add_assign(&mut self, rhs: Self) {
        self.files += rhs.files;
        self.dirs += rhs.dirs;
    }
}

fn validated_options(options: Generate) -> CliResult<Configuration> {
    create_dir_all(&options.root_dir)
        .with_context(|| format!("Failed to create directory {:?}", options.root_dir))
        .with_code(exitcode::IOERR)?;
    if options
        .root_dir
        .read_dir()
        .with_context(|| format!("Failed to read directory {:?}", options.root_dir))
        .with_code(exitcode::IOERR)?
        .count()
        != 0
    {
        return Err(anyhow!(format!(
            "The root directory {:?} must be empty.",
            options.root_dir,
        )))
            .with_code(exitcode::DATAERR);
    }

    if options.max_depth == 0 {
        return Ok(Configuration {
            root_dir: options.root_dir,
            files: options.num_files,
            files_per_dir: options.num_files as f64,
            dirs_per_dir: 0.,
            max_depth: 0,
            entropy: options.entropy,

            informational_dirs_per_dir: 0,
            informational_total_dirs: 1,
        });
    }

    let ratio = options
        .file_to_dir_ratio
        .unwrap_or_else(|| max(options.num_files / 1000, 1));
    if ratio > options.num_files {
        return Err(anyhow!(format!(
            "The file to dir ratio ({}) cannot be larger than the number of files to generate ({}).",
            ratio,
            options.num_files,
        ))).with_code(exitcode::DATAERR);
    }

    let num_dirs = options.num_files as f64 / ratio as f64;
    // This formula was derived from the following equation:
    // num_dirs = unknown_num_dirs_per_dir^max_depth
    let dirs_per_dir = 2f64.powf(num_dirs.log2() / options.max_depth as f64);

    Ok(Configuration {
        root_dir: options.root_dir,
        files: options.num_files,
        files_per_dir: ratio as f64,
        dirs_per_dir,
        max_depth: options.max_depth,
        entropy: options.entropy,

        informational_dirs_per_dir: dirs_per_dir.round() as usize,
        informational_total_dirs: num_dirs.round() as usize,
    })
}

fn print_configuration_info(config: &Configuration) {
    let locale = SystemLocale::new().unwrap();
    println!(
        "About {} {files_maybe_plural} will be generated in approximately \
        {} {directories_maybe_plural} distributed across a tree of maximum depth {} where each \
        directory contains approximately {} other {dpd_directories_maybe_plural}.",
        config.files.to_formatted_string(&locale),
        config.informational_total_dirs.to_formatted_string(&locale),
        config.max_depth.to_formatted_string(&locale),
        config
            .informational_dirs_per_dir
            .to_formatted_string(&locale),
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
    );
}

fn print_stats(stats: GeneratorStats) {
    let locale = SystemLocale::new().unwrap();
    println!(
        "Created {} {files_maybe_plural} across {} {directories_maybe_plural}.",
        stats.files.to_formatted_string(&locale),
        stats.dirs.to_formatted_string(&locale),
        files_maybe_plural = if stats.files == 1 { "file" } else { "files" },
        directories_maybe_plural = if stats.dirs == 1 {
            "directory"
        } else {
            "directories"
        },
    );
}

#[derive(Debug)]
struct GeneratorState {
    files_per_dir: f64,
    dirs_per_dir: f64,
    max_depth: u32,

    root_dir: PathBuf,
    seed: <XorShiftRng as SeedableRng>::Seed,
}

impl GeneratorState {
    fn next(&self, root_dir: PathBuf, random: &mut XorShiftRng) -> GeneratorState {
        GeneratorState {
            root_dir,
            seed: random.next_seed(),
            max_depth: self.max_depth - 1,
            ..*self
        }
    }
}

impl From<Configuration> for GeneratorState {
    fn from(config: Configuration) -> Self {
        GeneratorState {
            files_per_dir: config.files_per_dir,
            dirs_per_dir: config.dirs_per_dir,
            max_depth: config.max_depth,

            root_dir: config.root_dir,
            seed: XorShiftRng::seed_from_u64(
                ((config.files.wrapping_add(config.max_depth as usize) as f64
                    * (config.files_per_dir + config.dirs_per_dir)) as u64)
                    .wrapping_add(config.entropy),
            )
                .next_seed(),
        }
    }
}

fn run_generator(config: Configuration) -> CliResult<GeneratorStats> {
    let runtime = Builder::new_current_thread()
        .build()
        .with_context(|| "Failed to create tokio runtime")
        .with_code(exitcode::OSERR)?;

    let state = config.into();
    info!("Starting state: {:?}", state);
    runtime.block_on(run_generator_async(state))
}

async fn run_generator_async(state: GeneratorState) -> CliResult<GeneratorStats> {
    let mut random = XorShiftRng::from_seed(state.seed);
    let num_files_to_generate = state.files_per_dir.num_to_generate(&mut random);
    let num_dirs_to_generate = if state.max_depth == 0 {
        0
    } else {
        state.dirs_per_dir.num_to_generate(&mut random)
    };

    debug!(
        "Creating {} files and {} directories in {:?}",
        num_files_to_generate, num_dirs_to_generate, state.root_dir
    );

    let tasks = task::spawn_blocking(move || -> CliResult<_> {
        let mut dir_tasks = Vec::with_capacity(num_dirs_to_generate);

        for i in 0..num_dirs_to_generate {
            let dir = state.root_dir.join(format!("{}.dir", i));

            create_dir(&dir)
                .with_context(|| format!("Failed to create directory {:?}", dir))
                .with_code(exitcode::IOERR)?;
            dir_tasks.push(spawn_run_generator_async(state.next(dir, &mut random)))
        }

        let mut file = state.root_dir;
        for i in 0..num_files_to_generate {
            file.push(i.to_string());
            File::create(&file)
                .with_context(|| format!("Failed to create file {:?}", file))
                .with_code(exitcode::IOERR)?;
            file.pop();
        }

        Ok(dir_tasks)
    })
        .await
        .with_context(|| "Failed to retrieve task result")
        .with_code(exitcode::SOFTWARE)??;

    let mut stats = GeneratorStats {
        files: num_files_to_generate,
        dirs: num_dirs_to_generate,
    };

    // We want to poll every future continuously instead of going one-by-one because each future
    // recursively spawns more I/O bound children that wouldn't otherwise get a head start.
    let mut tasks = tasks.into_iter().collect::<FuturesUnordered<_>>();
    while let Some(task) = tasks.next().await {
        stats += task
            .with_context(|| "Failed to retrieve task result")
            .with_code(exitcode::SOFTWARE)??;
    }

    Ok(stats)
}

fn spawn_run_generator_async(state: GeneratorState) -> JoinHandle<CliResult<GeneratorStats>> {
    task::spawn(run_generator_async(state))
}

trait GeneratorUtils {
    fn num_to_generate(self, random: &mut impl RngCore) -> usize;
}

impl GeneratorUtils for f64 {
    fn num_to_generate(self, random: &mut impl RngCore) -> usize {
        let sample = if self > 10_000. {
            LogNormal::from_mean_cv(self, 2.).unwrap().sample(random)
        } else {
            // LogNormal doesn't perform well with values under 10K for our purposes,
            // so default to normal
            Normal::new(self, self * 0.2).unwrap().sample(random)
        };

        sample.round() as usize
    }
}

trait RandomUtils {
    type Seed;

    fn next_seed(&mut self) -> Self::Seed;
}

impl RandomUtils for XorShiftRng {
    type Seed = <XorShiftRng as SeedableRng>::Seed;

    fn next_seed(&mut self) -> Self::Seed {
        let seed = [
            self.next_u32(),
            self.next_u32(),
            self.next_u32(),
            self.next_u32(),
        ]
            .as_ptr() as *const [u8; 16];
        unsafe { *seed }
    }
}

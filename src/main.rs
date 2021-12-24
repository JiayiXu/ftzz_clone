#![feature(string_remove_matches)]

use std::{path::PathBuf, process::exit};

use anyhow::Context;
use clap::{AppSettings, Args, Parser, Subcommand, ValueHint};
use clap_num::si_number;

use ftzz::{
    errors::{CliExitAnyhowWrapper, CliResult},
    generator::GeneratorBuilder,
};

/// A random file and directory generator
#[derive(Parser, Debug)]
#[clap(version, author = "Alex Saveau (@SUPERCILEX)")]
#[clap(global_setting(AppSettings::InferSubcommands))]
#[clap(global_setting(AppSettings::UseLongFormatForHelpSubcommand))]
#[cfg_attr(test, clap(global_setting(AppSettings::HelpExpected)))]
struct Ftzz {
    // #[clap(flatten)]
    // verbose: Verbosity,
    #[clap(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Generate a random directory hierarchy with some number of files
    ///
    /// A pseudo-random directory hierarchy will be generated (seeded by this command's input
    /// parameters) containing approximately the target number of files. The exact configuration of
    /// files and directories in the hierarchy is probabilistically determined to mostly match the
    /// specified parameters.
    Generate(Generate),
}

#[derive(Args, Debug)]
struct Generate {
    /// The directory in which to generate files
    ///
    /// The directory will be created if it does not exist.
    #[clap(value_hint = ValueHint::DirPath)]
    root_dir: PathBuf,

    /// The number of files to generate
    ///
    /// Note: this value is probabilistically respected, meaning any number of files may be
    /// generated so long as we attempt to get close to N.
    #[clap(short = 'n', long = "files", parse(try_from_str = num_files_parser))]
    num_files: usize,

    /// The maximum directory tree depth
    #[clap(short = 'd', long = "depth", default_value = "5")]
    max_depth: u32,

    /// The number of files to generate per directory (default: files / 1000)
    ///
    /// Note: this value is probabilistically respected, meaning not all directories will have N
    /// files).
    #[clap(short = 'r', long = "ftd-ratio", parse(try_from_str = file_to_dir_ratio_parser))]
    file_to_dir_ratio: Option<usize>,

    /// Add some additional entropy to the PRNG's starting seed
    ///
    /// For example, you can use bash's `$RANDOM` function.
    #[clap(long = "entropy", default_value = "0")]
    entropy: u64,
}

fn main() {
    if let Err(e) = wrapped_main() {
        if let Some(source) = e.source {
            eprintln!("{:?}", source);
        }
        exit(e.code);
    }
}

fn wrapped_main() -> CliResult<()> {
    let args = Ftzz::parse();
    // TODO waiting on https://github.com/rust-cli/clap-verbosity-flag/issues/29
    // SimpleLogger::new()
    //     .with_level(args.verbose.log_level().unwrap().to_level_filter())
    //     .init()
    //     .unwrap();

    match args.cmd {
        Cmd::Generate(options) => {
            let mut builder = GeneratorBuilder::default();
            builder
                .root_dir(options.root_dir)
                .num_files(options.num_files)
                .max_depth(options.max_depth);
            if let Some(ratio) = options.file_to_dir_ratio {
                builder.file_to_dir_ratio(ratio);
            }
            builder
                .entropy(options.entropy)
                .build()
                .context("Input validation failed")
                .with_code(exitcode::DATAERR)?
                .generate()
        }
    }
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
    let mut s = s.replace('K', "k");
    s.remove_matches(",");
    s.remove_matches("_");
    si_number(&s)
}

#[cfg(test)]
mod cli_tests {
    use clap::{
        ErrorKind::{
            DisplayHelpOnMissingArgumentOrSubcommand, MissingRequiredArgument, UnknownArgument,
        },
        FromArgMatches, IntoApp,
    };

    use super::*;

    #[test]
    fn verify_app() {
        Ftzz::into_app().debug_assert();
    }

    #[test]
    fn empty_args_displays_help() {
        let f = Ftzz::try_parse_from(Vec::<String>::new());

        assert!(f.is_err());
        assert_eq!(
            f.unwrap_err().kind,
            DisplayHelpOnMissingArgumentOrSubcommand
        );
    }

    #[test]
    fn generate_empty_args_displays_error() {
        let f = Ftzz::try_parse_from(vec!["ftzz", "generate"]);

        assert!(f.is_err());
        assert_eq!(f.unwrap_err().kind, MissingRequiredArgument);
    }

    #[test]
    fn generate_minimal_use_case_uses_defaults() {
        let m = Ftzz::into_app().get_matches_from(vec!["ftzz", "generate", "-n", "1", "dir"]);
        let g = <Generate as FromArgMatches>::from_arg_matches(
            m.subcommand_matches("generate").unwrap(),
        )
        .unwrap();

        assert_eq!(g.root_dir, PathBuf::from("dir"));
        assert_eq!(g.num_files, 1);
        assert_eq!(g.max_depth, 5);
        assert_eq!(g.file_to_dir_ratio, None);
        assert_eq!(g.entropy, 0);
    }

    #[test]
    fn generate_num_files_rejects_negatives() {
        let f = Ftzz::try_parse_from(vec!["ftzz", "generate", "-n", "-1", "dir"]);

        assert!(f.is_err());
        assert_eq!(f.unwrap_err().kind, UnknownArgument);
    }

    #[test]
    fn generate_num_files_accepts_plain_nums() {
        let m =
            Ftzz::into_app().get_matches_from(vec!["ftzz", "generate", "--files", "1000", "dir"]);
        let g = <Generate as FromArgMatches>::from_arg_matches(
            m.subcommand_matches("generate").unwrap(),
        )
        .unwrap();

        assert_eq!(g.num_files, 1000);
    }

    #[test]
    fn generate_short_num_files_accepts_plain_nums() {
        let m = Ftzz::into_app().get_matches_from(vec!["ftzz", "generate", "-n", "1000", "dir"]);
        let g = <Generate as FromArgMatches>::from_arg_matches(
            m.subcommand_matches("generate").unwrap(),
        )
        .unwrap();

        assert_eq!(g.num_files, 1000);
    }

    #[test]
    fn generate_num_files_accepts_si_numbers() {
        let m = Ftzz::into_app().get_matches_from(vec!["ftzz", "generate", "--files", "1K", "dir"]);
        let g = <Generate as FromArgMatches>::from_arg_matches(
            m.subcommand_matches("generate").unwrap(),
        )
        .unwrap();

        assert_eq!(g.num_files, 1000);
    }

    #[test]
    fn generate_num_files_accepts_commas() {
        let m =
            Ftzz::into_app().get_matches_from(vec!["ftzz", "generate", "--files", "1,000", "dir"]);
        let g = <Generate as FromArgMatches>::from_arg_matches(
            m.subcommand_matches("generate").unwrap(),
        )
        .unwrap();

        assert_eq!(g.num_files, 1000);
    }

    #[test]
    fn generate_num_files_accepts_underscores() {
        let m =
            Ftzz::into_app().get_matches_from(vec!["ftzz", "generate", "--files", "1_000", "dir"]);
        let g = <Generate as FromArgMatches>::from_arg_matches(
            m.subcommand_matches("generate").unwrap(),
        )
        .unwrap();

        assert_eq!(g.num_files, 1000);
    }

    #[test]
    fn generate_max_depth_rejects_negatives() {
        let f = Ftzz::try_parse_from(vec!["ftzz", "generate", "--depth", "-1", "-n", "1", "dir"]);

        assert!(f.is_err());
        assert_eq!(f.unwrap_err().kind, UnknownArgument);
    }

    #[test]
    fn generate_max_depth_accepts_plain_nums() {
        let m = Ftzz::into_app()
            .get_matches_from(vec!["ftzz", "generate", "--depth", "123", "-n", "1", "dir"]);
        let g = <Generate as FromArgMatches>::from_arg_matches(
            m.subcommand_matches("generate").unwrap(),
        )
        .unwrap();

        assert_eq!(g.max_depth, 123);
    }

    #[test]
    fn generate_short_max_depth_accepts_plain_nums() {
        let m = Ftzz::into_app()
            .get_matches_from(vec!["ftzz", "generate", "-d", "123", "-n", "1", "dir"]);
        let g = <Generate as FromArgMatches>::from_arg_matches(
            m.subcommand_matches("generate").unwrap(),
        )
        .unwrap();

        assert_eq!(g.max_depth, 123);
    }

    #[test]
    fn generate_ratio_rejects_negatives() {
        let f = Ftzz::try_parse_from(vec![
            "ftzz",
            "generate",
            "--ftd-ratio",
            "-1",
            "-n",
            "1",
            "dir",
        ]);

        assert!(f.is_err());
        assert_eq!(f.unwrap_err().kind, UnknownArgument);
    }

    #[test]
    fn generate_ratio_accepts_plain_nums() {
        let m = Ftzz::into_app().get_matches_from(vec![
            "ftzz",
            "generate",
            "--ftd-ratio",
            "1000",
            "-n",
            "1",
            "dir",
        ]);
        let g = <Generate as FromArgMatches>::from_arg_matches(
            m.subcommand_matches("generate").unwrap(),
        )
        .unwrap();

        assert_eq!(g.file_to_dir_ratio, Some(1000));
    }

    #[test]
    fn generate_short_ratio_accepts_plain_nums() {
        let m = Ftzz::into_app()
            .get_matches_from(vec!["ftzz", "generate", "-r", "321", "-n", "1", "dir"]);
        let g = <Generate as FromArgMatches>::from_arg_matches(
            m.subcommand_matches("generate").unwrap(),
        )
        .unwrap();

        assert_eq!(g.file_to_dir_ratio, Some(321));
    }

    #[test]
    fn generate_ratio_accepts_si_numbers() {
        let m = Ftzz::into_app().get_matches_from(vec![
            "ftzz",
            "generate",
            "--ftd-ratio",
            "1K",
            "-n",
            "1",
            "dir",
        ]);
        let g = <Generate as FromArgMatches>::from_arg_matches(
            m.subcommand_matches("generate").unwrap(),
        )
        .unwrap();

        assert_eq!(g.file_to_dir_ratio, Some(1000));
    }

    #[test]
    fn generate_ratio_accepts_commas() {
        let m = Ftzz::into_app().get_matches_from(vec![
            "ftzz",
            "generate",
            "--ftd-ratio",
            "1,000",
            "-n",
            "1",
            "dir",
        ]);
        let g = <Generate as FromArgMatches>::from_arg_matches(
            m.subcommand_matches("generate").unwrap(),
        )
        .unwrap();

        assert_eq!(g.file_to_dir_ratio, Some(1000));
    }

    #[test]
    fn generate_ratio_accepts_underscores() {
        let m = Ftzz::into_app().get_matches_from(vec![
            "ftzz",
            "generate",
            "--ftd-ratio",
            "1_000",
            "-n",
            "1",
            "dir",
        ]);
        let g = <Generate as FromArgMatches>::from_arg_matches(
            m.subcommand_matches("generate").unwrap(),
        )
        .unwrap();

        assert_eq!(g.file_to_dir_ratio, Some(1000));
    }

    #[test]
    fn generate_entropy_rejects_negatives() {
        let f = Ftzz::try_parse_from(vec![
            "ftzz",
            "generate",
            "--entropy",
            "-1",
            "-n",
            "1",
            "dir",
        ]);

        assert!(f.is_err());
        assert_eq!(f.unwrap_err().kind, UnknownArgument);
    }

    #[test]
    fn generate_entropy_accepts_plain_nums() {
        let m = Ftzz::into_app().get_matches_from(vec![
            "ftzz",
            "generate",
            "--entropy",
            "231",
            "-n",
            "1",
            "dir",
        ]);
        let g = <Generate as FromArgMatches>::from_arg_matches(
            m.subcommand_matches("generate").unwrap(),
        )
        .unwrap();

        assert_eq!(g.entropy, 231);
    }
}

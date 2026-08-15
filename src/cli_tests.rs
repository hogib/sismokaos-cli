use crate::cli::{Cli, Commands};
use std::path::PathBuf;

#[test]
fn cli_parses_preprocess_subcommand() {
    let cli = Cli::parse_from([
        "sismokaos-cli",
        "preprocess",
        "--data-dir",
        "./data",
        "--out-dir",
        "./out",
        "--fs",
        "50.0",
        "--freqmin",
        "0.1",
        "--freqmax",
        "20.0",
    ]);

    match cli.command {
        Commands::Preprocess {
            data_dir,
            out_dir,
            fs,
            freqmin,
            freqmax,
            dry_run,
        } => {
            assert_eq!(data_dir, PathBuf::from("./data"));
            assert_eq!(out_dir, PathBuf::from("./out"));
            assert_eq!(fs, Some(50.0));
            assert_eq!(freqmin, Some(0.1));
            assert_eq!(freqmax, Some(20.0));
            assert!(!dry_run);
        }
        _ => panic!("expected preprocess subcommand"),
    }
}

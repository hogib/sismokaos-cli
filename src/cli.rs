use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "quake-features")]
#[command(author, version, about = "Earthquake Signal Processing & Feature Extraction CLI", long_about = None)]
pub struct Cli {
    /// Path to the JSON configuration file
    #[arg(short, long, value_name = "FILE", default_value = "config.json")]
    pub config: PathBuf,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Run the full pipeline (Preprocess -> Extract)
    Run {
        /// Override the MSEED input file path
        #[arg(long)]
        mseed_file: Option<PathBuf>,

        /// Override the data output root directory
        #[arg(long)]
        out_dir: Option<PathBuf>,

        /// Override the station name (e.g., ELZG)
        #[arg(long)]
        station: Option<String>,

        /// Override window size in seconds
        #[arg(long)]
        win_sec: Option<u32>,

        /// Enable dry-run mode (validate config without processing)
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
    /// Initialize a default config.json in the current directory
    Init {
        #[arg(short, long, default_value = "config.json")]
        out: PathBuf,
    },
}

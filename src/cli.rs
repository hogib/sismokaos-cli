use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "sismokaos-cli")]
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
    /// Run the full pipeline (Preprocess -> Extract) over every miniSEED file in a directory
    Run {
        /// Directory containing the miniSEED files to process
        #[arg(long, value_name = "DATA")]
        data_dir: PathBuf,

        /// Directory to write feature CSVs (and run metadata) to
        #[arg(long, value_name = "OUT")]
        out_dir: PathBuf,

        /// Override the station name (used only as a metadata label)
        #[arg(long)]
        station: Option<String>,

        /// Override window size in seconds
        #[arg(long)]
        win_sec: Option<u32>,

        /// Override the target sampling rate (Hz) after decimation
        #[arg(long)]
        fs: Option<f64>,

        /// Override the bandpass filter minimum frequency (Hz)
        #[arg(long)]
        freqmin: Option<f64>,

        /// Override the bandpass filter maximum frequency (Hz)
        #[arg(long)]
        freqmax: Option<f64>,

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

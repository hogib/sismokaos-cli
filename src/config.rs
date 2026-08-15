use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::cli::Cli;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AppConfig {
    pub station: String,
    pub mseed_file: PathBuf,
    pub earthquake_name: String,
    pub raw_file_name: String,
    pub fs: f64,
    pub win_sec: u32,
    pub step_sec: u32,
    pub prev_sec: u32,
    pub sta_sec: f64,
    pub lta_sec: u32,
    pub n_jobs: i32,
    pub freqmin: f64,
    pub freqmax: f64,
    pub gap_threshold: f64,

    // Derived fields (calculated after loading, so we skip them during JSON parsing)
    #[serde(skip)]
    pub mseed_input_file: PathBuf,
    #[serde(skip)]
    pub data_root: PathBuf,
    #[serde(skip)]
    pub output_root: PathBuf,
    #[serde(skip)]
    pub win_size: usize,
    #[serde(skip)]
    pub step_size: usize,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            station: "ELZG".to_string(),
            mseed_file: PathBuf::from("raw/test.mseed"),
            earthquake_name: "24012020_M6.8_Sivrice__Elazig_".to_string(),
            raw_file_name: "TU_ELZG_24012020_000000_25012020_000000_HH.mseed".to_string(),
            fs: 5.0,
            win_sec: 200,
            step_sec: 50,
            prev_sec: 150,
            sta_sec: 0.5,
            lta_sec: 60,
            n_jobs: -1,
            freqmin: 0.1,
            freqmax: 2.0,
            gap_threshold: 2.0,
            // Defaults for derived fields (will be overwritten by finalize())
            mseed_input_file: PathBuf::new(),
            data_root: PathBuf::new(),
            output_root: PathBuf::new(),
            win_size: 0,
            step_size: 0,
        }
    }
}

impl AppConfig {
    /// Loads the configuration from a JSON file. Falls back to default if file doesn't exist.
    pub fn from_json<P: AsRef<Path>>(path: P) -> Self {
        if let Ok(json_str) = fs::read_to_string(&path) {
            match serde_json::from_str::<AppConfig>(&json_str) {
                Ok(mut config) => {
                    config.finalize_derived_fields();
                    return config;
                }
                Err(e) => eprintln!("[WARNING] Failed to parse JSON: {}. Using defaults.", e),
            }
        } else {
            eprintln!(
                "[WARNING] Config file {:?} not found. Using defaults.",
                path.as_ref()
            );
        }

        let mut default_cfg = AppConfig::default();
        default_cfg.finalize_derived_fields();
        default_cfg
    }

    /// Merges CLI flags into the configuration. CLI flags take precedence.
    pub fn merge_with_cli(&mut self, cli: &crate::cli::Cli) {
        if let crate::cli::Commands::Run {
            mseed_file,
            out_dir,
            station,
            win_sec,
            ..
        } = &cli.command
        {
            if let Some(s) = station {
                self.station = s.clone();
            }
            if let Some(w) = win_sec {
                self.win_sec = *w;
            }
            if let Some(m) = mseed_file {
                self.mseed_file = m.clone();
            }
            if let Some(o) = out_dir {
                self.output_root = o.clone();
            }
        }
        self.finalize_derived_fields();
    }

    fn finalize_derived_fields(&mut self) {
        let base_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        let stem = self.mseed_file.file_stem().unwrap_or_default();
        self.data_root = base_dir.join("data").join(stem);

        if self.output_root.as_os_str().is_empty() {
            self.output_root = base_dir.join("results").join(&self.station);
        }

        self.win_size = (self.win_sec as f64 * self.fs) as usize;
        self.step_size = (self.step_sec as f64 * self.fs) as usize;
    }
    pub fn save_to_json<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        let json_str = serde_json::to_string_pretty(self)?;
        fs::write(path, json_str)
    }
}

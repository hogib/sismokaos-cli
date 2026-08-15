use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AppConfig {
    pub station: String,
    pub fs: f64,
    pub win_sec: u32,
    pub step_sec: u32,
    pub sta_sec: f64,
    pub lta_sec: u32,
    pub freqmin: f64,
    pub freqmax: f64,

    // Derived fields (calculated after loading, so we skip them during JSON parsing)
    #[serde(skip)]
    pub data_dir: PathBuf,
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
            fs: 5.0,
            win_sec: 200,
            step_sec: 50,
            sta_sec: 0.5,
            lta_sec: 60,
            freqmin: 0.1,
            freqmax: 2.0,
            data_dir: PathBuf::new(),
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
                Ok(config) => return config,
                Err(e) => eprintln!("[WARNING] Failed to parse JSON: {}. Using defaults.", e),
            }
        } else {
            eprintln!(
                "[WARNING] Config file {:?} not found. Using defaults.",
                path.as_ref()
            );
        }

        AppConfig::default()
    }

    /// Merges CLI flags into the configuration. CLI flags take precedence.
    pub fn merge_with_cli(&mut self, cli: &crate::cli::Cli) {
        match &cli.command {
            crate::cli::Commands::Run {
                data_dir,
                out_dir,
                station,
                win_sec,
                fs,
                freqmin,
                freqmax,
                ..
            } => {
                self.data_dir = data_dir.clone();
                self.output_root = out_dir.clone();

                if let Some(s) = station {
                    self.station = s.clone();
                }
                if let Some(w) = win_sec {
                    self.win_sec = *w;
                }
                if let Some(f) = fs {
                    self.fs = *f;
                }
                if let Some(f) = freqmin {
                    self.freqmin = *f;
                }
                if let Some(f) = freqmax {
                    self.freqmax = *f;
                }
            }
            crate::cli::Commands::Preprocess {
                data_dir,
                out_dir,
                fs,
                freqmin,
                freqmax,
                ..
            } => {
                self.data_dir = data_dir.clone();
                self.output_root = out_dir.clone();

                if let Some(f) = fs {
                    self.fs = *f;
                }
                if let Some(f) = freqmin {
                    self.freqmin = *f;
                }
                if let Some(f) = freqmax {
                    self.freqmax = *f;
                }
            }
            crate::cli::Commands::Init { .. } => {}
        }

        self.finalize_derived_fields();
    }

    fn finalize_derived_fields(&mut self) {
        self.win_size = (self.win_sec as f64 * self.fs) as usize;
        self.step_size = (self.step_sec as f64 * self.fs) as usize;
    }

    pub fn save_to_json<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        let json_str = serde_json::to_string_pretty(self)?;
        fs::write(path, json_str)
    }
}

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

    /// Rejects parameter combinations that would silently corrupt the output.
    ///
    /// Returns the list of problems rather than the first, so one run surfaces
    /// everything wrong with a config instead of one item per attempt.
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();

        if self.fs <= 0.0 {
            problems.push(format!("fs must be positive, got {}", self.fs));
            return problems; // Everything below divides by it.
        }
        if self.freqmin <= 0.0 || self.freqmax <= self.freqmin {
            problems.push(format!(
                "need 0 < freqmin < freqmax, got freqmin={} freqmax={}",
                self.freqmin, self.freqmax
            ));
        }

        // The bandpass IS the anti-alias filter: decimation happens straight
        // after it, with no separate low-pass. If freqmax reaches the output
        // Nyquist, everything above it folds back into the band and there is
        // no way to tell afterwards that it happened.
        let nyquist = self.fs / 2.0;
        if self.freqmax >= nyquist {
            problems.push(format!(
                "freqmax {} Hz is at or above the output Nyquist ({} Hz for fs={}). \
                 The bandpass doubles as the anti-alias filter, so everything above \
                 {} Hz would alias back into the band with no way to detect it \
                 afterwards. Lower freqmax below {} Hz, or raise fs.",
                self.freqmax, nyquist, self.fs, nyquist, nyquist
            ));
        } else if self.freqmax > nyquist * 0.9 {
            eprintln!(
                "[WARNING] freqmax {} Hz sits within 10% of the output Nyquist ({} Hz). \
                 A 4th-order bandpass has not fully rolled off by then; some aliasing \
                 is likely.",
                self.freqmax, nyquist
            );
        }

        if self.win_sec == 0 || self.step_sec == 0 {
            problems.push("win_sec and step_sec must both be non-zero".to_string());
        }

        problems
    }

    /// Warns when the requested output rate is not an exact divisor of the
    /// native rate.
    ///
    /// The decimation factor is `native / requested` truncated to an integer,
    /// so asking for 30 Hz from a 100 Hz archive yields factor 3 and a real
    /// rate of 33.3 Hz. The pipeline stays self-consistent -- timestamps use
    /// the real rate -- but `win_size` was derived from the REQUESTED rate, so
    /// windows would be the wrong length. Better to say so than to let the two
    /// quietly disagree.
    pub fn check_decimation(&self, native_fs: f64) -> Option<String> {
        if native_fs <= 0.0 {
            return None;
        }
        let factor = ((native_fs / self.fs) as usize).max(1);
        let actual = native_fs / factor as f64;
        if (actual - self.fs).abs() > 1e-9 {
            return Some(format!(
                "requested fs={} Hz is not an exact divisor of the native {} Hz; \
                 decimating by {} gives {:.4} Hz instead. Window lengths were derived \
                 from the requested rate and will not match the data. \
                 Use an fs that divides {} exactly.",
                self.fs, native_fs, factor, actual, native_fs
            ));
        }
        None
    }

    pub fn save_to_json<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        let json_str = serde_json::to_string_pretty(self)?;
        fs::write(path, json_str)
    }
}

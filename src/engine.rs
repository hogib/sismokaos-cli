#![allow(non_snake_case)]
use npyz::Deserialize;
use rayon::prelude::*;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::Sender;

use crate::config::AppConfig;
use crate::features::compute_window;
use crate::types::PipelineEvent;

// Matches the structured array dtype outputted by the Python preprocessor
#[derive(Deserialize, Debug, Clone, Copy)]
#[allow(non_snake_case)]
struct EnzRecord {
    E: f64,
    N: f64,
    Z: f64,
}

pub fn run_pipeline(config: AppConfig, progress_tx: Sender<PipelineEvent>) -> Result<(), String> {
    if !config.data_root.exists() {
        return Err(format!("Data root not found: {:?}", config.data_root));
    }

    // 1. Find all NPY files in the data root (you can expand this to read sub-folders later)
    let mut npy_files: Vec<PathBuf> = fs::read_dir(&config.data_root)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "npy"))
        .collect();

    npy_files.sort();

    for path in npy_files {
        let file_name = path.file_name().unwrap().to_string_lossy().to_string();

        // 2. Read the structured NPY file
        let bytes = fs::read(&path).map_err(|e| format!("Failed to read file: {}", e))?;
        let reader =
            npyz::NpyFile::new(&bytes[..]).map_err(|e| format!("NPY parse error: {}", e))?;
        let records: Vec<EnzRecord> = reader
            .into_vec()
            .map_err(|e| format!("Deserialization error: {}", e))?;

        // 3. Un-interleave into contiguous memory for fast slicing
        let total_samples = records.len();
        let mut e_data = Vec::with_capacity(total_samples);
        let mut n_data = Vec::with_capacity(total_samples);
        let mut z_data = Vec::with_capacity(total_samples);

        for record in records {
            e_data.push(record.E);
            n_data.push(record.N);
            z_data.push(record.Z);
        }

        if total_samples < config.win_size {
            let _ = progress_tx.send(PipelineEvent::Warning(format!(
                "File {} too small for window size.",
                file_name
            )));
            continue;
        }
        let total_windows = (total_samples - config.win_size) / config.step_size + 1;
        let _ = progress_tx.send(PipelineEvent::FileStarted {
            path: path.clone(),
            total_windows,
        });

        let num_windows = (total_samples - config.win_size) / config.step_size + 1;
        let _ = progress_tx.send(PipelineEvent::FileStarted {
            path: path.clone(),
            total_windows,
        });

        let results: Vec<_> = (0..num_windows)
            .into_par_iter()
            .map(|w_idx| {
                let start = w_idx * config.step_size;
                let end = start + config.win_size;

                let time_minutes = (end as f64 / config.fs) / 60.0;

                let window_features = compute_window(
                    &e_data[start..end],
                    &n_data[start..end],
                    &z_data[start..end],
                    &config,
                );

                // Notify UI that a window is done (Rayon threads share this channel safely)
                let _ = progress_tx.send(PipelineEvent::WindowProcessed);

                (w_idx, time_minutes, window_features)
            })
            .collect();

        let file_stem = path.file_stem().unwrap().to_string_lossy().to_string();
        let csv_path = config
            .output_root
            .join(format!("{}_features.csv", file_stem));

        if let Err(e) = crate::export::save_results(results, &file_stem, &csv_path) {
            let _ = progress_tx.send(PipelineEvent::Warning(format!("Failed to save CSV: {}", e)));
        }

        // NOTE: In Step 5, we will take `_results` and save it to CSV/Parquet!
        let _ = progress_tx.send(PipelineEvent::FileCompleted);
    }

    let _ = progress_tx.send(PipelineEvent::Finished);
    Ok(())
}

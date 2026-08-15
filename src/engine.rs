use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::mpsc::Sender;

use crate::config::AppConfig;
use crate::export::FeatureWriter;
use crate::features::compute_window;
use crate::preprocess::{self, ChannelChunk};
use crate::types::PipelineEvent;

pub fn run_pipeline(config: AppConfig, progress_tx: Sender<PipelineEvent>) -> Result<(), String> {
    if !config.data_dir.exists() {
        return Err(format!("Data directory not found: {:?}", config.data_dir));
    }

    let _ = progress_tx.send(PipelineEvent::FolderStarted(
        config.data_dir.display().to_string(),
    ));
    let _ = progress_tx.send(PipelineEvent::FileStarted {
        path: config.data_dir.clone(),
        total_windows: 0,
    });

    let file_stem = config
        .data_dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "output".to_string());
    let csv_path = config
        .output_root
        .join(format!("{}_features.csv", file_stem));

    // Rolling buffers: preprocessing hands us the directory one file at a time, and windows are
    // extracted as soon as enough samples accumulate, then written straight to CSV. Only the
    // not-yet-windowed tail (< win_size samples) and one chunk's worth of feature rows are ever
    // held at once, so memory stays flat no matter how long the recording is.
    let mut buf_e: Vec<f64> = Vec::new();
    let mut buf_n: Vec<f64> = Vec::new();
    let mut buf_z: Vec<f64> = Vec::new();
    let mut global_offset: usize = 0;
    let mut window_idx: usize = 0;
    let mut fs_out = config.fs;
    let mut writer: Option<FeatureWriter> = None;

    preprocess::preprocess_directory_chunked(&config.data_dir, &config, |chunk: ChannelChunk| {
        fs_out = chunk.fs;
        buf_e.extend(chunk.e);
        buf_n.extend(chunk.n);
        buf_z.extend(chunk.z);

        let mut starts = Vec::new();
        let mut cursor = 0usize;
        while buf_e.len() - cursor >= config.win_size {
            starts.push(cursor);
            cursor += config.step_size;
        }

        let mut per_window_config = config.clone();
        per_window_config.fs = fs_out;

        let chunk_results: Vec<(f64, HashMap<String, f64>)> = starts
            .par_iter()
            .map(|&start| {
                let end = start + config.win_size;
                let time_minutes = ((global_offset + end) as f64 / fs_out) / 60.0;
                let window_features = compute_window(
                    &buf_e[start..end],
                    &buf_n[start..end],
                    &buf_z[start..end],
                    &per_window_config,
                );
                (time_minutes, window_features)
            })
            .collect();

        for (time_minutes, features) in chunk_results {
            let w = match writer.as_mut() {
                Some(w) => w,
                None => {
                    let new_writer = FeatureWriter::new(&csv_path, &features)
                        .map_err(|e| format!("Failed to create output CSV: {}", e))?;
                    writer = Some(new_writer);
                    writer.as_mut().unwrap()
                }
            };

            let window_id = format!("{}_w{:02}", file_stem, window_idx + 1);
            w.write_window(&window_id, time_minutes, &features)
                .map_err(|e| format!("Failed to write CSV row: {}", e))?;

            let _ = progress_tx.send(PipelineEvent::WindowProcessed);
            window_idx += 1;
        }

        if cursor > 0 {
            buf_e.drain(0..cursor);
            buf_n.drain(0..cursor);
            buf_z.drain(0..cursor);
            global_offset += cursor;
        }

        Ok(())
    })?;

    match writer {
        None => {
            let _ = progress_tx.send(PipelineEvent::Warning(
                "Not enough data for a single window.".to_string(),
            ));
        }
        Some(w) => {
            let mut final_config = config.clone();
            final_config.fs = fs_out;
            w.finish(&csv_path, &final_config)
                .map_err(|e| format!("Failed to finalize output: {}", e))?;
            let _ = progress_tx.send(PipelineEvent::FileCompleted);
        }
    }

    let _ = progress_tx.send(PipelineEvent::Finished);
    Ok(())
}

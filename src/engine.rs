use rayon::prelude::*;
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{Sender, sync_channel};
use std::thread;

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

    let file_stem = config
        .data_dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "output".to_string());
    let parquet_path = config
        .output_root
        .join(format!("{}_features.parquet", file_stem));

    let (chunk_tx, chunk_rx) = sync_channel::<ChannelChunk>(2);

    let preprocess_config = config.clone();
    let data_dir = config.data_dir.clone();

    // Spawn the preprocessor in a background thread
    let preprocessor_thread = thread::spawn(move || {
        preprocess::preprocess_directory_chunked(&data_dir, &preprocess_config, |chunk| {
            if chunk_tx.send(chunk).is_err() {
                return Err("Pipeline channel closed unexpectedly.".to_string());
            }
            Ok(())
        })
    });

    let mut buf_e: VecDeque<f64> = VecDeque::new();
    let mut buf_n: VecDeque<f64> = VecDeque::new();
    let mut buf_z: VecDeque<f64> = VecDeque::new();

    let mut global_offset: usize = 0;
    let mut window_idx: usize = 0;
    let mut fs_out = config.fs;
    let mut writer: Option<FeatureWriter> = None;
    let mut current_buffer_epoch: f64 = f64::NAN;

    // Consume chunks as they arrive asynchronously
    for chunk in chunk_rx {
        fs_out = chunk.fs;
        let expected_next_epoch = current_buffer_epoch + (buf_e.len() as f64 / fs_out);

        if current_buffer_epoch.is_nan()
            || buf_e.is_empty()
            || (chunk.start_epoch - expected_next_epoch).abs() > (1.5 / fs_out)
        {
            buf_e.clear();
            buf_n.clear();
            buf_z.clear();
            current_buffer_epoch = chunk.start_epoch;
        }
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

        // Ensure buffers are contiguous in memory before slicing for the Rayon iteration
        let slice_e = buf_e.make_contiguous();
        let slice_n = buf_n.make_contiguous();
        let slice_z = buf_z.make_contiguous();

        // Data-parallel feature computation for all windows in this chunk
        let chunk_results: Vec<(f64, HashMap<&'static str, f64>)> = starts
            .par_iter()
            .map(|&start| {
                let end = start + config.win_size;
                let window_epoch = current_buffer_epoch + (end as f64 / fs_out);
                let time_minutes = window_epoch / 60.0;

                let window_features = compute_window(
                    &slice_e[start..end],
                    &slice_n[start..end],
                    &slice_z[start..end],
                    &per_window_config,
                );
                (time_minutes, window_features)
            })
            .collect();

        for (time_minutes, features) in chunk_results {
            let w = match writer.as_mut() {
                Some(w) => w,
                None => {
                    let new_writer = FeatureWriter::new(&parquet_path, &features)
                        .map_err(|e| format!("Failed to create output parquet: {}", e))?;
                    writer = Some(new_writer);
                    writer.as_mut().unwrap()
                }
            };

            let window_id = format!("{}_w{:02}", file_stem, window_idx + 1);
            w.write_window(&window_id, time_minutes, &features)
                .map_err(|e| format!("Failed to write parquet row: {}", e))?;

            let _ = progress_tx.send(PipelineEvent::WindowProcessed);
            window_idx += 1;
        }

        if cursor > 0 {
            // O(1) instantaneous memory pop via VecDeque
            buf_e.drain(0..cursor);
            buf_n.drain(0..cursor);
            buf_z.drain(0..cursor);
            current_buffer_epoch += cursor as f64 / fs_out;
            global_offset += cursor;
        }
    }

    preprocessor_thread
        .join()
        .map_err(|_| "Preprocessor thread panicked".to_string())??;

    match writer {
        None => {
            let _ = progress_tx.send(PipelineEvent::Warning(
                "Not enough data for a single window.".to_string(),
            ));
        }
        Some(w) => {
            let mut final_config = config.clone();
            final_config.fs = fs_out;
            w.finish(&parquet_path, &final_config)
                .map_err(|e| format!("Failed to finalize output: {}", e))?;
            let _ = progress_tx.send(PipelineEvent::Completed);
        }
    }

    let _ = progress_tx.send(PipelineEvent::Finished);
    Ok(())
}

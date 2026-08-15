use crate::config::AppConfig;
use crate::preprocess::{self, ChannelChunk};
use crate::types::PipelineEvent;
use std::fs::File;
use std::io::{BufWriter, Write};

pub fn run_preprocess_only(
    config: AppConfig,
    progress_tx: std::sync::mpsc::Sender<PipelineEvent>,
) -> Result<(), String> {
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

    let out_path = config
        .output_root
        .join(format!("{}_preprocessed.csv", file_stem));

    std::fs::create_dir_all(&config.output_root)
        .map_err(|e| format!("Failed to create output directory {:?}: {}", config.output_root, e))?;

    let file = File::create(&out_path)
        .map_err(|e| format!("Failed to create preprocess output {:?}: {}", out_path, e))?;
    let mut writer = BufWriter::new(file);

    writeln!(writer, "index,time_minutes,E,N,Z")
        .map_err(|e| format!("Failed to write preprocess CSV header: {}", e))?;

    let mut global_index: usize = 0;
    let mut fs_out = config.fs;

    preprocess::preprocess_directory_chunked(&config.data_dir, &config, |chunk: ChannelChunk| {
        fs_out = chunk.fs;

        let len = chunk
            .e
            .len()
            .min(chunk.n.len())
            .min(chunk.z.len());

        for i in 0..len {
            let idx = global_index + i;
            let time_minutes = (idx as f64 / fs_out) / 60.0;
            writeln!(
                writer,
                "{},{:.8},{:.10},{:.10},{:.10}",
                idx, time_minutes, chunk.e[i], chunk.n[i], chunk.z[i]
            )
            .map_err(|e| format!("Failed to write preprocess CSV row: {}", e))?;
            let _ = progress_tx.send(PipelineEvent::WindowProcessed);
        }

        global_index += len;
        Ok(())
    })?;

    writer
        .flush()
        .map_err(|e| format!("Failed to flush preprocess CSV {:?}: {}", out_path, e))?;

    if global_index == 0 {
        let _ = progress_tx.send(PipelineEvent::Warning(
            "No samples produced during preprocessing.".to_string(),
        ));
    } else {
        let _ = progress_tx.send(PipelineEvent::Completed);
    }

    let _ = progress_tx.send(PipelineEvent::Finished);
    Ok(())
}

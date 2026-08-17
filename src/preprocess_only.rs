use crate::config::AppConfig;
use crate::preprocess::{self, ChannelChunk};
use crate::raw_binary::RawBinaryWriter;
use crate::types::PipelineEvent;

/// Preprocess-only pipeline: filtered, decimated E/N/Z written as flat
/// little-endian f32 plus a JSON sidecar.
///
/// See `raw_binary` for why this is not Parquet. In short: the Parquet was
/// read exactly once, by a Python script that rewrote it as this very layout,
/// and it cost 16.41 GB where this costs 8.63 GB for the same 10 Hz archive.
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
        .join(format!("{}_preprocessed", file_stem));

    std::fs::create_dir_all(&config.output_root).map_err(|e| {
        format!(
            "Failed to create output directory {:?}: {}",
            config.output_root, e
        )
    })?;

    let mut writer = RawBinaryWriter::create(&out_path)?;
    let mut total: u64 = 0;

    preprocess::preprocess_directory_chunked(&config.data_dir, &config, |chunk: ChannelChunk| {
        let len = chunk.e.len().min(chunk.n.len()).min(chunk.z.len());

        if chunk.e.len() != len || chunk.n.len() != len || chunk.z.len() != len {
            let _ = progress_tx.send(PipelineEvent::Warning(format!(
                "Channel lengths mismatched in chunk! E: {}, N: {}, Z: {}",
                chunk.e.len(),
                chunk.n.len(),
                chunk.z.len()
            )));
        }

        writer.write_chunk(
            &chunk.e[..len],
            &chunk.n[..len],
            &chunk.z[..len],
            chunk.fs,
            chunk.start_epoch,
        )?;
        total += len as u64;

        let _ = progress_tx.send(PipelineEvent::ChunkProcessed(len));
        Ok(())
    })?;

    let path = writer.path().to_path_buf();
    let samples = writer.finish(&config)?;

    if samples == 0 {
        let _ = progress_tx.send(PipelineEvent::Warning(
            "No samples produced during preprocessing.".to_string(),
        ));
    } else {
        println!(
            "[OUTPUT] {} samples x 3 channels -> {:?} ({:.2} GB)",
            samples,
            path,
            (samples as f64 * 3.0 * 4.0) / 1e9
        );
        println!("[OUTPUT] metadata -> {:?}", path.with_extension("f32.json"));
        let _ = progress_tx.send(PipelineEvent::Completed);
    }

    let _ = progress_tx.send(PipelineEvent::Finished);
    Ok(())
}

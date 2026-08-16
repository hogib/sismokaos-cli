use crate::config::AppConfig;
use crate::preprocess::{self, ChannelChunk};
use crate::types::PipelineEvent;
use csv::{StringRecord, WriterBuilder};
use std::fs::File;
use std::io::BufWriter;

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

    std::fs::create_dir_all(&config.output_root).map_err(|e| {
        format!(
            "Failed to create output directory {:?}: {}",
            config.output_root, e
        )
    })?;

    let file = File::create(&out_path)
        .map_err(|e| format!("Failed to create preprocess output {:?}: {}", out_path, e))?;

    let buffered_file = BufWriter::with_capacity(256 * 1024, file);
    let mut writer = WriterBuilder::new().from_writer(buffered_file);

    writer
        .write_record(&["index", "time_minutes", "E", "N", "Z"])
        .map_err(|e| format!("Failed to write preprocess CSV header: {}", e))?;

    let mut global_index: usize = 0;
    let mut fs_out = config.fs;
    let mut record = StringRecord::with_capacity(128, 5);

    let mut ryu_buf = ryu::Buffer::new();
    let mut itoa_buf = itoa::Buffer::new();

    preprocess::preprocess_directory_chunked(&config.data_dir, &config, |chunk: ChannelChunk| {
        fs_out = chunk.fs;

        let len = chunk.e.len().min(chunk.n.len()).min(chunk.z.len());

        if chunk.e.len() != len || chunk.n.len() != len || chunk.z.len() != len {
            let _ = progress_tx.send(PipelineEvent::Warning(format!(
                "Channel lengths mismatched in chunk! E: {}, N: {}, Z: {}",
                chunk.e.len(),
                chunk.n.len(),
                chunk.z.len()
            )));
        }

        for i in 0..len {
            let idx = global_index + i;
            let sample_epoch = chunk.start_epoch + (i as f64 / fs_out);
            let time_minutes = sample_epoch / 60.0;

            record.clear();
            record.push_field(itoa_buf.format(idx));
            record.push_field(ryu_buf.format(time_minutes));
            record.push_field(ryu_buf.format(chunk.e[i]));
            record.push_field(ryu_buf.format(chunk.n[i]));
            record.push_field(ryu_buf.format(chunk.z[i]));

            writer
                .write_record(&record)
                .map_err(|e| format!("Failed to write preprocess CSV row: {}", e))?;
        }

        global_index += len;
        let _ = progress_tx.send(PipelineEvent::ChunkProcessed(len));
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

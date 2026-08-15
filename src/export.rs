use crate::config::AppConfig;
use csv::{StringRecord, Writer, WriterBuilder};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::Path;

/// Writes feature rows to CSV incrementally.
pub struct FeatureWriter {
    writer: Writer<BufWriter<File>>,
    feature_names: Vec<&'static str>,
    prev_values: Option<Vec<f64>>,
    record: StringRecord,
    rows_written: usize,
}

impl FeatureWriter {
    pub fn new(
        output_path: &Path,
        first_features: &HashMap<&'static str, f64>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut feature_names: Vec<&'static str> = first_features.keys().copied().collect();
        feature_names.sort();

        let file = File::create(output_path)?;
        let buffered_file = BufWriter::with_capacity(128 * 1024, file);
        let mut writer = WriterBuilder::new().from_writer(buffered_file);

        let expected_cols = 2 + (feature_names.len() * 2);

        let record = StringRecord::with_capacity(1024, expected_cols);

        let mut header = vec!["Pencere_ID".to_string(), "Zaman_Dk".to_string()];
        header.extend(feature_names.iter().map(|s| s.to_string()));
        header.extend(feature_names.iter().map(|n| format!("{}_DEV", n)));
        writer.write_record(&header)?;

        Ok(Self {
            writer,
            feature_names,
            prev_values: None,
            record,
            rows_written: 0,
        })
    }

    pub fn write_window(
        &mut self,
        window_id: &str,
        time_min: f64,
        features: &HashMap<&'static str, f64>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.record.clear();

        self.record.push_field(window_id);
        self.record.push_field(&format!("{:.6}", time_min));

        let mut current_values = Vec::with_capacity(self.feature_names.len());
        for name in &self.feature_names {
            let v = features.get(name).copied().unwrap_or(f64::NAN);
            current_values.push(v);
            push_formatted_value(&mut self.record, v);
        }

        match &self.prev_values {
            Some(prev) => {
                for (&v, &p) in current_values.iter().zip(prev.iter()) {
                    push_formatted_value(&mut self.record, v - p);
                }
            }
            None => {
                for _ in 0..self.feature_names.len() {
                    self.record.push_field("");
                }
            }
        }

        self.writer.write_record(&self.record)?;
        self.prev_values = Some(current_values);
        self.rows_written += 1;
        Ok(())
    }

    pub fn finish(
        mut self,
        output_path: &Path,
        config: &AppConfig,
    ) -> Result<usize, Box<dyn std::error::Error>> {
        self.writer.flush()?;
        let metadata_path = output_path.with_extension("run_metadata.json");
        config.save_to_json(&metadata_path)?;
        Ok(self.rows_written)
    }
}

#[inline(always)]
fn push_formatted_value(record: &mut StringRecord, v: f64) {
    if v.is_nan() {
        record.push_field("");
    } else {
        record.push_field(&format!("{:.6}", v));
    }
}

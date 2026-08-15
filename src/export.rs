use crate::config::AppConfig;
use csv::{Writer, WriterBuilder};
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::Path;

/// Writes feature rows to CSV incrementally.
///
/// Rows are streamed straight to disk as they are computed rather than being buffered up for the
/// whole run, so memory stays flat regardless of how many windows the directory produces. The
/// `_DEV` (first-difference) columns only need the immediately preceding row, so just that one
/// row of values is retained between writes.
pub struct FeatureWriter {
    writer: Writer<File>,
    feature_names: Vec<String>,
    prev_values: Option<Vec<f64>>,
    rows_written: usize,
}

impl FeatureWriter {
    /// Creates the output file and writes the header, derived from the first window's feature set.
    pub fn new(
        output_path: &Path,
        first_features: &HashMap<String, f64>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut feature_names: Vec<String> = first_features.keys().cloned().collect();
        feature_names.sort(); // Sort alphabetically for a consistent CSV layout

        let file = File::create(output_path)?;
        let mut writer = WriterBuilder::new().from_writer(file);

        let mut header = vec!["Pencere_ID".to_string(), "Zaman_Dk".to_string()];
        header.extend(feature_names.iter().cloned());
        header.extend(feature_names.iter().map(|n| format!("{}_DEV", n)));
        writer.write_record(&header)?;

        Ok(Self {
            writer,
            feature_names,
            prev_values: None,
            rows_written: 0,
        })
    }

    pub fn write_window(
        &mut self,
        window_id: &str,
        time_min: f64,
        features: &HashMap<String, f64>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let values: Vec<f64> = self
            .feature_names
            .iter()
            .map(|name| features.get(name).copied().unwrap_or(f64::NAN))
            .collect();

        let mut row = vec![window_id.to_string(), format!("{:.6}", time_min)];
        row.extend(values.iter().map(|&v| format_value(v)));

        // The first row has no predecessor, so its derivatives are blank.
        match &self.prev_values {
            Some(prev) => row.extend(
                values
                    .iter()
                    .zip(prev.iter())
                    .map(|(&v, &p)| format_value(v - p)),
            ),
            None => row.extend(self.feature_names.iter().map(|_| String::new())),
        }

        self.writer.write_record(&row)?;
        self.prev_values = Some(values);
        self.rows_written += 1;
        Ok(())
    }

    /// Flushes the CSV and writes the run metadata sidecar next to it.
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

fn format_value(v: f64) -> String {
    if v.is_nan() {
        String::new()
    } else {
        format!("{:.6}", v)
    }
}

use crate::config::AppConfig;
use polars::prelude::*;
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::Path;

/// Accumulates feature rows in memory and writes to Parquet on finish.
pub struct FeatureWriter {
    feature_names: Vec<&'static str>,
    prev_values: Option<Vec<f64>>,
    rows_written: usize,

    // Columnar buffers for Polars
    pencere_ids: Vec<String>,
    zaman_dks: Vec<f64>,
    feature_cols: Vec<Vec<Option<f64>>>,
    dev_cols: Vec<Vec<Option<f64>>>,

    // Quality flags, deliberately NOT features: they are held apart from
    // `feature_names` so they get no `_DEV` twin (a first difference of a
    // gap fraction is meaningless) and so adding them cannot perturb the
    // `_DEV` diffing of any real feature.
    interp_cols: [Vec<f64>; 3],
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

        let num_features = feature_names.len();

        Ok(Self {
            feature_names,
            prev_values: None,
            rows_written: 0,
            // Pre-allocate to avoid reallocation overhead (100_000 hours ≈ 11.4 years)
            pencere_ids: Vec::with_capacity(100_000),
            zaman_dks: Vec::with_capacity(100_000),
            feature_cols: vec![Vec::with_capacity(100_000); num_features],
            dev_cols: vec![Vec::with_capacity(100_000); num_features],
            // Constructed unconditionally, so a run whose first window happens
            // to be gap-free produces the same schema as one whose first
            // window is not.
            interp_cols: [
                Vec::with_capacity(100_000),
                Vec::with_capacity(100_000),
                Vec::with_capacity(100_000),
            ],
        })
    }

    pub fn write_window(
        &mut self,
        window_id: &str,
        time_min: f64,
        features: &HashMap<&'static str, f64>,
        interp: [f64; 3],
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.pencere_ids.push(window_id.to_string());
        self.zaman_dks.push(time_min);
        for (col, v) in self.interp_cols.iter_mut().zip(interp) {
            col.push(v);
        }

        let mut current_values = Vec::with_capacity(self.feature_names.len());

        for (i, name) in self.feature_names.iter().enumerate() {
            let v = features.get(name).copied().unwrap_or(f64::NAN);
            current_values.push(v);

            // Map NaN to None so Parquet records a true NULL
            // (matches the old logic of pushing an empty string "")
            let v_opt = if v.is_nan() { None } else { Some(v) };
            self.feature_cols[i].push(v_opt);
        }

        match &self.prev_values {
            Some(prev) => {
                for (i, (&v, &p)) in current_values.iter().zip(prev.iter()).enumerate() {
                    let dev = if v.is_nan() || p.is_nan() {
                        None
                    } else {
                        Some(v - p)
                    };
                    self.dev_cols[i].push(dev);
                }
            }
            None => {
                for i in 0..self.feature_names.len() {
                    self.dev_cols[i].push(None); // First row has no previous data
                }
            }
        }

        self.prev_values = Some(current_values);
        self.rows_written += 1;
        Ok(())
    }

    pub fn finish(
        mut self,
        output_path: &Path,
        config: &AppConfig,
    ) -> Result<usize, Box<dyn std::error::Error>> {
        let parquet_path = output_path.with_extension("parquet");

        let mut columns = Vec::new();

        columns.push(Series::new("Pencere_ID".into(), &self.pencere_ids));
        columns.push(Series::new("Zaman_Dk".into(), &self.zaman_dks));

        for (i, name) in self.feature_names.iter().enumerate() {
            columns.push(Series::new((*name).into(), &self.feature_cols[i]));
        }

        for (i, name) in self.feature_names.iter().enumerate() {
            let dev_name = format!("{}_DEV", name);
            columns.push(Series::new(dev_name.as_str().into(), &self.dev_cols[i]));
        }

        // Fraction of each component's samples in the window that the gap
        // interpolant fabricated. Without these a reconstructed window is
        // indistinguishable from a recorded one, and a flat interpolated
        // segment reads as a confident low-dimensional chaos measurement.
        for (name, col) in ["E_INTERP_FRAC", "N_INTERP_FRAC", "Z_INTERP_FRAC"]
            .iter()
            .zip(self.interp_cols.iter())
        {
            columns.push(Series::new((*name).into(), col));
        }

        let mut df = DataFrame::new(columns)?;

        let file = File::create(&parquet_path)?;
        ParquetWriter::new(file)
            .with_compression(ParquetCompression::Zstd(None)) // Zstd compression
            .finish(&mut df)?;

        let metadata_path = parquet_path.with_extension("run_metadata.json");
        config.save_to_json(&metadata_path)?;

        Ok(self.rows_written)
    }
}

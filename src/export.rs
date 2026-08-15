use csv::WriterBuilder;
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::Path;

pub fn save_results(
    results: Vec<(usize, f64, HashMap<String, f64>)>,
    file_identifier: &str,
    output_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if results.is_empty() {
        return Ok(());
    }

    // Ensure the output directory exists
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Pivot the row-based results into column-based vectors for easy math
    let mut window_ids = Vec::with_capacity(results.len());
    let mut times = Vec::with_capacity(results.len());
    let mut columns: HashMap<String, Vec<f64>> = HashMap::new();

    // Setup empty vectors for every feature key found in the first window
    let first_features = &results[0].2;
    let mut feature_names: Vec<String> = first_features.keys().cloned().collect();
    feature_names.sort(); // Sort alphabetically for a consistent CSV layout

    for name in &feature_names {
        columns.insert(name.clone(), Vec::with_capacity(results.len()));
    }

    // Populate the vectors
    for (w_idx, time_min, features) in results {
        window_ids.push(format!("{}_w{:02}", file_identifier, w_idx + 1));
        times.push(time_min);
        for name in &feature_names {
            let val = features.get(name).copied().unwrap_or(f64::NAN);
            columns.get_mut(name).unwrap().push(val);
        }
    }

    // Calculate the derivatives
    let mut diff_columns: HashMap<String, Vec<f64>> = HashMap::new();
    for name in &feature_names {
        let vals = &columns[name];
        let mut diffs = Vec::with_capacity(vals.len());

        diffs.push(f64::NAN); // First row has no previous row to subtract from
        for i in 1..vals.len() {
            diffs.push(vals[i] - vals[i - 1]);
        }
        diff_columns.insert(format!("{}_DEV", name), diffs);
    }

    // Write to CSV
    let file = File::create(output_path)?;
    let mut wtr = WriterBuilder::new().from_writer(file);

    // Build and write the header row
    let mut header = vec!["Pencere_ID".to_string(), "Zaman_Dk".to_string()];
    for name in &feature_names {
        header.push(name.clone());
    }
    for name in &feature_names {
        header.push(format!("{}_DEV", name));
    }
    wtr.write_record(&header)?;

    // Build and write the data rows
    for i in 0..window_ids.len() {
        let mut row = vec![window_ids[i].clone(), format!("{:.6}", times[i])];

        // Write base features
        for name in &feature_names {
            let val = columns[name][i];
            row.push(if val.is_nan() {
                "".to_string()
            } else {
                format!("{:.6}", val)
            });
        }

        // Write derivative features
        for name in &feature_names {
            let val = diff_columns[&format!("{}_DEV", name)][i];
            row.push(if val.is_nan() {
                "".to_string()
            } else {
                format!("{:.6}", val)
            });
        }

        wtr.write_record(&row)?;
    }

    wtr.flush()?;
    Ok(())
}

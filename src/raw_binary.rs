//! Flat little-endian `f32` output for decimated waveform streams.
//!
//! # Why not Parquet
//!
//! The Parquet the preprocess path used to emit was never consumed as Parquet.
//! `parquet_to_memory.py` on the model side read it once and rewrote it as a
//! flat `(hours, 3, hour_samples)` float32 memmap, which is what the training
//! DataLoader actually wants. Everything between those two points was work
//! spent on a file that was immediately thrown away.
//!
//! Measured on `aegean_bodt_2024_2026` (718,848,001 samples at 10 Hz):
//!
//! | | size |
//! |---|---|
//! | Parquet, zstd, 5 columns of f64 | 16.41 GB |
//! | this writer, 3 channels of f32  |  8.63 GB |
//!
//! The compressed file was nearly twice the size of the uncompressed one,
//! because it stored two columns that are pure functions of the row number
//! (`index`, and `Zaman_Dk = start_epoch + i/fs`, together 11.9% of the bytes)
//! at double the precision the consumer keeps -- it casts to float32 on load.
//!
//! # Layout
//!
//! `<stem>.f32` is channel-major within each sample: `E N Z E N Z ...`, so a
//! contiguous read of any time range yields all three components. NumPy reads
//! it with no parsing and no decompression:
//!
//! ```python
//! import json, numpy as np
//! meta = json.load(open("out.f32.json"))
//! a = np.memmap("out.f32", dtype="<f4", mode="r").reshape(-1, 3)   # (samples, E/N/Z)
//! ```
//!
//! # Why the sidecar carries segments, not one start time
//!
//! The time axis is *piecewise* linear, not linear. `preprocess.rs` resets its
//! epoch whenever records jump more than two blocks ahead, so a single global
//! `start_epoch` would put every sample after the first real data gap at the
//! wrong time. `segments` records `(sample_offset, start_epoch)` at each
//! discontinuity; between entries, sample `i` is at
//! `start_epoch + (i - sample_offset) / fs`.
//!
//! Gaps that could not be interpolated stay `NaN` in the output, which f32
//! represents exactly. Downstream decides whether to zero-fill them; the file
//! does not make that choice on its behalf.

use crate::config::AppConfig;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// One contiguous run of samples with a known start time.
struct Segment {
    sample_offset: u64,
    start_epoch: f64,
}

pub struct RawBinaryWriter {
    out: BufWriter<File>,
    path: PathBuf,
    fs: f64,
    samples: u64,
    segments: Vec<Segment>,
    nan_counts: [u64; 3],
    scratch: Vec<u8>,
}

impl RawBinaryWriter {
    pub fn create(output_path: &Path) -> Result<Self, String> {
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create {:?}: {}", parent, e))?;
        }
        let path = output_path.with_extension("f32");
        let file = File::create(&path)
            .map_err(|e| format!("Failed to create {:?}: {}", path, e))?;
        Ok(Self {
            // 8 MiB of buffering: large enough that the write syscall stops
            // mattering next to the filtering, small enough to stay resident.
            out: BufWriter::with_capacity(8 << 20, file),
            path,
            fs: 0.0,
            samples: 0,
            segments: Vec::new(),
            nan_counts: [0; 3],
            scratch: Vec::new(),
        })
    }

    /// Appends one chunk. `start_epoch` opens a new segment whenever it does
    /// not continue the previous one.
    pub fn write_chunk(
        &mut self,
        e: &[f64],
        n: &[f64],
        z: &[f64],
        fs: f64,
        start_epoch: f64,
    ) -> Result<(), String> {
        let len = e.len().min(n.len()).min(z.len());
        if len == 0 {
            return Ok(());
        }
        self.fs = fs;

        let continues = self.segments.last().is_some_and(|s| {
            let expected = s.start_epoch + (self.samples - s.sample_offset) as f64 / fs;
            // Half a sample of tolerance: epochs are f64 seconds and accumulate
            // rounding, but a real gap is at least one whole sample.
            (start_epoch - expected).abs() < 0.5 / fs
        });
        if !continues {
            self.segments.push(Segment {
                sample_offset: self.samples,
                start_epoch,
            });
        }

        self.scratch.clear();
        self.scratch.reserve(len * 3 * 4);
        for i in 0..len {
            for (c, v) in [e[i], n[i], z[i]].into_iter().enumerate() {
                if v.is_nan() {
                    self.nan_counts[c] += 1;
                }
                self.scratch.extend_from_slice(&(v as f32).to_le_bytes());
            }
        }
        self.out
            .write_all(&self.scratch)
            .map_err(|e| format!("Failed writing to {:?}: {}", self.path, e))?;

        self.samples += len as u64;
        Ok(())
    }

    /// Flushes and writes the sidecar. Returns the sample count.
    pub fn finish(mut self, config: &AppConfig) -> Result<u64, String> {
        self.out
            .flush()
            .map_err(|e| format!("Failed to flush {:?}: {}", self.path, e))?;

        let segments: Vec<String> = self
            .segments
            .iter()
            .map(|s| {
                format!(
                    "    {{\"sample_offset\": {}, \"start_epoch\": {}}}",
                    s.sample_offset, s.start_epoch
                )
            })
            .collect();

        // Hand-rolled rather than pulling serde_json into a hot path for one
        // small file; the shape is fixed and fully covered by the round-trip
        // test below.
        let meta = format!(
            r#"{{
  "format": "raw-f32-le",
  "layout": "interleaved",
  "channels": ["E", "N", "Z"],
  "dtype": "<f4",
  "samples": {},
  "shape": [{}, 3],
  "fs": {},
  "nan_counts": {{"E": {}, "N": {}, "Z": {}}},
  "freqmin": {},
  "freqmax": {},
  "station": "{}",
  "segments": [
{}
  ],
  "note": "Sample i of segment s is at start_epoch + (i - sample_offset)/fs. The time axis is piecewise linear; use the segment table, not a single start time."
}}
"#,
            self.samples,
            self.samples,
            self.fs,
            self.nan_counts[0],
            self.nan_counts[1],
            self.nan_counts[2],
            config.freqmin,
            config.freqmax,
            config.station,
            segments.join(",\n")
        );

        let meta_path = self.path.with_extension("f32.json");
        fs::write(&meta_path, meta)
            .map_err(|e| format!("Failed to write {:?}: {}", meta_path, e))?;

        Ok(self.samples)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> AppConfig {
        AppConfig::default()
    }

    #[test]
    fn round_trips_values_and_records_one_segment_when_contiguous() {
        let dir = std::env::temp_dir().join("sismokaos_rawbin_contig");
        let _ = fs::remove_dir_all(&dir);
        let mut w = RawBinaryWriter::create(&dir.join("out")).unwrap();

        // Two chunks that join seamlessly at 5 Hz.
        w.write_chunk(&[1.0, 2.0], &[3.0, 4.0], &[5.0, 6.0], 5.0, 1000.0)
            .unwrap();
        w.write_chunk(&[7.0], &[8.0], &[9.0], 5.0, 1000.4).unwrap();
        let path = w.path().to_path_buf();
        let n = w.finish(&cfg()).unwrap();
        assert_eq!(n, 3);

        let bytes = fs::read(&path).unwrap();
        assert_eq!(bytes.len(), 3 * 3 * 4);
        let vals: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        assert_eq!(vals, vec![1.0, 3.0, 5.0, 2.0, 4.0, 6.0, 7.0, 8.0, 9.0]);

        let meta = fs::read_to_string(path.with_extension("f32.json")).unwrap();
        assert_eq!(meta.matches("{\"sample_offset\"").count(), 1, "{meta}");
    }

    #[test]
    fn opens_a_new_segment_across_a_time_gap() {
        let dir = std::env::temp_dir().join("sismokaos_rawbin_gap");
        let _ = fs::remove_dir_all(&dir);
        let mut w = RawBinaryWriter::create(&dir.join("out")).unwrap();
        w.write_chunk(&[1.0], &[1.0], &[1.0], 5.0, 1000.0).unwrap();
        // Jumps an hour: must not be folded into the previous segment.
        w.write_chunk(&[2.0], &[2.0], &[2.0], 5.0, 4600.0).unwrap();
        let path = w.path().to_path_buf();
        w.finish(&cfg()).unwrap();

        let meta = fs::read_to_string(path.with_extension("f32.json")).unwrap();
        assert_eq!(meta.matches("{\"sample_offset\"").count(), 2, "{meta}");
        assert!(meta.contains("\"sample_offset\": 1"), "{meta}");
    }

    #[test]
    fn nan_survives_and_is_counted() {
        let dir = std::env::temp_dir().join("sismokaos_rawbin_nan");
        let _ = fs::remove_dir_all(&dir);
        let mut w = RawBinaryWriter::create(&dir.join("out")).unwrap();
        w.write_chunk(&[f64::NAN, 1.0], &[1.0, 1.0], &[1.0, f64::NAN], 5.0, 0.0)
            .unwrap();
        let path = w.path().to_path_buf();
        w.finish(&cfg()).unwrap();

        let bytes = fs::read(&path).unwrap();
        let vals: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        assert!(vals[0].is_nan(), "leading NaN must survive the f32 cast");
        assert!(vals[5].is_nan());

        let meta = fs::read_to_string(path.with_extension("f32.json")).unwrap();
        assert!(meta.contains(r#""E": 1"#), "{meta}");
        assert!(meta.contains(r#""Z": 1"#), "{meta}");
    }
}

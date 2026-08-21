use butterworth::{Cutoff, Filter};
use mseed::{MSControlFlags, MSRecord, MSSampleType, detect};
use rayon::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::config::AppConfig;

const BLOCK_SECONDS: f64 = 3600.0;
const WATERMARK_SECONDS: f64 = 60.0;
const FILTER_CONTEXT_SECONDS: f64 = 120.0;

thread_local! {
    static SCRATCH_BUF: RefCell<Vec<f64>> = RefCell::new(Vec::with_capacity(3600 * 250));
}

pub struct ChannelChunk {
    pub e: Vec<f64>,
    pub n: Vec<f64>,
    pub z: Vec<f64>,
    /// Per-sample flags aligned 1:1 with `e`/`n`/`z`, marking samples the gap
    /// interpolant fabricated.
    ///
    /// `interpolate_gaps` fills interior gaps and leaves only *edge* gaps as
    /// NaN, so a reconstructed sample is indistinguishable from a recorded one
    /// downstream -- and a flat interpolated segment genuinely is
    /// low-dimensional, so the chaotic estimators return small, confident,
    /// entirely fictional values for it. Carrying the mask alongside the data
    /// is what makes those windows filterable after the fact.
    pub e_interp: Vec<bool>,
    pub n_interp: Vec<bool>,
    pub z_interp: Vec<bool>,
    pub fs: f64,
    pub start_epoch: f64,
}

const COMPONENTS: [char; 3] = ['E', 'N', 'Z'];

struct Assembler {
    start_epoch: f64,
    fs: f64,
    comps: HashMap<char, Vec<f64>>,
    watermark: f64,
    context: Vec<HashMap<char, Vec<f64>>>,
    global_offset: usize,
    dropped_late_records: usize,
}

struct RecordRef {
    file: u32,
    offset: u64,
    len: u32,
    start_epoch: f64,
    component: char,
    count: u32,
}

pub fn preprocess_directory_chunked(
    data_dir: &Path,
    config: &AppConfig,
    mut on_chunk: impl FnMut(ChannelChunk) -> Result<(), String>,
) -> Result<(), String> {
    let mut files: Vec<PathBuf> = fs::read_dir(data_dir)
        .map_err(|e| format!("Failed to read data directory {:?}: {}", data_dir, e))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|p| p.is_file())
        .collect();
    files.sort();

    if files.is_empty() {
        return Err(format!("No files found in data directory: {:?}", data_dir));
    }

    let (mut index, native_fs) = build_index(&files)?;
    index.sort_by(|a, b| a.start_epoch.total_cmp(&b.start_epoch));

    let mut asm = Assembler {
        start_epoch: f64::NAN,
        fs: native_fs,
        comps: COMPONENTS.into_iter().map(|c| (c, Vec::new())).collect(),
        watermark: f64::NEG_INFINITY,
        context: Vec::new(),
        global_offset: 0,
        dropped_late_records: 0,
    };

    if let Some(warning) = config.check_decimation(native_fs) {
        eprintln!("[WARNING] {}", warning);
    }

    let decimation_factor = ((native_fs / config.fs) as usize).max(1);
    let out_fs = native_fs / decimation_factor as f64;
    let block_len = (BLOCK_SECONDS * native_fs) as usize;

    let mut file_cache: Vec<(u32, File)> = Vec::with_capacity(8);
    let mut raw = Vec::new();

    for r in &index {
        let file_path = &files[r.file as usize];
        raw.resize(r.len as usize, 0u8);

        if let Some(pos) = file_cache.iter().position(|(i, _)| *i == r.file) {
            let entry = file_cache.remove(pos);
            file_cache.push(entry);
        } else {
            if file_cache.len() == 8 {
                file_cache.remove(0);
            }
            let f = File::open(file_path)
                .map_err(|e| format!("Failed to open {:?}: {}", file_path, e))?;
            file_cache.push((r.file, f));
        }

        let handle = &mut file_cache.last_mut().unwrap().1;
        handle
            .seek(SeekFrom::Start(r.offset))
            .map_err(|e| format!("Failed to seek in {:?}: {}", file_path, e))?;
        handle
            .read_exact(&mut raw)
            .map_err(|e| format!("Failed to read record from {:?}: {}", file_path, e))?;

        let rec = MSRecord::parse(&raw, MSControlFlags::MSF_UNPACKDATA)
            .map_err(|e| format!("Failed to unpack record in {:?}: {}", file_path, e))?;

        let count = (rec.num_samples() as usize).min(r.count as usize);
        if count == 0 {
            continue;
        }

        if asm.start_epoch.is_nan() {
            asm.start_epoch = r.start_epoch;
        }

        if (r.start_epoch - asm.start_epoch) > (2.0 * BLOCK_SECONDS) {
            while asm.comps.values().any(|v| !v.is_empty()) {
                flush_block(
                    &mut asm,
                    block_len,
                    config,
                    decimation_factor,
                    out_fs,
                    &mut on_chunk,
                )?;
            }
            asm.start_epoch = r.start_epoch;
            asm.watermark = f64::NEG_INFINITY;
            asm.context.clear();
        }

        let idx = ((r.start_epoch - asm.start_epoch) * native_fs).round() as i64;
        if idx < 0 {
            asm.dropped_late_records += 1;
            continue;
        }
        let idx = idx as usize;

        let dest = asm.comps.get_mut(&r.component).unwrap();
        if dest.len() < idx + count {
            dest.resize(idx + count, f64::NAN);
        }

        copy_record_samples(&rec, count, &mut dest[idx..idx + count], file_path)?;

        asm.watermark = asm
            .watermark
            .max(r.start_epoch + (count as f64 - 1.0) / native_fs);

        while asm.watermark >= asm.start_epoch + BLOCK_SECONDS + WATERMARK_SECONDS {
            flush_block(
                &mut asm,
                block_len,
                config,
                decimation_factor,
                out_fs,
                &mut on_chunk,
            )?;
        }
    }

    while asm.comps.values().any(|v| !v.is_empty()) {
        flush_block(
            &mut asm,
            block_len,
            config,
            decimation_factor,
            out_fs,
            &mut on_chunk,
        )?;
    }

    if asm.dropped_late_records > 0 {
        eprintln!(
            "[WARNING] Skipped {} record(s) that arrived after their block had been emitted; \
             input may not be in time order.",
            asm.dropped_late_records
        );
    }

    Ok(())
}

fn flush_block(
    asm: &mut Assembler,
    block_len: usize,
    config: &AppConfig,
    decimation_factor: usize,
    out_fs: f64,
    on_chunk: &mut impl FnMut(ChannelChunk) -> Result<(), String>,
) -> Result<(), String> {
    let available = asm.comps.values().map(|v| v.len()).max().unwrap_or(0);
    if available == 0 {
        asm.comps.values_mut().for_each(|v| v.clear());
        return Ok(());
    }
    let take = block_len.min(available);

    for v in asm.comps.values_mut() {
        if v.len() < take {
            v.resize(take, f64::NAN);
        }
    }

    let lead = asm
        .context
        .first()
        .map_or(0, |c| c.values().map(|v| v.len()).min().unwrap_or(0));

    let context_len = ((FILTER_CONTEXT_SECONDS * asm.fs) as usize).min(take);
    let mut next_context = HashMap::new();
    for comp in COMPONENTS {
        let v = asm.comps.get_mut(&comp).unwrap();
        next_context.insert(comp, v[take - context_len..take].to_vec());
    }

    let phase = (decimation_factor - (asm.global_offset % decimation_factor)) % decimation_factor;

    let processed_results: Result<Vec<(char, (Vec<f64>, Vec<bool>))>, String> = COMPONENTS
        .par_iter()
        .map(|&comp| {
            SCRATCH_BUF.with(|cell| {
                let mut combined = cell.borrow_mut();
                combined.clear();

                if lead > 0 {
                    let ctx = &asm.context[0][&comp];
                    combined.extend_from_slice(&ctx[ctx.len() - lead..]);
                }

                let v = &asm.comps[&comp];
                combined.extend_from_slice(&v[..take]);

                let processed = process_component(
                    &mut *combined,
                    config.freqmin,
                    config.freqmax,
                    asm.fs,
                    decimation_factor,
                    lead + phase,
                )?;
                Ok((comp, processed))
            })
        })
        .collect();

    let mut out: HashMap<char, (Vec<f64>, Vec<bool>)> = processed_results?.into_iter().collect();
    let (e, e_interp) = out.remove(&'E').unwrap();
    let (n, n_interp) = out.remove(&'N').unwrap();
    let (z, z_interp) = out.remove(&'Z').unwrap();

    on_chunk(ChannelChunk {
        e,
        n,
        z,
        e_interp,
        n_interp,
        z_interp,
        fs: out_fs,
        start_epoch: asm.start_epoch,
    })?;

    for v in asm.comps.values_mut() {
        v.drain(0..take);
    }
    asm.start_epoch += take as f64 / asm.fs;
    asm.global_offset += take;
    asm.context.clear();
    asm.context.push(next_context);

    Ok(())
}

fn build_index(files: &[PathBuf]) -> Result<(Vec<RecordRef>, f64), String> {
    let thread_results: Result<Vec<(Vec<RecordRef>, Option<f64>, Vec<char>)>, String> = files
        .par_iter()
        .enumerate()
        .map(|(file_idx, path)| {
            let mut local_index = Vec::new();
            let mut local_fs: Option<f64> = None;
            let mut local_seen = Vec::new();

            let mut handle =
                File::open(path).map_err(|e| format!("Failed to open {:?}: {}", path, e))?;
            let file_len = handle
                .metadata()
                .map_err(|e| format!("Failed to stat {:?}: {}", path, e))?
                .len();

            let mut pos: u64 = 0;
            let mut header = [0u8; 128];
            let mut raw: Vec<u8> = Vec::new();

            while pos + 64 <= file_len {
                let want = 128.min((file_len - pos) as usize);
                handle
                    .seek(SeekFrom::Start(pos))
                    .map_err(|e| format!("Failed to seek in {:?}: {}", path, e))?;
                handle
                    .read_exact(&mut header[..want])
                    .map_err(|e| format!("Failed to read header in {:?}: {}", path, e))?;

                let detection = detect(&header[..want])
                    .map_err(|e| format!("Not miniSEED at byte {} of {:?}: {}", pos, path, e))?;
                let rec_len = detection.rec_len.ok_or_else(|| {
                    format!(
                        "Indeterminate record length at byte {} of {:?}; cannot index this file",
                        pos, path
                    )
                })? as usize;

                raw.resize(rec_len, 0u8);
                handle
                    .seek(SeekFrom::Start(pos))
                    .map_err(|e| format!("Failed to seek in {:?}: {}", path, e))?;
                handle
                    .read_exact(&mut raw)
                    .map_err(|e| format!("Failed to read record in {:?}: {}", path, e))?;

                let rec = MSRecord::parse(&raw, MSControlFlags::empty())
                    .map_err(|e| format!("Failed to parse record in {:?}: {}", path, e))?;

                let sid = rec.sid_lossy();
                let rate = rec.sample_rate_hz();
                if let Some(component) = component_of(&sid) {
                    if rate > 0.0 {
                        if !local_seen.contains(&component) {
                            local_seen.push(component);
                        }
                        match local_fs {
                            None => local_fs = Some(rate),
                            Some(existing) if (existing - rate).abs() > 1e-6 => {
                                return Err(format!(
                                    "Mixed sample rates in {:?} ({} and {} Hz); not supported",
                                    path, existing, rate
                                ));
                            }
                            _ => {}
                        }

                        local_index.push(RecordRef {
                            file: file_idx as u32,
                            offset: pos,
                            len: rec_len as u32,
                            start_epoch: to_epoch_seconds(
                                rec.start_time().map_err(|e| e.to_string())?,
                            ),
                            component,
                            count: rec.sample_cnt() as u32,
                        });
                    }
                }
                pos += rec_len as u64;
            }
            Ok((local_index, local_fs, local_seen))
        })
        .collect();

    let mut index = Vec::new();
    let mut fs = None;
    let mut seen = Vec::new();

    for (local_index, local_fs, local_seen) in thread_results? {
        index.extend(local_index);
        for c in local_seen {
            if !seen.contains(&c) {
                seen.push(c);
            }
        }
        if let Some(rate) = local_fs {
            match fs {
                None => fs = Some(rate),
                Some(existing) if (existing - rate).abs() > 1e-6 => {
                    return Err(format!(
                        "Mixed sample rates across files ({} and {} Hz); not supported",
                        existing, rate
                    ));
                }
                _ => {}
            }
        }
    }

    let fs = fs.ok_or_else(|| "No E/N/Z component data found in the input files".to_string())?;

    for comp in COMPONENTS {
        if !seen.contains(&comp) {
            return Err(format!(
                "Component '{}' not found in the input files; need all of E, N, Z",
                comp
            ));
        }
    }

    Ok((index, fs))
}

fn copy_record_samples(
    rec: &mseed::MSRecord,
    count: usize,
    dest: &mut [f64],
    file: &Path,
) -> Result<(), String> {
    match rec.sample_type() {
        MSSampleType::Integer32 => {
            if let Some(s) = rec.data_samples::<i32>() {
                let n = count.min(s.len()).min(dest.len());
                for i in 0..n {
                    dest[i] = s[i] as f64;
                }
            }
        }
        MSSampleType::Float32 => {
            if let Some(s) = rec.data_samples::<f32>() {
                let n = count.min(s.len()).min(dest.len());
                for i in 0..n {
                    dest[i] = s[i] as f64;
                }
            }
        }
        MSSampleType::Float64 => {
            if let Some(s) = rec.data_samples::<f64>() {
                let n = count.min(s.len()).min(dest.len());
                dest[..n].copy_from_slice(&s[..n]);
            }
        }
        other => {
            return Err(format!("Unsupported sample type {:?} in {:?}", other, file));
        }
    }
    Ok(())
}

fn component_of(sid: &str) -> Option<char> {
    let last = sid.rsplit(['_', '.']).next()?;
    match last.chars().next()?.to_ascii_uppercase() {
        'Z' => Some('Z'),
        'N' | '2' => Some('N'),
        'E' | '1' => Some('E'),
        _ => None,
    }
}

fn to_epoch_seconds(t: time::OffsetDateTime) -> f64 {
    t.unix_timestamp() as f64 + t.nanosecond() as f64 / 1e9
}

fn interpolate_gaps(data: &mut [f64]) {
    let n = data.len();
    if n == 0 || !data.iter().any(|v| v.is_nan()) {
        return; // Fast path: Bypass entirely if no gaps
    }

    let mut i = 0;
    while i < n {
        if !data[i].is_nan() {
            i += 1;
            continue;
        }
        let gap_start = i;
        while i < n && data[i].is_nan() {
            i += 1;
        }
        let gap_end = i;
        if gap_start == 0 || gap_end == n {
            continue;
        }

        let left = gap_start - 1;
        let right = gap_end;
        let y0 = data[left];
        let y1 = data[right];
        let span = (right - left) as f64;

        // Catmull-Rom tangents, as a slope per SAMPLE scaled into the Hermite
        // parameter t in [0, 1] (hence the `* span`).
        //
        // The divisor must be the real index distance between the two samples
        // being differenced, not a constant. `left - 1` and `right` are
        // `span + 1` samples apart; dividing by 2 is only correct for a
        // single-sample gap. With the constant, tangents were overestimated by
        // (span + 1) / 2 -- a factor of 50 across a 100-sample gap -- and the
        // interpolant overshot wildly: measured on a sine of amplitude 100, a
        // 100-sample gap filled to a peak of 894, worse than plain linear
        // interpolation for any gap beyond ~10 samples. Those spikes then
        // passed through the bandpass into the features.
        let m0 = if left > 0 {
            ((y1 - data[left - 1]) / (span + 1.0)) * span
        } else {
            y1 - y0
        };
        let m1 = if right + 1 < n {
            ((data[right + 1] - y0) / (span + 1.0)) * span
        } else {
            y1 - y0
        };

        for k in gap_start..gap_end {
            let t = (k - left) as f64 / span;
            let t2 = t * t;
            let t3 = t2 * t;
            let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
            let h10 = t3 - 2.0 * t2 + t;
            let h01 = -2.0 * t3 + 3.0 * t2;
            let h11 = t3 - t2;
            data[k] = h00 * y0 + h10 * m0 + h01 * y1 + h11 * m1;
        }
    }
}

fn detrend_linear(data: &mut [f64]) {
    let n = data.len();
    if n < 2 {
        return;
    }

    let has_nans = data.iter().any(|v| v.is_nan());

    if !has_nans {
        // SIMD-friendly fast path (no branches)
        let count = n as f64;
        let sum_y: f64 = data.iter().sum();
        let mean_y = sum_y / count;

        let sum_x = (n * (n - 1)) as f64 / 2.0;
        let mean_x = sum_x / count;

        let mut sum_dx_dy = 0.0;
        let mut sum_dx2 = 0.0;

        for (i, &y) in data.iter().enumerate() {
            let dx = i as f64 - mean_x;
            sum_dx_dy += dx * (y - mean_y);
            sum_dx2 += dx * dx;
        }

        let slope = if sum_dx2.abs() < f64::EPSILON {
            0.0
        } else {
            sum_dx_dy / sum_dx2
        };
        let intercept = mean_y - slope * mean_x;

        for (i, y) in data.iter_mut().enumerate() {
            *y -= intercept + slope * i as f64;
        }
    } else {
        // Fallback for data with gaps (NaNs)
        let mut sum_y = 0.0;
        let mut count: f64 = 0.0;
        for &y in data.iter() {
            if !y.is_nan() {
                sum_y += y;
                count += 1.0;
            }
        }

        if count < 2.0 {
            let mean = sum_y / count.max(1.0);
            for y in data.iter_mut().filter(|y| !y.is_nan()) {
                *y -= mean;
            }
            return;
        }

        let mean_y = sum_y / count;

        let mut sum_x = 0.0;
        for (i, &y) in data.iter().enumerate() {
            if !y.is_nan() {
                sum_x += i as f64;
            }
        }
        let mean_x = sum_x / count;

        let mut sum_dx_dy = 0.0;
        let mut sum_dx2 = 0.0;

        for (i, &y) in data.iter().enumerate() {
            if !y.is_nan() {
                let dx = i as f64 - mean_x;
                sum_dx_dy += dx * (y - mean_y);
                sum_dx2 += dx * dx;
            }
        }

        let slope = if sum_dx2.abs() < f64::EPSILON {
            0.0
        } else {
            sum_dx_dy / sum_dx2
        };
        let intercept = mean_y - slope * mean_x;

        for (i, y) in data.iter_mut().enumerate() {
            if !y.is_nan() {
                *y -= intercept + slope * i as f64;
            }
        }
    }
}

/// Returns the processed samples together with a per-sample flag marking which
/// of them the gap interpolant fabricated. Both are decimated identically, so
/// they stay index-aligned.
fn process_component(
    data: &mut Vec<f64>,
    freqmin: f64,
    freqmax: f64,
    fs: f64,
    decimation_factor: usize,
    skip: usize,
) -> Result<(Vec<f64>, Vec<bool>), String> {
    if data.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }

    // Captured between the interpolant and everything downstream, because this
    // is the only point where "was NaN, now is not" means exactly "fabricated
    // by the interpolant". Edge gaps are still NaN here and stay NaN in the
    // output -- they are already visible, so they are not counted as
    // interpolated.
    let had_nan = data.iter().any(|v| v.is_nan());
    let was_nan: Vec<bool> = if had_nan {
        data.iter().map(|v| v.is_nan()).collect()
    } else {
        Vec::new()
    };
    interpolate_gaps(data);
    let interpolated: Vec<bool> = if had_nan {
        was_nan
            .iter()
            .zip(data.iter())
            .map(|(&w, v)| w && !v.is_nan())
            .collect()
    } else {
        vec![false; data.len()]
    };

    detrend_linear(data);

    let mut missing = Vec::new();
    for (i, v) in data.iter_mut().enumerate() {
        if v.is_nan() {
            missing.push(i);
            *v = 0.0;
        }
    }

    if missing.len() == data.len() {
        let kept = data.len().saturating_sub(skip).div_ceil(decimation_factor);
        return Ok((vec![f64::NAN; kept], vec![false; kept]));
    }

    let filter =
        Filter::new(4, fs, Cutoff::BandPass(freqmin, freqmax)).map_err(|e| e.to_string())?;
    let mut filtered = filter.bidirectional(data).map_err(|e| e.to_string())?;

    for i in missing {
        filtered[i] = f64::NAN;
    }

    // A mask shorter than its data would not panic -- `engine.rs` buffers the
    // two independently -- it would silently desynchronise and mis-attribute
    // every window after the first short chunk. Assert instead.
    assert_eq!(
        filtered.len(),
        interpolated.len(),
        "interpolation mask desynchronised from filtered samples"
    );
    let skip = skip.min(filtered.len());
    let out: Vec<f64> = filtered[skip..]
        .iter()
        .step_by(decimation_factor)
        .copied()
        .collect();
    let mask: Vec<bool> = interpolated[skip..]
        .iter()
        .step_by(decimation_factor)
        .copied()
        .collect();
    assert_eq!(out.len(), mask.len());
    Ok((out, mask))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(n: usize, amp: f64) -> Vec<f64> {
        (0..n)
            .map(|i| (i as f64 / n as f64 * 4.0 * std::f64::consts::PI).sin() * amp)
            .collect()
    }

    /// A window whose samples were fabricated by the gap interpolant has to be
    /// identifiable afterwards. Without the mask a flat reconstructed segment
    /// reads as a confident low-dimensional chaos measurement, and nothing in
    /// the feature table distinguishes it from real quiet ground.
    #[test]
    fn interior_gaps_are_reported_as_interpolated() {
        let mut data = sine(1000, 100.0);
        for v in data.iter_mut().take(320).skip(300) {
            *v = f64::NAN;
        }

        let (out, mask) = process_component(&mut data, 1.0, 20.0, 100.0, 1, 0).unwrap();

        assert_eq!(out.len(), mask.len());
        assert!(
            mask[300..320].iter().all(|&b| b),
            "every filled sample should be flagged"
        );
        assert_eq!(
            mask.iter().filter(|&&b| b).count(),
            20,
            "nothing outside the gap should be flagged"
        );
    }

    /// Edge gaps survive as NaN in the output, so they are already visible and
    /// must not be double-counted as fabricated data.
    #[test]
    fn edge_gaps_are_not_counted_as_interpolated() {
        let mut data = sine(1000, 100.0);
        for v in data.iter_mut().take(5) {
            *v = f64::NAN;
        }
        for v in data.iter_mut().skip(995) {
            *v = f64::NAN;
        }

        let (out, mask) = process_component(&mut data, 1.0, 20.0, 100.0, 1, 0).unwrap();

        assert!(mask.iter().all(|&b| !b), "edge gaps are not interpolated");
        assert!(out[..5].iter().all(|v| v.is_nan()), "edge gaps stay NaN");
    }

    /// A clean trace must report exactly zero, not an empty or absent mask.
    #[test]
    fn clean_data_reports_no_interpolation() {
        let mut data = sine(1000, 100.0);
        let (out, mask) = process_component(&mut data, 1.0, 20.0, 100.0, 1, 0).unwrap();
        assert_eq!(out.len(), mask.len());
        assert!(mask.iter().all(|&b| !b));
    }

    /// The mask is decimated by the same slice-and-stride as the samples. If it
    /// were not, it would stay the same length as the raw input and silently
    /// describe the wrong samples from the first chunk onward.
    #[test]
    fn mask_survives_decimation_in_alignment() {
        for (factor, skip) in [(1usize, 0usize), (5, 0), (5, 7), (20, 13)] {
            let mut data = sine(1000, 100.0);
            for v in data.iter_mut().take(520).skip(500) {
                *v = f64::NAN;
            }
            let (out, mask) = process_component(&mut data, 1.0, 20.0, 100.0, factor, skip).unwrap();

            assert_eq!(out.len(), mask.len(), "factor={factor} skip={skip}");
            // Every flagged output index must map back into the original gap.
            for (i, &flagged) in mask.iter().enumerate() {
                let src = skip + i * factor;
                assert_eq!(
                    flagged,
                    (500..520).contains(&src),
                    "factor={factor} skip={skip} out_idx={i} src={src}"
                );
            }
        }
    }

    /// The interpolant must not invent amplitude that is not in the signal.
    ///
    /// This is the regression test for the Catmull-Rom tangent scaling: with a
    /// constant divisor of 2.0 a 100-sample gap in a sine of amplitude 100
    /// filled to a peak of ~894.
    #[test]
    fn interpolated_gaps_do_not_overshoot() {
        for gap in [1usize, 2, 5, 20, 100] {
            let clean = sine(400, 100.0);
            let mut data = clean.clone();
            let start = 150;
            for v in data[start..start + gap].iter_mut() {
                *v = f64::NAN;
            }
            interpolate_gaps(&mut data);

            let peak = data[start..start + gap]
                .iter()
                .fold(0.0f64, |m, v| m.max(v.abs()));
            assert!(
                peak <= 110.0,
                "gap of {gap} samples filled to peak {peak:.1}, signal peak is 100"
            );

            // And it should be no worse than linear interpolation across the gap.
            let (y0, y1) = (clean[start - 1], clean[start + gap]);
            let lin_err = (0..gap)
                .map(|k| {
                    let t = (k + 1) as f64 / (gap + 1) as f64;
                    (y0 + (y1 - y0) * t - clean[start + k]).abs()
                })
                .fold(0.0f64, f64::max);
            let err = (0..gap)
                .map(|k| (data[start + k] - clean[start + k]).abs())
                .fold(0.0f64, f64::max);
            assert!(
                err <= lin_err * 1.5 + 1e-6,
                "gap {gap}: hermite error {err:.2} vs linear {lin_err:.2}"
            );
        }
    }

    /// A single-sample gap is the one case the old constant divisor got right;
    /// pin it so the fix cannot regress the easy case.
    #[test]
    fn single_sample_gap_is_near_exact() {
        let clean = sine(400, 100.0);
        let mut data = clean.clone();
        data[200] = f64::NAN;
        interpolate_gaps(&mut data);
        assert!((data[200] - clean[200]).abs() < 0.1);
    }

    /// Leading and trailing gaps have no bracketing sample, so they must stay
    /// NaN rather than be silently extrapolated.
    #[test]
    fn edge_gaps_are_left_as_nan() {
        let mut data = sine(100, 10.0);
        data[0] = f64::NAN;
        data[1] = f64::NAN;
        data[99] = f64::NAN;
        interpolate_gaps(&mut data);
        assert!(data[0].is_nan() && data[1].is_nan() && data[99].is_nan());
        assert!(data[50].is_finite());
    }

    #[test]
    fn detrend_removes_a_linear_ramp() {
        let mut data: Vec<f64> = (0..1000).map(|i| 5.0 + 0.25 * i as f64).collect();
        detrend_linear(&mut data);
        assert!(data.iter().all(|v| v.abs() < 1e-6));
    }

    /// Detrend must ignore NaNs rather than propagate them into the fit.
    #[test]
    fn detrend_tolerates_gaps() {
        let mut data: Vec<f64> = (0..1000).map(|i| 5.0 + 0.25 * i as f64).collect();
        data[10] = f64::NAN;
        data[500] = f64::NAN;
        detrend_linear(&mut data);
        assert!(data[10].is_nan() && data[500].is_nan());
        assert!(data.iter().filter(|v| v.is_finite()).all(|v| v.abs() < 1e-6));
    }
}

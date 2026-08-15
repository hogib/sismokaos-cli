use butterworth::{Cutoff, Filter};
use mseed::{detect, MSControlFlags, MSRecord, MSSampleType};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::config::AppConfig;

/// Duration of signal processed (and emitted) at a time. Chosen to keep working memory small and
/// independent of both file size and directory size; a whole miniSEED file is never held at once.
const BLOCK_SECONDS: f64 = 3600.0;

/// How far past a block's end the input stream must advance before that block is considered
/// complete. Records for the three components are interleaved in a file and can be slightly out
/// of order, so a block is only flushed once data comfortably beyond it has been seen.
const WATERMARK_SECONDS: f64 = 60.0;

/// Seconds of preceding signal fed into the bandpass filter ahead of each block, so the zero-phase
/// filter has settled by the time it reaches samples that are actually emitted.
const FILTER_CONTEXT_SECONDS: f64 = 120.0;

/// A gap-filled, filtered and decimated block of E/N/Z channel data. Blocks arrive in time order
/// and join seamlessly: decimation runs on a single global sample grid across the whole run.
pub struct ChannelChunk {
    pub e: Vec<f64>,
    pub n: Vec<f64>,
    pub z: Vec<f64>,
    pub fs: f64,
}

const COMPONENTS: [char; 3] = ['E', 'N', 'Z'];

/// Rolling assembly buffer holding the not-yet-emitted span of the signal.
struct Assembler {
    /// Epoch of index 0 of every component buffer.
    start_epoch: f64,
    fs: f64,
    comps: HashMap<char, Vec<f64>>,
    /// Latest sample time seen anywhere in the input, used to decide when a block is complete.
    watermark: f64,
    /// Native-rate samples immediately preceding `start_epoch`, to prime the filter.
    context: Vec<HashMap<char, Vec<f64>>>,
    /// Absolute native-sample index of `start_epoch` in the global stream, so decimation stays on
    /// one grid across block boundaries.
    global_offset: usize,
    dropped_late_records: usize,
}

/// Location and header facts for a single miniSEED record, so it can be re-read on demand.
struct RecordRef {
    file: u32,
    offset: u64,
    len: u32,
    start_epoch: f64,
    component: char,
    count: u32,
}

/// Reads every miniSEED file in `data_dir` and invokes `on_chunk` with successive fixed-duration
/// blocks of demeaned, detrended, bandpass-filtered and decimated E/N/Z samples.
///
/// Memory is bounded by `BLOCK_SECONDS` regardless of how large any individual file is or how
/// many files there are: neither a directory of many files nor a single enormous file is ever
/// held in memory whole.
///
/// A first header-only pass indexes every record's location and start time; the index is then
/// sorted so records can be replayed in true time order, one at a time, into a rolling buffer
/// that is flushed and drained every block. The sort matters because miniSEED files are commonly
/// written channel-sequentially (every E record, then every N, then every Z) rather than
/// interleaved by time — reading such a file front to back would otherwise finish an entire
/// component before the other two started.
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

    let decimation_factor = ((native_fs / config.fs) as usize).max(1);
    let out_fs = native_fs / decimation_factor as f64;
    let block_len = (BLOCK_SECONDS * native_fs) as usize;

    let mut handles: Vec<File> = Vec::with_capacity(files.len());
    for f in &files {
        handles.push(File::open(f).map_err(|e| format!("Failed to open {:?}: {}", f, e))?);
    }

    let mut raw = Vec::new();
    for r in &index {
        let file = &files[r.file as usize];
        raw.resize(r.len as usize, 0u8);
        let handle = &mut handles[r.file as usize];
        handle
            .seek(SeekFrom::Start(r.offset))
            .map_err(|e| format!("Failed to seek in {:?}: {}", file, e))?;
        handle
            .read_exact(&mut raw)
            .map_err(|e| format!("Failed to read record from {:?}: {}", file, e))?;

        let rec = MSRecord::parse(&raw, MSControlFlags::MSF_UNPACKDATA)
            .map_err(|e| format!("Failed to unpack record in {:?}: {}", file, e))?;

        let count = (rec.num_samples() as usize).min(r.count as usize);
        if count == 0 {
            continue;
        }

        if asm.start_epoch.is_nan() {
            asm.start_epoch = r.start_epoch;
        }

        let idx = ((r.start_epoch - asm.start_epoch) * native_fs).round() as i64;
        if idx < 0 {
            // Belongs to a block that has already been emitted; nothing sensible to do with it
            // now, so count it and carry on rather than corrupting the current block.
            asm.dropped_late_records += 1;
            continue;
        }
        let idx = idx as usize;

        let dest = asm.comps.get_mut(&r.component).unwrap();
        if dest.len() < idx + count {
            dest.resize(idx + count, f64::NAN);
        }
        copy_record_samples(&rec, count, &mut dest[idx..idx + count], file)?;

        asm.watermark = asm
            .watermark
            .max(r.start_epoch + (count as f64 - 1.0) / native_fs);

        // Emit every block that the input has now moved safely past.
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

    // Drain whatever is left once the input is exhausted.
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

/// Emits one block from the front of the assembler and drains it from the buffers.
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

    // Square off the block: a component with no data this far along is short, so pad it out and
    // let the NaNs mark it as missing rather than silently shifting samples.
    for v in asm.comps.values_mut() {
        if v.len() < take {
            v.resize(take, f64::NAN);
        }
    }

    let lead = asm.context.first().map_or(0, |c| {
        c.values().map(|v| v.len()).min().unwrap_or(0)
    });

    // Keep the tail of this block (native rate) to prime the next block's filter.
    let context_len = ((FILTER_CONTEXT_SECONDS * asm.fs) as usize).min(take);
    let mut next_context = HashMap::new();
    for comp in COMPONENTS {
        let v = &asm.comps[&comp];
        next_context.insert(comp, v[take - context_len..take].to_vec());
    }

    // Decimation runs on one global grid, so the first kept sample of this block is whichever
    // offset lands back on that grid.
    let phase = (decimation_factor - (asm.global_offset % decimation_factor)) % decimation_factor;

    let mut out: HashMap<char, Vec<f64>> = HashMap::new();
    for comp in COMPONENTS {
        let mut combined = Vec::with_capacity(lead + take);
        if lead > 0 {
            let ctx = &asm.context[0][&comp];
            combined.extend_from_slice(&ctx[ctx.len() - lead..]);
        }
        combined.extend_from_slice(&asm.comps[&comp][..take]);

        out.insert(
            comp,
            process_component(
                combined,
                config.freqmin,
                config.freqmax,
                asm.fs,
                decimation_factor,
                lead + phase,
            )?,
        );
    }

    on_chunk(ChannelChunk {
        e: out.remove(&'E').unwrap(),
        n: out.remove(&'N').unwrap(),
        z: out.remove(&'Z').unwrap(),
        fs: out_fs,
    })?;

    // Advance past the emitted block.
    for v in asm.comps.values_mut() {
        v.drain(0..take);
    }
    asm.start_epoch += take as f64 / asm.fs;
    asm.global_offset += take;
    asm.context.clear();
    asm.context.push(next_context);

    Ok(())
}

/// Header-only pass over every file, recording where each record lives and what it covers.
///
/// Records are located by walking each file with `detect()` (which reports the record length),
/// then parsing the header without unpacking sample data, so this stays cheap and allocates only
/// the index itself (a few tens of bytes per record).
fn build_index(files: &[PathBuf]) -> Result<(Vec<RecordRef>, f64), String> {
    let mut index: Vec<RecordRef> = Vec::new();
    let mut fs: Option<f64> = None;
    let mut seen: Vec<char> = Vec::new();

    for (file_idx, path) in files.iter().enumerate() {
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
                    if !seen.contains(&component) {
                        seen.push(component);
                    }
                    match fs {
                        None => fs = Some(rate),
                        Some(existing) if (existing - rate).abs() > 1e-6 => {
                            return Err(format!(
                                "Mixed sample rates in {:?} ({} and {} Hz); not supported",
                                path, existing, rate
                            ));
                        }
                        _ => {}
                    }

                    index.push(RecordRef {
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

/// Copies one record's unpacked samples into `dest`, widening whatever integer/float encoding the
/// record uses to f64.
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
            return Err(format!(
                "Unsupported sample type {:?} in {:?}",
                other, file
            ));
        }
    }
    Ok(())
}

/// Maps an FDSN source identifier / NSLC channel code to its E/N/Z component.
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

/// Fills NaN gaps that are bounded by real data on both sides with a cubic Hermite
/// interpolation between the two boundary samples. Leading/trailing gaps (no boundary on one
/// side) are left as NaN, so genuinely missing data stays marked as missing.
fn interpolate_gaps(data: &mut [f64]) {
    let n = data.len();
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
        // m0/m1 are dy/dt tangents over the [0,1] gap parameter, so per-index slopes (central
        // differences) are scaled by `span` while the boundary-value slope (already dy/dt) is not.
        let m0 = if left > 0 {
            ((y1 - data[left - 1]) / 2.0) * span
        } else {
            y1 - y0
        };
        let m1 = if right + 1 < n {
            ((data[right + 1] - y0) / 2.0) * span
        } else {
            y1 - y0
        };

        for k in gap_start..gap_end {
            let t = (k - left) as f64 / span;
            data[k] = hermite(t, y0, y1, m0, m1);
        }
    }
}

fn hermite(t: f64, y0: f64, y1: f64, m0: f64, m1: f64) -> f64 {
    let t2 = t * t;
    let t3 = t2 * t;
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;
    h00 * y0 + h10 * m0 + h01 * y1 + h11 * m1
}

fn detrend_linear(data: &mut [f64]) {
    let n = data.len();
    if n == 0 {
        return;
    }

    let (mut sum_x, mut sum_y, mut sum_xx, mut sum_xy, mut count) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (i, &y) in data.iter().enumerate() {
        if y.is_nan() {
            continue;
        }
        let x = i as f64;
        sum_x += x;
        sum_y += y;
        sum_xx += x * x;
        sum_xy += x * y;
        count += 1.0;
    }

    if count == 0.0 {
        return;
    }

    let denom = count * sum_xx - sum_x * sum_x;
    if count < 2.0 || denom.abs() < f64::EPSILON {
        let mean = sum_y / count;
        for y in data.iter_mut() {
            *y -= mean;
        }
        return;
    }

    let slope = (count * sum_xy - sum_x * sum_y) / denom;
    let intercept = (sum_y - slope * sum_x) / count;
    for (i, y) in data.iter_mut().enumerate() {
        *y -= intercept + slope * i as f64;
    }
}

/// Detrends, bandpass-filters, drops the leading `skip` samples (filter context plus decimation
/// phase) and decimates.
///
/// Any sample still NaN after gap interpolation is genuinely missing data. A single NaN would
/// propagate through the IIR filter and destroy the whole component, so those positions are
/// zeroed for the filtering pass and restored to NaN afterwards, leaving downstream windowing to
/// skip the windows that overlap them.
fn process_component(
    mut data: Vec<f64>,
    freqmin: f64,
    freqmax: f64,
    fs: f64,
    decimation_factor: usize,
    skip: usize,
) -> Result<Vec<f64>, String> {
    if data.is_empty() {
        return Ok(data);
    }

    interpolate_gaps(&mut data);
    detrend_linear(&mut data);

    let missing: Vec<usize> = data
        .iter()
        .enumerate()
        .filter(|(_, v)| v.is_nan())
        .map(|(i, _)| i)
        .collect();

    if missing.len() == data.len() {
        // Nothing but gaps: skip the filter entirely and report the block as missing.
        let kept = data.len().saturating_sub(skip).div_ceil(decimation_factor);
        return Ok(vec![f64::NAN; kept]);
    }

    for &i in &missing {
        data[i] = 0.0;
    }

    let filter =
        Filter::new(4, fs, Cutoff::BandPass(freqmin, freqmax)).map_err(|e| e.to_string())?;
    let mut filtered = filter.bidirectional(&data).map_err(|e| e.to_string())?;
    drop(data);

    for &i in &missing {
        filtered[i] = f64::NAN;
    }

    let skip = skip.min(filtered.len());
    Ok(filtered[skip..]
        .iter()
        .step_by(decimation_factor)
        .copied()
        .collect())
}

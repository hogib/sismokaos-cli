use butterworth::{Cutoff, Filter};
use mseed::{MSControlFlags, MSReader, MSSampleType};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::AppConfig;

/// Seconds of preceding signal fed into the bandpass filter ahead of each file's own data, so the
/// zero-phase filter has settled by the time it reaches samples we actually keep. Generous
/// relative to the filter's transient (which is on the order of a few periods of `freqmin`) but
/// still negligible in memory next to a whole file.
const FILTER_CONTEXT_SECONDS: f64 = 120.0;

/// A gap-filled, filtered and decimated slice of E/N/Z channel data covering one file's worth of
/// the directory. Chunks arrive in time order.
pub struct ChannelChunk {
    pub e: Vec<f64>,
    pub n: Vec<f64>,
    pub z: Vec<f64>,
    pub fs: f64,
}

/// E/N/Z samples for one file, already intersected onto a common time base.
struct AlignedFile {
    fs: f64,
    start_epoch: f64,
    comps: HashMap<char, Vec<f64>>,
    len: usize,
}

/// Native-rate tail of the previous file, carried forward to prime the filter for the next one.
struct FilterContext {
    end_epoch: f64,
    comps: HashMap<char, Vec<f64>>,
}

/// Reads every miniSEED file in `data_dir` one at a time (in filename order) and, for each file,
/// invokes `on_chunk` with that file's demeaned, detrended, bandpass-filtered and decimated E/N/Z
/// samples.
///
/// Memory is bounded by a single file rather than by the size of the directory: files are never
/// concatenated, and the only state carried between them is a short native-rate tail
/// (`FILTER_CONTEXT_SECONDS`) used to let the zero-phase filter settle before the samples that
/// are actually emitted. Records are also read and released one at a time rather than being
/// accumulated into a libmseed trace list, whose `insert` copies each record without taking
/// ownership of it.
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

    let mut context: Option<FilterContext> = None;

    for file in &files {
        let mut aligned = read_aligned_file(file)?;
        let native_fs = aligned.fs;

        // Prepend the previous file's tail only if it runs contiguously into this file (within
        // half a sample); after a real gap there is nothing meaningful to settle from.
        let mut lead = 0usize;
        if let Some(ctx) = context.take() {
            let expected = aligned.start_epoch - 1.0 / native_fs;
            if (ctx.end_epoch - expected).abs() < 0.5 / native_fs {
                lead = ctx
                    .comps
                    .values()
                    .map(|v| v.len())
                    .min()
                    .unwrap_or(0);
                for comp in ['E', 'N', 'Z'] {
                    let tail = &ctx.comps[&comp][ctx.comps[&comp].len() - lead..];
                    let cur = aligned.comps.get_mut(&comp).unwrap();
                    let mut combined = Vec::with_capacity(lead + cur.len());
                    combined.extend_from_slice(tail);
                    combined.append(cur);
                    *cur = combined;
                }
            }
        }

        // Stash this file's tail for the next iteration before it gets consumed by filtering.
        let context_len = ((FILTER_CONTEXT_SECONDS * native_fs) as usize).min(aligned.len);
        let mut next_comps = HashMap::new();
        for comp in ['E', 'N', 'Z'] {
            let v = &aligned.comps[&comp];
            next_comps.insert(comp, v[v.len() - context_len..].to_vec());
        }
        context = Some(FilterContext {
            end_epoch: aligned.start_epoch + (aligned.len as f64 - 1.0) / native_fs,
            comps: next_comps,
        });

        let decimation_factor = ((native_fs / config.fs) as usize).max(1);
        let out_fs = native_fs / decimation_factor as f64;

        let e = process_component(
            aligned.comps.remove(&'E').unwrap(),
            config.freqmin,
            config.freqmax,
            native_fs,
            decimation_factor,
            lead,
        )?;
        let n = process_component(
            aligned.comps.remove(&'N').unwrap(),
            config.freqmin,
            config.freqmax,
            native_fs,
            decimation_factor,
            lead,
        )?;
        let z = process_component(
            aligned.comps.remove(&'Z').unwrap(),
            config.freqmin,
            config.freqmax,
            native_fs,
            decimation_factor,
            lead,
        )?;

        on_chunk(ChannelChunk { e, n, z, fs: out_fs })?;
    }

    Ok(())
}

/// Reads one file's E/N/Z components and intersects them onto a common, sample-aligned time base
/// (cross-channel features assume E/N/Z line up, and NaN-padding a component would poison it
/// through the filter).
fn read_aligned_file(file: &Path) -> Result<AlignedFile, String> {
    let spans = scan_component_spans(file)?;

    for comp in ['E', 'N', 'Z'] {
        if !spans.contains_key(&comp) {
            return Err(format!(
                "Component '{}' not found in {:?}; need all of E, N, Z",
                comp, file
            ));
        }
    }

    let native_fs = spans[&'E'].fs;
    let common_start = spans
        .values()
        .map(|s| s.start_epoch)
        .fold(f64::NEG_INFINITY, f64::max);
    let common_end = spans
        .values()
        .map(|s| s.end_epoch)
        .fold(f64::INFINITY, f64::min);

    if common_end <= common_start {
        return Err(format!(
            "E/N/Z components in {:?} do not overlap in time",
            file
        ));
    }

    let len = ((common_end - common_start) * native_fs).round() as usize + 1;
    let mut comps: HashMap<char, Vec<f64>> = ['E', 'N', 'Z']
        .into_iter()
        .map(|c| (c, vec![f64::NAN; len]))
        .collect();

    fill_component_samples(file, common_start, native_fs, &mut comps)?;

    for v in comps.values_mut() {
        interpolate_gaps(v);
    }

    Ok(AlignedFile {
        fs: native_fs,
        start_epoch: common_start,
        comps,
        len,
    })
}

struct ComponentSpan {
    fs: f64,
    start_epoch: f64,
    end_epoch: f64,
}

/// Header-only pass: learns each component's time span and sample rate without unpacking any
/// sample data, so the destination arrays can be sized exactly.
fn scan_component_spans(file: &Path) -> Result<HashMap<char, ComponentSpan>, String> {
    let mut spans: HashMap<char, ComponentSpan> = HashMap::new();

    let mut reader = MSReader::new_with_flags(file, MSControlFlags::empty())
        .map_err(|e| format!("Failed to open {:?}: {}", file, e))?;

    while let Some(rec) = reader.next() {
        let rec = rec.map_err(|e| format!("Failed to read record in {:?}: {}", file, e))?;
        let sid = rec.sid_lossy();
        let Some(component) = component_of(&sid) else {
            continue;
        };
        let fs = rec.sample_rate_hz();
        if fs <= 0.0 {
            continue;
        }
        let start = to_epoch_seconds(rec.start_time().map_err(|e| e.to_string())?);
        let end = start + (rec.sample_cnt() as f64 - 1.0) / fs;

        spans
            .entry(component)
            .and_modify(|s| {
                s.start_epoch = s.start_epoch.min(start);
                s.end_epoch = s.end_epoch.max(end);
            })
            .or_insert(ComponentSpan {
                fs,
                start_epoch: start,
                end_epoch: end,
            });
    }

    Ok(spans)
}

/// Data pass: unpacks each record in turn and scatters its samples into the destination arrays.
/// Records are dropped (and their sample buffers freed) as the iteration advances, so only one
/// record is held at a time on top of the destination arrays.
fn fill_component_samples(
    file: &Path,
    common_start: f64,
    fs: f64,
    comps: &mut HashMap<char, Vec<f64>>,
) -> Result<(), String> {
    let mut reader = MSReader::new_with_flags(file, MSControlFlags::MSF_UNPACKDATA)
        .map_err(|e| format!("Failed to open {:?}: {}", file, e))?;

    while let Some(rec) = reader.next() {
        let rec = rec.map_err(|e| format!("Failed to read record in {:?}: {}", file, e))?;
        let sid = rec.sid_lossy();
        let Some(component) = component_of(&sid) else {
            continue;
        };
        let Some(dest) = comps.get_mut(&component) else {
            continue;
        };

        let start = to_epoch_seconds(rec.start_time().map_err(|e| e.to_string())?);
        let offset = ((start - common_start) * fs).round() as i64;
        let count = rec.num_samples() as usize;

        let write = |dest: &mut Vec<f64>, i: usize, v: f64| {
            let idx = offset + i as i64;
            if idx >= 0 {
                if let Some(slot) = dest.get_mut(idx as usize) {
                    *slot = v;
                }
            }
        };

        match rec.sample_type() {
            MSSampleType::Integer32 => {
                if let Some(s) = rec.data_samples::<i32>() {
                    for (i, &v) in s[..count.min(s.len())].iter().enumerate() {
                        write(dest, i, v as f64);
                    }
                }
            }
            MSSampleType::Float32 => {
                if let Some(s) = rec.data_samples::<f32>() {
                    for (i, &v) in s[..count.min(s.len())].iter().enumerate() {
                        write(dest, i, v as f64);
                    }
                }
            }
            MSSampleType::Float64 => {
                if let Some(s) = rec.data_samples::<f64>() {
                    for (i, &v) in s[..count.min(s.len())].iter().enumerate() {
                        write(dest, i, v);
                    }
                }
            }
            other => {
                return Err(format!(
                    "Unsupported sample type {:?} in {:?}",
                    other, file
                ));
            }
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
/// side) are left as NaN, matching how downstream windowing already skips incomplete windows.
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

/// Detrends, bandpass-filters, drops the leading `skip` context samples and decimates.
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

    detrend_linear(&mut data);

    let filter =
        Filter::new(4, fs, Cutoff::BandPass(freqmin, freqmax)).map_err(|e| e.to_string())?;
    let filtered = filter.bidirectional(&data).map_err(|e| e.to_string())?;
    drop(data);

    let skip = skip.min(filtered.len());
    Ok(filtered[skip..]
        .iter()
        .step_by(decimation_factor)
        .copied()
        .collect())
}

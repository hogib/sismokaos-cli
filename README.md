# sismokaos-cli

Seismic signal preprocessing and feature extraction for three-component (E/N/Z) miniSEED data.

Point it at a directory of miniSEED files and it produces a single CSV of windowed features —
statistical, spectral, and cross-channel — ready for downstream analysis or model training.

```bash
sismokaos-cli run --data-dir DATA --out-dir OUT
```

Everything runs natively in Rust: miniSEED decoding via [libmseed][libmseed], filtering, windowing,
and feature computation. There is no Python dependency and no intermediate files on disk.

[libmseed]: https://github.com/EarthScope/libmseed

---

## Contents

- [Requirements](#requirements)
- [Building](#building)
- [Quick start](#quick-start)
- [Input expectations](#input-expectations)
- [CLI reference](#cli-reference)
- [Configuration](#configuration)
- [How the pipeline works](#how-the-pipeline-works)
- [Output format](#output-format)
- [Feature reference](#feature-reference)
- [Memory and performance](#memory-and-performance)
- [Project layout](#project-layout)
- [Vendored `libmseed-sys`](#vendored-libmseed-sys)
- [Troubleshooting](#troubleshooting)

---

## Requirements

- **Rust 1.85+** (the crate uses edition 2024)
- **A C compiler** (`cc`/`gcc`/`clang`) — libmseed is compiled from vendored C sources
- **libclang** — `bindgen` uses it to generate the FFI bindings at build time

On Arch: `pacman -S base-devel clang`
On Debian/Ubuntu: `apt install build-essential libclang-dev`

No system libmseed installation is needed; the C library is vendored and built automatically.

## Building

```bash
cargo build --release
```

The binary lands at `target/release/sismokaos-cli`.

> The first build compiles libmseed from C and runs `bindgen`, so it takes noticeably longer than
> subsequent builds. A few `-Wdiscarded-qualifiers` warnings from libmseed's own C sources are
> expected and harmless.

## Quick start

```bash
# 1. Write a default config (optional — sensible defaults apply without one)
sismokaos-cli init

# 2. Check the configuration resolves without doing any work
sismokaos-cli run --data-dir data/BODT_20240501_HH --out-dir results --dry-run

# 3. Run it
sismokaos-cli run --data-dir data/BODT_20240501_HH --out-dir results
```

Output:

```
=== STARTING PIPELINE ===
 Input: data/BODT_20240501_HH
 Windows processed: 15549
All processing finished successfully
```

```
results/
├── BODT_20240501_HH_features.csv           # the features
└── BODT_20240501_HH_features.run_metadata.json  # config used for this run
```

The CSV is named after the input directory, so re-running against a different `--data-dir` into the
same `--out-dir` will not overwrite previous results.

## Input expectations

`--data-dir` is scanned for files (non-recursively); every file in it is treated as miniSEED input.

- **All three components (E, N, Z) must be present.** The run aborts with a clear error if any is
  missing, rather than silently emitting a CSV with blank columns.
- Components are identified from the last character of the FDSN source identifier, so
  `FDSN:KO_BODT__H_H_E` → `E`. The aliases `1`→`E` and `2`→`N` are also accepted.
- **File layout does not matter.** One file per day, one giant file, or files split arbitrarily all
  produce identical output — see [How the pipeline works](#how-the-pipeline-works). Files are read
  in filename order but records are replayed in true time order regardless.
- **Sample rate must be uniform** across all records; mixed rates are rejected.
- Both miniSEED v2 and v3 are supported, as are `Steim1`/`Steim2`/`int32`/`float32`/`float64`
  encodings.
- Gaps and overlaps between records are handled (see below); records do not need to be contiguous.

## CLI reference

```
sismokaos-cli [OPTIONS] <COMMAND>

Options:
  -c, --config <FILE>   Path to the JSON configuration file [default: config.json]
```

### `run`

Runs the full pipeline (preprocess → extract) over a directory.

| Flag                | Required | Description                                                      |
| ------------------- | -------- | ---------------------------------------------------------------- |
| `--data-dir <DATA>` | yes      | Directory containing the miniSEED files                          |
| `--out-dir <OUT>`   | yes      | Directory for the feature CSV and run metadata                   |
| `--station <NAME>`  | no       | Station label recorded in the run metadata                       |
| `--win-sec <N>`     | no       | Window length in seconds                                         |
| `--fs <HZ>`         | no       | Target sample rate after decimation                              |
| `--freqmin <HZ>`    | no       | Bandpass lower cutoff                                            |
| `--freqmax <HZ>`    | no       | Bandpass upper cutoff                                            |
| `--dry-run`         | no       | Resolve and validate configuration, then exit without processing |

CLI flags override the config file.

### `init`

```bash
sismokaos-cli init [--out config.json]
```

Writes a config file populated with the defaults.

## Configuration

Configuration is read from `config.json` (override with `-c`). Any missing or unparseable file
falls back to defaults with a warning, so the config file is entirely optional.

```json
{
  "station": "ELZG",
  "fs": 5.0,
  "win_sec": 200,
  "step_sec": 50,
  "sta_sec": 0.5,
  "lta_sec": 60,
  "freqmin": 0.1,
  "freqmax": 2.0
}
```

| Field      | Default  | Meaning                                                          |
| ---------- | -------- | ---------------------------------------------------------------- |
| `station`  | `"ELZG"` | Label only; recorded in run metadata, does not affect processing |
| `fs`       | `5.0`    | Target sample rate (Hz) after decimation                         |
| `win_sec`  | `200`    | Analysis window length in seconds                                |
| `step_sec` | `50`     | Hop between consecutive windows in seconds                       |
| `sta_sec`  | `0.5`    | STA/LTA short-term average length in seconds                     |
| `lta_sec`  | `60`     | STA/LTA long-term average length in seconds                      |
| `freqmin`  | `0.1`    | Bandpass lower cutoff (Hz)                                       |
| `freqmax`  | `2.0`    | Bandpass upper cutoff (Hz)                                       |

Window and step sizes in samples are derived as `win_sec × fs` and `step_sec × fs`.

With the defaults, windows are 200 s long and advance 50 s at a time, so consecutive windows
overlap by 150 s.

> **Decimation is integer-factor.** The factor is `floor(native_fs / fs)`, so the effective output
> rate is `native_fs / factor`, which may differ from `fs` if the two do not divide evenly. The
> effective rate is what gets recorded in the run metadata and used for all time calculations.

## How the pipeline works

```
   index          replay in         assemble           per block                per window
 all records  →   time order   →   into 1-hour   →   detrend, filter,   →   51 features   →   CSV row
 (headers)                          blocks           decimate               × N windows
```

**1. Index.** A header-only pass walks every file, locating each record with libmseed's `detect()`
and parsing its header _without_ unpacking sample data. It records each record's file, byte offset,
length, start time, component and sample count.

**2. Replay in time order.** The index is sorted by start time and records are re-read by seeking
to their offsets. This matters because miniSEED files are commonly written _channel-sequentially_ —
every E record for the day, then every N, then every Z — rather than interleaved by time. Reading
such a file front to back would finish an entire component before the other two began, making
block-wise processing impossible. Sorting makes file layout irrelevant to the result.

**3. Assemble into blocks.** Records are scattered into a rolling buffer by absolute time, so
overlaps land correctly and gaps stay as `NaN`. A block is emitted once a watermark confirms the
input has moved safely past it (60 s of slack absorbs out-of-order records). Blocks are one hour of
signal; the buffer is drained after each, which is what keeps memory flat.

**4. Gap filling.** Gaps bounded by real data on both sides are filled by cubic Hermite
interpolation between the boundary samples. Gaps at the very start or end of the data are left as
`NaN` — genuinely missing data stays marked as missing.

**5. Detrend and filter.** Each component is linearly detrended (which also removes the mean), then
bandpass filtered with a 4th-order Butterworth applied forward and backward for zero phase shift
(equivalent to SciPy/MATLAB `filtfilt`).

Each block is filtered with 120 s of the preceding signal prepended so the filter has settled before
reaching samples that are actually kept; the context is then discarded. Samples still `NaN` after
gap filling are zeroed for the filter pass and restored to `NaN` afterwards — without this, a single
`NaN` would propagate through the IIR filter and destroy the entire component.

**6. Decimate.** Integer-factor decimation onto a _single global sample grid_, so blocks join
seamlessly with no phase drift at boundaries.

**7. Window and extract.** Overlapping windows are cut from the decimated stream and features
computed in parallel across cores (via `rayon`). Rows are written to CSV as they are produced.

Because E/N/Z are placed on a shared absolute-time axis, the three components are sample-aligned by
construction — which is what the cross-channel features assume.

### Correctness of the streaming design

Splitting the work into blocks does not change the result. The same 9-day dataset processed as one
434 MB file and as nine separate day-files produces **bit-identical** output across all 1,585,947
numeric cells.

## Output format

One CSV per run, named `<data-dir-name>_features.csv`, alongside a
`<data-dir-name>_features.run_metadata.json` recording the exact configuration used.

| Column               | Description                                                         |
| -------------------- | ------------------------------------------------------------------- |
| `Pencere_ID`         | Window identifier, `<dir>_w<n>`, 1-based                            |
| `Zaman_Dk`           | Time in minutes from the start of the data, at the window's **end** |
| _51 feature columns_ | See [Feature reference](#feature-reference)                         |
| _51 `_DEV` columns_  | First difference of each feature versus the previous window         |

104 columns total. Feature columns are sorted alphabetically, with all `_DEV` columns after the
base ones.

**Blank cells** mean the value was not computable, and are written as empty rather than `NaN`:

- The first row's `_DEV` columns are always blank (no preceding window).
- A component's features are blank for any window overlapping missing data.

## Feature reference

Per component (`E_`, `N_`, `Z_` prefix), 16 features each:

| Feature                             | Description                                                                 |
| ----------------------------------- | --------------------------------------------------------------------------- |
| `PEAK`                              | Maximum absolute amplitude                                                  |
| `RMS`                               | Root mean square amplitude                                                  |
| `SKEWNESS`                          | Third standardised moment                                                   |
| `KURTOSIS`                          | Fourth standardised moment (not excess)                                     |
| `ZCR`                               | Zero-crossing rate                                                          |
| `STA_LTA_Max` / `_Mean` / `_Median` | Short-term/long-term average ratio statistics, from `sta_sec` and `lta_sec` |
| `DOMINANT_FREQ`                     | Frequency of peak power (DC bin excluded)                                   |
| `SPECTRAL_CENTROID`                 | Power-weighted mean frequency                                               |
| `LOW_FREQ_ENERGY`                   | Fraction of power at ≤ 1 Hz                                                 |
| `HIGH_FREQ_ENERGY`                  | Fraction of power at > 1 Hz                                                 |
| `HJORTH_ACTIVITY`                   | Variance of the signal                                                      |
| `HJORTH_MOBILITY`                   | Mean frequency proxy                                                        |
| `HJORTH_COMPLEXITY`                 | Bandwidth proxy                                                             |
| `PERMUTATION_ENTROPY`               | Ordinal-pattern complexity, embedding dimension 3, normalised to `ln(6)`    |

Cross-channel (3 features):

| Feature         | Description                         |
| --------------- | ----------------------------------- |
| `EN_CROSS_CORR` | Pearson correlation between E and N |
| `EZ_CROSS_CORR` | Pearson correlation between E and Z |
| `NZ_CROSS_CORR` | Pearson correlation between N and Z |

**3 × 16 + 3 = 51 features**, each with a `_DEV` counterpart.

Spectral features are computed from the FFT power spectrum of the standardised window. The 1 Hz
low/high energy split is currently fixed.

## Memory and performance

Peak memory is bounded by the block size, not by the size of the input. It does not matter whether
the data arrives as many files or one enormous one:

| Input                 | Peak RSS  |
| --------------------- | --------- |
| 9 files, 434 MB total | 64 MB     |
| **1 file, 434 MB**    | **64 MB** |
| 1 file, 868 MB        | 89 MB     |
| 25 files, 1.2 GB      | 108 MB    |

The residual growth is the record index itself, roughly 48 bytes per record — about 45 MB for a
year of continuous 100 Hz three-component data.

Throughput: 9 days of 100 Hz three-component data (434 MB, 15,549 windows) processes in about
**10 seconds** on 12 cores. Feature extraction is parallel; reading and filtering are sequential.

## Project layout

```
src/
├── main.rs         CLI entry point, progress reporting
├── cli.rs          Argument definitions (clap)
├── config.rs       Config file loading, CLI merge, derived values
├── preprocess.rs   miniSEED indexing, block streaming, gap fill, filter, decimate
├── engine.rs       Windowing, parallel feature extraction, orchestration
├── export.rs       Streaming CSV writer with derivative columns
├── types.rs        Pipeline progress events
└── features/
    ├── mod.rs      Per-window feature assembly and FFT
    ├── math.rs     Statistical and spectral feature implementations
    └── cross.rs    Cross-channel correlation
vendor/
└── libmseed-sys/   Patched vendored FFI bindings (see below)
```

## Vendored `libmseed-sys`

`vendor/libmseed-sys/` is a locally patched copy of the crate, wired in through
`[patch.crates-io]` in `Cargo.toml`.
The upstream crate's `bindgen` invocation trips over glibc's `_IO_FILE` internals on modern glibc
(2.44+), failing with a `size_of::<_IO_FILE>() - 216` overflow in a generated layout assertion. The
patch blocklists the stdio internal types and substitutes an opaque `FILE`, which the API only ever
handles as a pointer. Layout assertions remain enabled for all the `MS3*` structs that are actually
dereferenced, so genuine ABI mismatches would still be caught.

### Note on upstream `mseed` leaks

The pipeline deliberately avoids two leaky paths in the `mseed` crate:

- `MSTraceList::insert` transfers ownership via `into_raw()`, but libmseed's
  `mstl3_addmsr_recordptr` _copies_ the record rather than taking ownership — leaking every record.
  Records are therefore read and scattered individually instead of via a trace list.
- `MSRecord::sid()` leaks several `CString`s per call through `NetStaLocCha`. `sid_lossy()` is used
  instead, which is pure Rust.

Both mattered: together they were the dominant source of memory growth before being worked around.

## Troubleshooting

**`Component 'N' not found in the input files; need all of E, N, Z`**
The directory does not contain all three components. Check that channel codes end in `E`/`N`/`Z`
(or `1`/`2`/`Z`) and that files for every component are present.

**`Mixed sample rates in ... ; not supported`**
Records in the input disagree on sample rate. Split the data by rate and run separately.

**`Indeterminate record length at byte N of ...; cannot index this file`**
A record's length could not be determined — usually a truncated or corrupt file, or a non-miniSEED
file sitting in `--data-dir`. Remove non-data files from the directory.

**`Not enough data for a single window`**
The input is shorter than `win_sec` after decimation, or is almost entirely gaps.

**`Skipped N record(s) that arrived after their block had been emitted`**
A warning, not an error: records were more than 60 s out of order relative to the assembly
watermark. Usually harmless; if the count is large, the input timestamps may be inconsistent.

**Build fails looking for `libclang`**
Install `clang`/`libclang-dev`. `bindgen` needs it to generate the libmseed bindings.

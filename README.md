# Sismokaos CLI

Sismokaos CLI is a command-line interface tool for preprocessing and feature extraction of seismic data from miniSEED files.

## Features

- Parse and process miniSEED files from a local directory.
- Stream-safe preprocessing pipeline (gap fill, detrend, bandpass, decimation).
- Full feature extraction pipeline (`run`) producing per-window engineered features.
- Preprocess-only pipeline (`preprocess`) producing filtered/decimated component streams without feature extraction.
- Configurable processing parameters via JSON config and CLI overrides.
- Output results with metadata (see **Output formats** below).

## Output formats

The two pipelines write different things, because they are consumed
differently.

### `preprocess` → flat `f32` + JSON sidecar

`<stem>_preprocessed.f32` is little-endian `float32`, interleaved
`E N Z E N Z …`, alongside `<stem>_preprocessed.f32.json`.

```python
import json, numpy as np
meta = json.load(open("out_preprocessed.f32.json"))
a = np.memmap("out_preprocessed.f32", dtype="<f4", mode="r").reshape(-1, 3)
```

This used to be zstd Parquet with five `f64` columns. It was replaced because
the Parquet was never read as Parquet — the model side loaded it once and
rewrote it as exactly this layout — and because it was *larger*: on the
718,848,001-sample 10 Hz archive, 16.41 GB of Parquet against 8.63 GB here.
Two of its five columns (`index`, `Zaman_Dk`) were pure functions of the row
number, 11.9% of the bytes, and the other three were stored at double the
precision the consumer keeps.

**Timing is piecewise.** The pipeline restarts its clock at every data gap, so
the sidecar carries a `segments` table of `(sample_offset, start_epoch)` rather
than one start time. Sample `i` sits at `start_epoch + (i - sample_offset) / fs`
for the last segment beginning at or before it. A single global start time
would place every sample after the first gap at the wrong moment.

Unfillable gaps stay `NaN`, which `f32` represents exactly; whether to
zero-fill is left to the consumer.

### `run` → Parquet

The feature table is small, heterogeneous and read whole, which is what Parquet
is for. It keeps `<name>` and `<name>_DEV` columns; the `_DEV` columns are
first differences, and the models consume them as features.

## Guards

Configurations whose damage would be invisible in the output are rejected
before any work starts:

- **`freqmax` at or above the output Nyquist** is a hard error (exit 2). The
  bandpass doubles as the anti-alias filter — decimation follows it with no
  separate low-pass — so a too-high `freqmax` folds everything above Nyquist
  back into the band with no way to detect it afterwards. Landing within 10%
  of Nyquist warns instead.
- **A requested `fs` that does not divide the native rate** warns: the
  decimation factor is an integer, so asking for 30 Hz of a 100 Hz archive
  yields 33.3 Hz, and window lengths derived from the requested rate would not
  match the data.

## Installation

Ensure you have Rust installed, then clone this repository and run:

```bash
cargo build --release
```

The binary will be located at `target/release/sismokaos-cli`.

## Usage

Initialize a default config:

```bash
sismokaos-cli init --out config.json
```

Run the full pipeline:

```bash
sismokaos-cli --config config.json run --data-dir ./data --out-dir ./output
```

Run preprocessing only (no feature extraction):

```bash
sismokaos-cli --config config.json preprocess --data-dir ./data --out-dir ./output
```

Dry-run (validate config/args only):

```bash
sismokaos-cli --config config.json preprocess --data-dir ./data --out-dir ./output --dry-run
```

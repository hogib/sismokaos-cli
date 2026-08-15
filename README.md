# Sismokaos CLI

Sismokaos CLI is a command-line interface tool for preprocessing and feature extraction of seismic data from miniSEED files.

## Features

- Parse and process miniSEED files from a local directory.
- Stream-safe preprocessing pipeline (gap fill, detrend, bandpass, decimation).
- Full feature extraction pipeline (`run`) producing per-window engineered features.
- Preprocess-only pipeline (`preprocess`) producing filtered/decimated component streams without feature extraction.
- Configurable processing parameters via JSON config and CLI overrides.
- Output results as CSV with metadata.

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

import argparse
import re
from datetime import datetime
from pathlib import Path

import numpy as np
import numpy.ma as _nma
import scipy.interpolate as _sci_interp
from obspy import UTCDateTime, read


def _interpolate_nans(data, mask):
    valid = np.where(~np.isnan(data))[0]
    if len(valid) > 3:
        f = _sci_interp.interp1d(valid, data[valid], kind="cubic", bounds_error=False, fill_value=np.nan)
        data[np.where(mask)[0]] = f(np.where(mask)[0])
    return data

def main():
    parser = argparse.ArgumentParser(description="MSEED to NPY Preprocessor")
    parser.add_argument("--mseed", required=True, type=Path, help="Path to input MSEED file")
    parser.add_argument("--out-dir", required=True, type=Path, help="Output directory for NPY files")
    parser.add_argument("--fs", type=float, default=5.0, help="Target sampling rate")
    parser.add_argument("--freqmin", type=float, default=0.1, help="Bandpass minimum frequency")
    parser.add_argument("--freqmax", type=float, default=2.0, help="Bandpass maximum frequency")
    parser.add_argument("--gap-threshold", type=float, default=2.0, help="Large gap threshold in seconds")
    
    args = parser.parse_args()

    print(f"[PYTHON] Reading {args.mseed.name}...")
    st_full = read(str(args.mseed))
    
    for tr in st_full:
        if tr.data.dtype != np.float64:
            tr.data = tr.data.astype(np.float64)

    real_fs = st_full[0].stats.sampling_rate
    decimation_factor = max(1, int(real_fs / args.fs))
    
    # 1. Manage Gaps & Interpolate
    gaps = st_full.get_gaps(min_gap=-1)
    actual_gaps = [g for g in gaps if g[7] > 0] if gaps else []
    large_gaps = [g for g in actual_gaps if g[6] >= args.gap_threshold]
    
    if not actual_gaps:
        st_full.merge()
    else:
        st_full.merge(fill_value=np.nan)
        for tr in st_full:
            data = np.ma.filled(tr.data, np.nan) if np.ma.is_masked(tr.data) else np.array(tr.data, dtype=float)
            nan_mask = np.isnan(data)
            if nan_mask.any():
                data = _interpolate_nans(data, nan_mask)
            tr.data = data

    # 2. Filter & Decimate
    print(f"[PYTHON] Filtering ({args.freqmin}-{args.freqmax}Hz) and Decimating...")
    st_full.detrend("demean")
    st_full.detrend("linear")
    st_full.filter("bandpass", freqmin=args.freqmin, freqmax=args.freqmax, corners=4, zerophase=True)
    
    if decimation_factor > 1:
        for tr in st_full:
            tr.decimate(factor=decimation_factor, no_filter=True)

    # 3. Save as Structured NPY
    args.out_dir.mkdir(parents=True, exist_ok=True)
    
    # For this script, we output a single large NPY for simplicity, 
    # or you can re-add your 1-hour windowing logic here.
    max_len = max(len(tr.data) for tr in st_full)
    dtype = [('E', 'f8'), ('N', 'f8'), ('Z', 'f8')]
    struct_arr = np.empty(max_len, dtype=dtype)
    
    for comp in ['E', 'N', 'Z']:
        struct_arr[comp] = np.nan
        try:
            tr = st_full.select(component=comp)[0]
            struct_arr[comp][:len(tr.data)] = tr.data
        except IndexError:
            pass
            
    timestamp = st_full[0].stats.starttime.strftime('%Y%m%d_%H%M%S')
    out_file = args.out_dir / f"{timestamp}_ENZ.npy"
    np.save(out_file, struct_arr)
    
    print(f"[PYTHON] Preprocessing complete: {out_file.name}")

if __name__ == "__main__":
    main()

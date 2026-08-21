//! Nonlinear (chaotic) time-series metrics: Wolf and Rosenstein maximum
//! Lyapunov exponents, Sample Entropy, and Correlation Dimension.
//!
//! Ported from the Python sibling project's `sismokaos/chaos_algorithms.py`
//! (itself adapted from S. Sarwar, A. Likens, N. Stergiou, S. Mastorakis,
//! "A nonlinear analysis software toolkit for biomechanical data",
//! arXiv:2311.06723, 2023), so results stay comparable between the two
//! implementations.

/// Maximum Lyapunov exponent via Wolf (1985).
///
/// `x` must already be z-score normalized. Returns `NaN` if there isn't
/// enough data to embed at the requested `tau`/`dim`/`evolve`.
pub fn wolf_lye(x: &[f64], fs: f64, tau: usize, dim: usize, evolve: usize) -> f64 {
    let n = x.len();
    if dim == 0 || (dim - 1) * tau + evolve >= n {
        return f64::NAN;
    }

    let min_v = x.iter().copied().fold(f64::INFINITY, f64::min);
    let max_v = x.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let scale_mx = (max_v - min_v) / 10.0;
    let dt = 1.0 / fs;

    let m = n - (dim - 1) * tau;
    let mut y = vec![0.0; m * dim];
    for i in 0..dim {
        for j in 0..m {
            y[j * dim + i] = x[i * tau + j];
        }
    }

    let npt = n - (dim - 1) * tau - evolve;
    if npt == 0 {
        return f64::NAN;
    }
    let y_len = (npt + evolve).min(m);
    fn point(y: &[f64], dim: usize, idx: usize) -> &[f64] {
        &y[idx * dim..idx * dim + dim]
    }
    let dist = |a: &[f64], b: &[f64]| -> f64 {
        a.iter().zip(b).map(|(&p, &q)| (p - q).powi(2)).sum::<f64>().sqrt()
    };

    // Initial neighbour pair: nearest point to index 0, excluding a small
    // temporal window around it.
    let mut current_pair = {
        let mut best_idx = usize::MAX;
        let mut best_dist = f64::INFINITY;
        for i in 0..npt.min(y_len) {
            if i <= 10 {
                continue;
            }
            let d = dist(point(&y, dim, 0), point(&y, dim, i));
            if d > 0.0 && d < best_dist {
                best_dist = d;
                best_idx = i;
            }
        }
        if best_idx == usize::MAX {
            return f64::NAN;
        }
        best_idx
    };

    let mut its: u64 = 0;
    let mut dist_sum = 0.0;
    let mut lye = 0.0;
    let mut i = 0;
    while i < npt {
        let ep = i + evolve;
        if ep >= y_len {
            break;
        }
        let safe = current_pair + evolve < y_len;
        let pair_ep = if safe { current_pair + evolve } else { current_pair + evolve - 1 };
        if pair_ep >= y_len {
            break;
        }

        let start_dist = dist(point(&y, dim, i), point(&y, dim, current_pair));
        let end_dist = dist(point(&y, dim, ep), point(&y, dim, pair_ep));

        if start_dist > 0.0 && end_dist > 0.0 {
            dist_sum += (end_dist / start_dist).log2() / (evolve as f64 * dt);
            its += 1;
            lye = dist_sum / its as f64;
        }

        if end_dist < scale_mx {
            current_pair += evolve;
            if current_pair > npt {
                current_pair -= evolve;
                current_pair = wolf_next_point(true, &y, dim, i, current_pair, npt, y_len, evolve, scale_mx);
            }
        } else {
            current_pair = wolf_next_point(false, &y, dim, i, current_pair, npt, y_len, evolve, scale_mx);
        }

        i += evolve;
    }

    lye
}

/// Selects the next nearest neighbour for the Wolf algorithm. Mirrors
/// `_wolf_next_point` in the Python port.
///
/// `flag1` corresponds to the Python function's `flag == 1` call (made when
/// the running neighbour pair index overflowed `NPT`): it skips straight to
/// the nearest valid neighbour by distance alone, ignoring the angle
/// criterion entirely. `flag1 == false` (Python's `flag == 0`, made when the
/// trajectory pair diverged past `SCALEMX`) first tries the nearest neighbour
/// *within* the 30-degree angle limit, accepting it only if it's also within
/// `SCALEMX`; otherwise it falls through to the same angle-agnostic nearest
/// neighbour as `flag1`.
///
/// Note: in the reference Python implementation the angle limit passed
/// between calls is only ever reset to 30 degrees or left unchanged, so it
/// never actually varies across a run -- it's hardcoded as a constant here
/// rather than threaded through as mutable state.
fn wolf_next_point(
    flag1: bool,
    y: &[f64],
    dim: usize,
    current_point: usize,
    current_point_pair: usize,
    npt: usize,
    y_len: usize,
    evolve: usize,
    scale_mx: f64,
) -> usize {
    const ANGLE_LIMIT_DEG: f64 = 30.0;

    let point = |idx: usize| -> &[f64] { &y[idx * dim..idx * dim + dim] };
    let ep = current_point + evolve;
    if ep >= y_len {
        return current_point_pair;
    }

    let safe = current_point_pair + evolve < y_len;
    let end_idx = if safe { current_point_pair + evolve } else { current_point_pair + evolve - 1 };
    if end_idx >= y_len {
        return current_point_pair;
    }

    let v_curr: Vec<f64> = point(ep).iter().zip(point(end_idx)).map(|(&a, &b)| a - b).collect();
    let end_dist: f64 = v_curr.iter().map(|v| v * v).sum::<f64>().sqrt();

    let bound = npt.min(y_len);
    let in_excl_window = |k: usize| k + 10 >= ep && k <= ep + 10;

    if !flag1 {
        let mut best: Option<(usize, f64)> = None;
        for k in 0..bound {
            if in_excl_window(k) {
                continue;
            }
            let diff: Vec<f64> = point(ep).iter().zip(point(k)).map(|(&a, &b)| a - b).collect();
            let yd: f64 = diff.iter().map(|v| v * v).sum::<f64>().sqrt();
            if yd <= 0.0 {
                continue;
            }
            let dot: f64 = v_curr.iter().zip(diff.iter()).map(|(&a, &b)| a * b).sum();
            let denom = yd * end_dist;
            if denom <= 0.0 {
                continue;
            }
            let cos_t = (dot / denom).abs().clamp(-1.0, 1.0);
            let theta = cos_t.acos();
            if theta < ANGLE_LIMIT_DEG.to_radians() && best.is_none_or(|(_, bd)| yd < bd) {
                best = Some((k, yd));
            }
        }
        if let Some((idx, d)) = best {
            if d <= scale_mx {
                return idx;
            }
        }
        // Falls through to the angle-agnostic nearest neighbour below,
        // exactly as the Python `next_pt == -1` fallback does.
    }

    let mut best_idx = usize::MAX;
    let mut best_d = f64::INFINITY;
    for k in 0..bound {
        if in_excl_window(k) {
            continue;
        }
        let diff: Vec<f64> = point(ep).iter().zip(point(k)).map(|(&a, &b)| a - b).collect();
        let yd: f64 = diff.iter().map(|v| v * v).sum::<f64>().sqrt();
        if yd > 0.0 && yd < best_d {
            best_d = yd;
            best_idx = k;
        }
    }

    if best_idx == usize::MAX { current_point_pair } else { best_idx }
}

/// Short- and long-term Lyapunov exponents via Rosenstein (1993).
///
/// `x` must already be z-score normalized. `slope` is
/// `[short_start, short_end, long_start, long_end]` expressed in units of
/// `mean_period`. Returns `(short, long)`, either of which may be `NaN` if
/// the requested slope window falls outside the available divergence curve.
pub fn rosenstein_lye(
    x: &[f64],
    fs: f64,
    tau: usize,
    dim: usize,
    slope: [f64; 4],
    mean_period: f64,
) -> (f64, f64) {
    let n = x.len();
    if dim == 0 || (dim - 1) * tau >= n {
        return (f64::NAN, f64::NAN);
    }
    let m = n - (dim - 1) * tau;
    if m < 2 {
        return (f64::NAN, f64::NAN);
    }

    let mut y = vec![0.0; m * dim];
    for j in 0..dim {
        for i in 0..m {
            y[i * dim + j] = x[j * tau + i];
        }
    }
    let point = |idx: usize| -> &[f64] { &y[idx * dim..idx * dim + dim] };

    let band = (dim - 1) * tau;

    // Nearest neighbour (excluding a temporal band around the diagonal) for
    // every embedded point.
    let mut ind2 = vec![0usize; m];
    for i in 0..m {
        let mut best_idx = usize::MAX;
        let mut best_d = f64::INFINITY;
        for k in 0..m {
            if k.abs_diff(i) <= band {
                continue;
            }
            let d: f64 = point(i).iter().zip(point(k)).map(|(&a, &b)| (a - b).powi(2)).sum();
            if d < best_d {
                best_d = d;
                best_idx = k;
            }
        }
        ind2[i] = if best_idx == usize::MAX { i } else { best_idx };
    }

    // Average log divergence at each evolution step j: average, over all
    // starting points i whose trajectory and its neighbour's both survive j
    // steps, of ln(distance after j steps).
    let mut ave_ln_div = vec![0.0f64; m];
    for j in 0..m {
        let mut sum = 0.0;
        let mut count = 0usize;
        for i in 0..m {
            let nn = ind2[i];
            if i + j >= m || nn + j >= m {
                continue;
            }
            let d: f64 = point(i + j).iter().zip(point(nn + j)).map(|(&a, &b)| (a - b).powi(2)).sum::<f64>().sqrt();
            if d > 0.0 {
                sum += d.ln();
                count += 1;
            }
        }
        ave_ln_div[j] = if count > 0 { sum / count as f64 } else { 0.0 };
        if count == 0 {
            break;
        }
    }

    let nz = ave_ln_div.iter().filter(|&&v| v != 0.0).count();
    let time: Vec<f64> = (0..m).map(|i| i as f64 / fs / mean_period).collect();

    let fit = |lo: usize, hi: usize| -> f64 {
        if hi >= ave_ln_div.len() || hi > nz || lo > hi {
            return f64::NAN;
        }
        linear_regression_slope(&time[lo..=hi], &ave_ln_div[lo..=hi])
    };

    let round_idx = |v: f64| -> usize { python_round(v * mean_period * fs).max(0.0) as usize };
    let s_lo = if slope[0] == 0.0 { 0 } else { round_idx(slope[0]) };
    let s_hi = round_idx(slope[1]);
    let l_lo = round_idx(slope[2]);
    let l_hi = round_idx(slope[3]);

    (fit(s_lo, s_hi), fit(l_lo, l_hi))
}

/// Rounds like Python 3's built-in `round()` (round-half-to-even), not
/// Rust's `f64::round()` (round-half-away-from-zero). The two disagree
/// exactly at `.5` boundaries -- e.g. `round(2.5)` is `2` in Python but
/// `3.0` in Rust -- which is precisely where the Rosenstein slope-window
/// bounds land with the default `slope`/`mean_period`/`fs` values. Assumes
/// a non-negative input, which is all this module ever rounds.
fn python_round(v: f64) -> f64 {
    let floor = v.floor();
    let diff = v - floor;
    if diff < 0.5 {
        floor
    } else if diff > 0.5 {
        floor + 1.0
    } else if (floor as i64) % 2 == 0 {
        floor
    } else {
        floor + 1.0
    }
}

/// Ordinary least-squares slope of y against x.
fn linear_regression_slope(x: &[f64], y: &[f64]) -> f64 {
    let n = x.len() as f64;
    if n < 2.0 {
        return f64::NAN;
    }
    let mean_x = x.iter().sum::<f64>() / n;
    let mean_y = y.iter().sum::<f64>() / n;
    let mut num = 0.0;
    let mut den = 0.0;
    for (&xi, &yi) in x.iter().zip(y) {
        num += (xi - mean_x) * (yi - mean_y);
        den += (xi - mean_x).powi(2);
    }
    if den == 0.0 { f64::NAN } else { num / den }
}

/// Sample Entropy, using Chebyshev (max-norm) distance between
/// `m`/`m+1`-length templates. `data` must already be z-score normalized;
/// `r` is applied as a multiple of the segment's own std.
pub fn sample_entropy(data: &[f64], m: usize, r: f64) -> f64 {
    let n = data.len();
    if n <= m + 1 {
        return f64::NAN;
    }
    let mean = data.iter().sum::<f64>() / n as f64;
    let std = (data.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / (n as f64 - 1.0)).sqrt();
    let tol = r * std;
    if tol <= 0.0 {
        return f64::NAN;
    }

    let l = n - m;
    let mut b_count: u64 = 0;
    let mut a_count: u64 = 0;

    for i in 0..l {
        for j in 0..l {
            if i == j {
                continue;
            }
            let mut dist_m: f64 = 0.0;
            for k in 0..m {
                dist_m = dist_m.max((data[i + k] - data[j + k]).abs());
            }
            if dist_m <= tol {
                b_count += 1;
                let dist_m1 = dist_m.max((data[i + m] - data[j + m]).abs());
                if dist_m1 <= tol {
                    a_count += 1;
                }
            }
        }
    }

    if a_count == 0 || b_count == 0 {
        return f64::NAN;
    }
    -((a_count as f64) / (b_count as f64)).ln()
}

/// Correlation dimension estimate via a Grassberger-Procaccia style
/// correlation-integral slope fit, ported from the MATLAB `corrdim.m`
/// derived implementation used by the Python sibling project.
pub fn correlation_dimension(x: &[f64], tau: usize, de: usize) -> f64 {
    let n = x.len();
    if de == 0 || (de - 1) * tau >= n {
        return 0.0;
    }
    let sample_count = n - (de - 1) * tau;
    if sample_count == 0 {
        return 0.0;
    }

    let mut y = vec![0.0; de * sample_count];
    for i in 0..de {
        for j in 0..sample_count {
            y[i * sample_count + j] = x[i * tau + j];
        }
    }
    let col = |idx: usize| -> Vec<f64> { (0..de).map(|i| y[i * sample_count + idx]).collect() };

    let bins = 200usize;
    let k = de * tau;
    if k + 1 >= sample_count {
        return 0.0;
    }
    let pair_count = sample_count - k - 1;

    let dist_row = |i: usize| -> Vec<f64> {
        let pi = col(i);
        (i + k + 1..sample_count)
            .map(|j| {
                let pj = col(j);
                pi.iter().zip(pj.iter()).map(|(&a, &b)| (a - b).powi(2)).sum::<f64>().sqrt()
            })
            .collect()
    };

    let mut eps1 = 0.0f64;
    let mut eps2 = f64::INFINITY;
    for i in 0..pair_count {
        for &d in &dist_row(i) {
            if d > eps1 {
                eps1 = d;
            }
            if d < eps2 {
                eps2 = d;
            }
        }
    }
    if eps2 == 0.0 {
        eps2 = f64::EPSILON;
    }
    if eps1 <= eps2 {
        return 0.0;
    }

    let log_eps2 = eps2.ln();
    let log_eps1 = eps1.ln();
    let epsilon: Vec<f64> = (0..bins)
        .map(|b| (log_eps2 + (log_eps1 - log_eps2) * b as f64 / (bins - 1) as f64).exp())
        .collect();

    let mut ci = vec![0.0f64; bins];
    for i in 0..pair_count {
        let mut sorted = dist_row(i);
        sorted.sort_by(f64::total_cmp);
        let mut prev_count = 0usize;
        for (b, &e) in epsilon.iter().enumerate() {
            let count = sorted.partition_point(|&d| d <= e);
            ci[b] += (count - prev_count) as f64;
            prev_count = count;
        }
    }

    // Cumulative sum -> correlation integral -> log-log curve.
    let denom = (sample_count - k) as f64 * (sample_count - k) as f64;
    let mut cum = 0.0;
    let mut curve = vec![f64::NAN; bins];
    for b in 0..bins {
        cum += ci[b];
        if cum > 0.0 {
            curve[b] = (cum / denom).ln() + ((sample_count - k - 1) as f64).ln();
        }
    }
    let log_eps: Vec<f64> = epsilon.iter().map(|e| e.ln()).collect();

    let finite: Vec<(f64, f64)> = log_eps
        .iter()
        .zip(curve.iter())
        .filter(|(_, v)| v.is_finite())
        .map(|(&e, &v)| (e, v))
        .collect();
    if finite.is_empty() {
        return 0.0;
    }

    let lo = finite.iter().map(|&(_, v)| v).fold(f64::INFINITY, f64::min);
    let hi = finite.iter().map(|&(_, v)| v).fold(f64::NEG_INFINITY, f64::max);
    let mid = (lo + hi) / 2.0;
    let q = (hi - lo) / 4.0;

    let selected: Vec<(f64, f64)> = finite
        .into_iter()
        .filter(|&(_, v)| v > mid && v < mid + q)
        .collect();
    if selected.len() < 2 {
        return 0.0;
    }

    let xs: Vec<f64> = selected.iter().map(|&(e, _)| e).collect();
    let ys: Vec<f64> = selected.iter().map(|&(_, v)| v).collect();
    linear_regression_slope(&xs, &ys)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic (RNG-free) 1000-sample signal, reproducible bit-for-bit
    /// in Python (`np.sin(i*0.13) + 0.5*np.sin(i*0.031)`) so the two sibling
    /// implementations can be checked against the same reference values
    /// without needing to match PRNGs across languages.
    fn reference_signal() -> Vec<f64> {
        let n = 1000;
        let raw: Vec<f64> = (0..n)
            .map(|i| (i as f64 * 0.13).sin() + 0.5 * (i as f64 * 0.031).sin())
            .collect();
        let mean = raw.iter().sum::<f64>() / n as f64;
        let std = (raw.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / (n as f64 - 1.0)).sqrt();
        raw.iter().map(|&v| (v - mean) / std).collect()
    }

    // Reference values computed by sismokaos/chaos_algorithms.py (the Python
    // sibling project) on the exact same `reference_signal()` above, with
    // fs=5.0, tau=5, dim=5, evolve=5, slope=[0.5,5.0,5.0,20.0],
    // mean_period=1.0, sample-entropy m=2/r=0.2.
    const REF_WOLF_LYE: f64 = 0.3035492535457496;
    const REF_ROS_SHORT: f64 = 0.38280772273173597;
    const REF_ROS_LONG: f64 = -0.029816027419789917;
    const REF_SAMP_ENT: f64 = 0.39819536425420143;
    const REF_CORR_DIM: f64 = 2.3299697367683927;

    fn assert_close(actual: f64, expected: f64, tol: f64, label: &str) {
        assert!(
            (actual - expected).abs() <= tol,
            "{label}: expected {expected}, got {actual} (tolerance {tol})"
        );
    }

    #[test]
    fn wolf_lye_matches_python_reference() {
        let x = reference_signal();
        let got = wolf_lye(&x, 5.0, 5, 5, 5);
        assert_close(got, REF_WOLF_LYE, 1e-10, "wolf_lye");
    }

    #[test]
    fn rosenstein_lye_matches_python_reference() {
        let x = reference_signal();
        let (short, long) = rosenstein_lye(&x, 5.0, 5, 5, [0.5, 5.0, 5.0, 20.0], 1.0);
        assert_close(short, REF_ROS_SHORT, 1e-10, "ros_short");
        assert_close(long, REF_ROS_LONG, 1e-10, "ros_long");
    }

    #[test]
    fn sample_entropy_matches_python_reference() {
        let x = reference_signal();
        let got = sample_entropy(&x, 2, 0.2);
        assert_close(got, REF_SAMP_ENT, 1e-9, "samp_ent");
    }

    #[test]
    fn correlation_dimension_matches_python_reference() {
        let x = reference_signal();
        let got = correlation_dimension(&x, 5, 5);
        assert_close(got, REF_CORR_DIM, 1e-10, "corr_dim");
    }

    #[test]
    fn functions_return_nan_or_zero_on_too_short_input() {
        let short = vec![1.0, 2.0, 3.0];
        assert!(wolf_lye(&short, 5.0, 5, 5, 5).is_nan());
        let (s, l) = rosenstein_lye(&short, 5.0, 5, 5, [0.5, 5.0, 5.0, 20.0], 1.0);
        assert!(s.is_nan() && l.is_nan());
        assert!(sample_entropy(&short, 2, 0.2).is_nan());
        assert_eq!(correlation_dimension(&short, 5, 5), 0.0);
    }
}

pub fn compute_moments(segment: &[f64]) -> (f64, f64, f64, f64) {
    let n = segment.len() as f64;
    let mean = segment.iter().sum::<f64>() / n;

    let mut m2 = 0.0;
    let mut m3 = 0.0;
    let mut m4 = 0.0;

    for &x in segment {
        let dev = x - mean;
        let dev2 = dev * dev;
        m2 += dev2;
        m3 += dev2 * dev;
        m4 += dev2 * dev2;
    }

    (mean, m2 / n, m3 / n, m4 / n)
}

pub fn peak(segment: &[f64]) -> f64 {
    segment
        .iter()
        .copied()
        .map(f64::abs)
        .max_by(f64::total_cmp)
        .unwrap_or(f64::NAN)
}

pub fn compute_rms(segment: &[f64]) -> f64 {
    if segment.is_empty() {
        return f64::NAN;
    }
    let mean_sq = segment.iter().map(|&x| x * x).sum::<f64>() / (segment.len() as f64);
    mean_sq.sqrt()
}

pub fn compute_zcr(segment: &[f64]) -> f64 {
    if segment.len() < 2 {
        return f64::NAN;
    }
    // Branchless optimization: >= 0.0 check avoids float multiplication and signum branches
    let crossings = segment
        .windows(2)
        .filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0))
        .count();

    crossings as f64 / (segment.len() - 1) as f64
}

pub fn compute_sta_lta(segment: &[f64], nsta: usize, nlta: usize) -> (f64, f64, f64) {
    if segment.len() <= nlta || nsta == 0 || nlta == 0 {
        return (f64::NAN, f64::NAN, f64::NAN);
    }

    let mut cft_valid = Vec::with_capacity(segment.len() - nlta + 1);

    // Compute initial sums without allocating a squared data array
    let mut lta_sum: f64 = segment[0..nlta].iter().map(|&x| x * x).sum();
    let mut sta_sum: f64 = segment[(nlta - nsta)..nlta].iter().map(|&x| x * x).sum();

    cft_valid.push((sta_sum / nsta as f64) / (lta_sum / nlta as f64));

    // Sliding window sum updates
    for i in nlta..segment.len() {
        let new_sq = segment[i] * segment[i];
        let old_lta_sq = segment[i - nlta] * segment[i - nlta];
        let old_sta_sq = segment[i - nsta] * segment[i - nsta];

        lta_sum += new_sq - old_lta_sq;
        sta_sum += new_sq - old_sta_sq;

        let sta = sta_sum / nsta as f64;
        let lta = lta_sum / nlta as f64;

        let ratio = if lta <= f64::EPSILON { 0.0 } else { sta / lta };
        cft_valid.push(ratio);
    }

    let max = cft_valid
        .iter()
        .copied()
        .max_by(f64::total_cmp)
        .unwrap_or(f64::NAN);
    let mean = cft_valid.iter().sum::<f64>() / cft_valid.len() as f64;

    // O(N) Median Selection
    let mid = cft_valid.len() / 2;
    let (_, &mut median, _) = cft_valid.select_nth_unstable_by(mid, f64::total_cmp);

    // Average the middle two for mathematically perfect medians on even arrays
    let final_median = if cft_valid.len() % 2 == 0 {
        let max_left = cft_valid[..mid]
            .iter()
            .copied()
            .max_by(f64::total_cmp)
            .unwrap_or(median);
        (median + max_left) / 2.0
    } else {
        median
    };

    (max, mean, final_median)
}

pub fn dominant_frequency(power_spectrum: &[f64], freqs: &[f64]) -> f64 {
    if power_spectrum.len() <= 1 {
        return f64::NAN;
    }
    let (max_idx, _) = power_spectrum[1..]
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.total_cmp(b))
        .unwrap_or((0, &0.0));
    freqs.get(max_idx + 1).copied().unwrap_or(f64::NAN)
}

pub fn spectral_centroid_and_energy(
    freqs: &[f64],
    power_spectrum: &[f64],
    lf_hf_cutoff: f64,
) -> (f64, f64, f64) {
    let total_power: f64 = power_spectrum.iter().sum();
    if total_power <= 0.0 || total_power.is_nan() {
        return (0.0, 0.0, 0.0);
    }

    let mut centroid_sum = 0.0;
    let mut lf_energy = 0.0;
    let mut hf_energy = 0.0;

    for (&f, &p) in freqs.iter().zip(power_spectrum.iter()) {
        centroid_sum += f * p;
        if f <= lf_hf_cutoff {
            lf_energy += p;
        } else {
            hf_energy += p;
        }
    }

    (
        centroid_sum / total_power,
        lf_energy / total_power,
        hf_energy / total_power,
    )
}

pub fn compute_hjorth_parameters(segment: &[f64]) -> (f64, f64, f64) {
    if segment.len() < 3 {
        return (f64::NAN, f64::NAN, f64::NAN);
    }

    // Dynamic variance calculation without allocating diff arrays
    let n = segment.len() as f64;
    let n1 = (segment.len() - 1) as f64;
    let n2 = (segment.len() - 2) as f64;

    let mut sum_x = 0.0;
    let mut sum_x2 = 0.0;
    let mut sum_dx = 0.0;
    let mut sum_dx2 = 0.0;
    let mut sum_ddx = 0.0;
    let mut sum_ddx2 = 0.0;

    let mut prev_x = segment[0];
    let mut prev_dx = segment[1] - segment[0];

    sum_x += prev_x;
    sum_x2 += prev_x * prev_x;
    sum_dx += prev_dx;
    sum_dx2 += prev_dx * prev_dx;

    for &x in &segment[2..] {
        let dx = x - prev_x;
        let ddx = dx - prev_dx;

        sum_x += x;
        sum_x2 += x * x;
        sum_dx += dx;
        sum_dx2 += dx * dx;
        sum_ddx += ddx;
        sum_ddx2 += ddx * ddx;

        prev_x = x;
        prev_dx = dx;
    }
    sum_x += segment[1];
    sum_x2 += segment[1] * segment[1]; // catch the skipped element

    // var = (Sum(X^2) - (Sum(X)^2 / N)) / N
    let var_x = (sum_x2 - (sum_x * sum_x) / n) / n;
    let var_x_prime = (sum_dx2 - (sum_dx * sum_dx) / n1) / n1;
    let var_x_bis = (sum_ddx2 - (sum_ddx * sum_ddx) / n2) / n2;

    let activity = var_x;
    let mobility = if var_x > 0.0 {
        (var_x_prime / var_x).sqrt()
    } else {
        0.0
    };
    let complexity = if var_x_prime > 0.0 && mobility > 0.0 {
        ((var_x_bis / var_x_prime).sqrt()) / mobility
    } else {
        0.0
    };

    (activity, mobility, complexity)
}

pub fn compute_permutation_entropy(segment: &[f64]) -> f64 {
    if segment.len() < 3 {
        return f64::NAN;
    }

    let mut counts = [0_usize; 6];
    let mut total_windows = 0;

    for w in segment.windows(3) {
        let (a, b, c) = (w[0], w[1], w[2]);

        let idx = if a <= b {
            if b <= c {
                0
            } else if a <= c {
                1
            } else {
                4
            }
        } else {
            if a <= c {
                2
            } else if b <= c {
                3
            } else {
                5
            }
        };

        counts[idx] += 1;
        total_windows += 1;
    }

    let max_entropy = 6.0_f64.ln();
    let mut shannon_entropy = 0.0;
    let total = total_windows as f64;

    for &count in &counts {
        if count > 0 {
            let p = count as f64 / total;
            shannon_entropy -= p * p.ln();
        }
    }

    shannon_entropy / max_entropy
}

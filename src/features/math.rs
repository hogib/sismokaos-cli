use std::collections::HashMap;

/// Helper function to calculate variance
fn variance(data: &[f64]) -> f64 {
    if data.is_empty() {
        return f64::NAN;
    }
    let n = data.len() as f64;
    let mean = data.iter().sum::<f64>() / n;
    data.iter().map(|&x| (x - mean).powi(2)).sum::<f64>() / n
}

/// Helper function to calculate moments (mean, variance, skewness num, kurtosis num)
fn compute_moments(segment: &[f64]) -> (f64, f64, f64, f64) {
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

pub fn compute_skewness(segment: &[f64]) -> f64 {
    if segment.len() < 3 {
        return f64::NAN;
    }
    let (_, var, m3, _) = compute_moments(segment);
    if var == 0.0 {
        return f64::NAN;
    }
    m3 / var.powf(1.5)
}

pub fn compute_kurtosis(segment: &[f64]) -> f64 {
    if segment.len() < 4 {
        return f64::NAN;
    }
    let (_, var, _, m4) = compute_moments(segment);
    if var == 0.0 {
        return f64::NAN;
    }
    m4 / (var * var)
}

pub fn compute_zcr(segment: &[f64]) -> f64 {
    if segment.len() < 2 {
        return f64::NAN;
    }
    let crossings = segment
        .windows(2)
        .filter(|w| {
            let s1 = if w[0] == 0.0 { 1.0 } else { w[0].signum() };
            let s2 = if w[1] == 0.0 { 1.0 } else { w[1].signum() };
            s1 != s2
        })
        .count();

    crossings as f64 / (segment.len() - 1) as f64
}

pub fn compute_sta_lta(segment: &[f64], nsta: usize, nlta: usize) -> (f64, f64, f64) {
    if segment.len() <= nlta || nsta == 0 || nlta == 0 {
        return (f64::NAN, f64::NAN, f64::NAN);
    }

    let data_sq: Vec<f64> = segment.iter().map(|&x| x * x).collect();
    let mut cft_valid = Vec::with_capacity(segment.len() - nlta + 1);

    let mut lta_sum: f64 = data_sq[0..nlta].iter().sum();
    let mut sta_sum: f64 = data_sq[(nlta - nsta)..nlta].iter().sum();

    cft_valid.push((sta_sum / nsta as f64) / (lta_sum / nlta as f64));

    for i in nlta..segment.len() {
        lta_sum += data_sq[i] - data_sq[i - nlta];
        sta_sum += data_sq[i] - data_sq[i - nsta];

        let sta = sta_sum / nsta as f64;
        let lta = lta_sum / nlta as f64;

        let ratio = if lta == 0.0 { 0.0 } else { sta / lta };
        cft_valid.push(ratio);
    }

    let max = cft_valid
        .iter()
        .copied()
        .max_by(f64::total_cmp)
        .unwrap_or(f64::NAN);
    let mean = cft_valid.iter().sum::<f64>() / cft_valid.len() as f64;

    cft_valid.sort_by(f64::total_cmp);
    let median = if cft_valid.len() % 2 == 0 {
        (cft_valid[cft_valid.len() / 2 - 1] + cft_valid[cft_valid.len() / 2]) / 2.0
    } else {
        cft_valid[cft_valid.len() / 2]
    };

    (max, mean, median)
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

    let centroid = freqs
        .iter()
        .zip(power_spectrum.iter())
        .map(|(&f, &p)| f * p)
        .sum::<f64>()
        / total_power;
    let lf_energy = freqs
        .iter()
        .zip(power_spectrum.iter())
        .filter(|&(&f, _)| f <= lf_hf_cutoff)
        .map(|(_, &p)| p)
        .sum::<f64>()
        / total_power;
    let hf_energy = freqs
        .iter()
        .zip(power_spectrum.iter())
        .filter(|&(&f, _)| f > lf_hf_cutoff)
        .map(|(_, &p)| p)
        .sum::<f64>()
        / total_power;

    (centroid, lf_energy, hf_energy)
}

pub fn compute_hjorth_parameters(segment: &[f64]) -> (f64, f64, f64) {
    if segment.len() < 3 {
        return (f64::NAN, f64::NAN, f64::NAN);
    }

    let var_x = variance(segment);
    let x_prime: Vec<f64> = segment.windows(2).map(|w| w[1] - w[0]).collect();
    let var_x_prime = variance(&x_prime);
    let x_bis: Vec<f64> = x_prime.windows(2).map(|w| w[1] - w[0]).collect();
    let var_x_bis = variance(&x_bis);

    let activity = var_x;
    let mobility = if var_x != 0.0 {
        (var_x_prime / var_x).sqrt()
    } else {
        0.0
    };
    let complexity = if var_x_prime == 0.0 || mobility == 0.0 {
        0.0
    } else {
        ((var_x_bis / var_x_prime).sqrt()) / mobility
    };

    (activity, mobility, complexity)
}

pub fn compute_permutation_entropy(segment: &[f64]) -> f64 {
    let d = 3;
    let tau = 1;
    if segment.len() < d * tau {
        return f64::NAN;
    }

    let mut counts: HashMap<Vec<usize>, usize> = HashMap::new();
    let mut total_windows = 0;

    for window in segment.windows(d) {
        let mut indexed_window: Vec<(usize, f64)> = window.iter().copied().enumerate().collect();
        indexed_window.sort_by(|a, b| a.1.total_cmp(&b.1));
        let permutation: Vec<usize> = indexed_window.into_iter().map(|(i, _)| i).collect();
        *counts.entry(permutation).or_insert(0) += 1;
        total_windows += 1;
    }

    let max_entropy = 6.0_f64.ln();
    let mut shannon_entropy = 0.0;
    let total = total_windows as f64;

    for &count in counts.values() {
        let p = count as f64 / total;
        if p > 0.0 {
            shannon_entropy -= p * p.ln();
        }
    }

    shannon_entropy / max_entropy
}

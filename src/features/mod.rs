pub mod cross;
pub mod math;

use crate::config::AppConfig;
use rustfft::{FftPlanner, num_complex::Complex};
use std::collections::HashMap;

pub fn compute_window(
    e_seg: &[f64],
    n_seg: &[f64],
    z_seg: &[f64],
    config: &AppConfig,
) -> HashMap<String, f64> {
    let mut result = HashMap::new();

    // Cross-channel features
    result.insert(
        "EN_CROSS_CORR".to_string(),
        cross::compute_cross_correlation(e_seg, n_seg),
    );
    result.insert(
        "EZ_CROSS_CORR".to_string(),
        cross::compute_cross_correlation(e_seg, z_seg),
    );
    result.insert(
        "NZ_CROSS_CORR".to_string(),
        cross::compute_cross_correlation(n_seg, z_seg),
    );

    let components = [("E", e_seg), ("N", n_seg), ("Z", z_seg)];
    let win_size_f = config.win_size as f64;

    // Set up FFT planner (we only need the positive frequencies up to Nyquist)
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(config.win_size);
    let num_freqs = config.win_size / 2 + 1;
    let freqs: Vec<f64> = (0..num_freqs)
        .map(|i| i as f64 * config.fs / config.win_size as f64)
        .collect();

    for (comp, seg) in components {
        // Base case: NaNs for everything
        if seg.iter().any(|&x| x.is_nan()) {
            continue;
        }

        // Standardize the segment for STA/LTA and FFT
        let mean = seg.iter().sum::<f64>() / win_size_f;
        let variance = seg.iter().map(|&x| (x - mean).powi(2)).sum::<f64>() / (win_size_f - 1.0);
        let std_dev = variance.sqrt();

        let seg_norm: Vec<f64> = if std_dev != 0.0 {
            seg.iter().map(|&x| (x - mean) / std_dev).collect()
        } else {
            seg.to_vec()
        };

        // Compute FFT & Power Spectrum
        let mut buffer: Vec<Complex<f64>> = seg_norm
            .iter()
            .map(|&x| Complex { re: x, im: 0.0 })
            .collect();
        fft.process(&mut buffer);
        let power_spectrum: Vec<f64> = buffer
            .iter()
            .take(num_freqs)
            .map(|c| c.norm_sqr() / win_size_f)
            .collect();

        // --- Calculate Features ---
        let (sta_max, sta_mean, sta_median) = math::compute_sta_lta(
            &seg_norm,
            (config.sta_sec * config.fs) as usize,
            config.lta_sec as usize * config.fs as usize,
        );
        result.insert(format!("{}_STA_LTA_Max", comp), sta_max);
        result.insert(format!("{}_STA_LTA_Mean", comp), sta_mean);
        result.insert(format!("{}_STA_LTA_Median", comp), sta_median);

        result.insert(format!("{}_PEAK", comp), math::peak(seg));
        result.insert(format!("{}_RMS", comp), math::compute_rms(seg));
        result.insert(format!("{}_SKEWNESS", comp), math::compute_skewness(seg));
        result.insert(format!("{}_KURTOSIS", comp), math::compute_kurtosis(seg));
        result.insert(format!("{}_ZCR", comp), math::compute_zcr(seg));

        let dom_freq = math::dominant_frequency(&power_spectrum, &freqs);
        result.insert(format!("{}_DOMINANT_FREQ", comp), dom_freq);

        let (centroid, lf_e, hf_e) =
            math::spectral_centroid_and_energy(&freqs, &power_spectrum, 1.0);
        result.insert(format!("{}_SPECTRAL_CENTROID", comp), centroid);
        result.insert(format!("{}_LOW_FREQ_ENERGY", comp), lf_e);
        result.insert(format!("{}_HIGH_FREQ_ENERGY", comp), hf_e);

        let (act, mob, cx) = math::compute_hjorth_parameters(seg);
        result.insert(format!("{}_HJORTH_ACTIVITY", comp), act);
        result.insert(format!("{}_HJORTH_MOBILITY", comp), mob);
        result.insert(format!("{}_HJORTH_COMPLEXITY", comp), cx);

        result.insert(
            format!("{}_PERMUTATION_ENTROPY", comp),
            math::compute_permutation_entropy(seg),
        );
    }

    result
}

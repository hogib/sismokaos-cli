pub mod chaos;
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
) -> HashMap<&'static str, f64> {
    let mut result = HashMap::new();

    // Cross-channel features
    result.insert(
        "EN_CROSS_CORR",
        cross::compute_cross_correlation(e_seg, n_seg),
    );
    result.insert(
        "EZ_CROSS_CORR",
        cross::compute_cross_correlation(e_seg, z_seg),
    );
    result.insert(
        "NZ_CROSS_CORR",
        cross::compute_cross_correlation(n_seg, z_seg),
    );

    // Map component letters to their static key prefixes to avoid formatting
    let components = [
        (
            e_seg,
            [
                "E_STA_LTA_Max",
                "E_STA_LTA_Mean",
                "E_STA_LTA_Median",
                "E_PEAK",
                "E_RMS",
                "E_SKEWNESS",
                "E_KURTOSIS",
                "E_ZCR",
                "E_DOMINANT_FREQ",
                "E_SPECTRAL_CENTROID",
                "E_LOW_FREQ_ENERGY",
                "E_HIGH_FREQ_ENERGY",
                "E_HJORTH_ACTIVITY",
                "E_HJORTH_MOBILITY",
                "E_HJORTH_COMPLEXITY",
                "E_PERMUTATION_ENTROPY",
                "E_WOLF_LYE",
                "E_ROS_SHORT",
                "E_ROS_LONG",
                "E_SAMP_ENT",
                "E_CORR_DIM",
            ],
        ),
        (
            n_seg,
            [
                "N_STA_LTA_Max",
                "N_STA_LTA_Mean",
                "N_STA_LTA_Median",
                "N_PEAK",
                "N_RMS",
                "N_SKEWNESS",
                "N_KURTOSIS",
                "N_ZCR",
                "N_DOMINANT_FREQ",
                "N_SPECTRAL_CENTROID",
                "N_LOW_FREQ_ENERGY",
                "N_HIGH_FREQ_ENERGY",
                "N_HJORTH_ACTIVITY",
                "N_HJORTH_MOBILITY",
                "N_HJORTH_COMPLEXITY",
                "N_PERMUTATION_ENTROPY",
                "N_WOLF_LYE",
                "N_ROS_SHORT",
                "N_ROS_LONG",
                "N_SAMP_ENT",
                "N_CORR_DIM",
            ],
        ),
        (
            z_seg,
            [
                "Z_STA_LTA_Max",
                "Z_STA_LTA_Mean",
                "Z_STA_LTA_Median",
                "Z_PEAK",
                "Z_RMS",
                "Z_SKEWNESS",
                "Z_KURTOSIS",
                "Z_ZCR",
                "Z_DOMINANT_FREQ",
                "Z_SPECTRAL_CENTROID",
                "Z_LOW_FREQ_ENERGY",
                "Z_HIGH_FREQ_ENERGY",
                "Z_HJORTH_ACTIVITY",
                "Z_HJORTH_MOBILITY",
                "Z_HJORTH_COMPLEXITY",
                "Z_PERMUTATION_ENTROPY",
                "Z_WOLF_LYE",
                "Z_ROS_SHORT",
                "Z_ROS_LONG",
                "Z_SAMP_ENT",
                "Z_CORR_DIM",
            ],
        ),
    ];

    let win_size_f = config.win_size as f64;
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(config.win_size);
    let num_freqs = config.win_size / 2 + 1;
    let freqs: Vec<f64> = (0..num_freqs)
        .map(|i| i as f64 * config.fs / config.win_size as f64)
        .collect();

    for (seg, keys) in components {
        if seg.iter().any(|&x| x.is_nan()) {
            continue;
        }

        let (mean, var, m3, m4) = math::compute_moments(seg);
        let std_dev = var.sqrt();
        let skewness = if var > 0.0 {
            m3 / var.powf(1.5)
        } else {
            f64::NAN
        };
        let kurtosis = if var > 0.0 {
            m4 / (var * var)
        } else {
            f64::NAN
        };

        let seg_norm: Vec<f64> = if std_dev > 0.0 {
            seg.iter().map(|&x| (x - mean) / std_dev).collect()
        } else {
            seg.to_vec()
        };

        // --- Compute FFT & Power Spectrum ---
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

        let dom_freq = math::dominant_frequency(&power_spectrum, &freqs);
        let (centroid, lf_e, hf_e) =
            math::spectral_centroid_and_energy(&freqs, &power_spectrum, 1.0);
        let (act, mob, cx) = math::compute_hjorth_parameters(seg);
        let entropy = math::compute_permutation_entropy(seg);

        // Map computed values to their static string keys
        result.insert(keys[0], sta_max);
        result.insert(keys[1], sta_mean);
        result.insert(keys[2], sta_median);
        result.insert(keys[3], math::peak(seg));
        result.insert(keys[4], math::compute_rms(seg));
        result.insert(keys[5], skewness);
        result.insert(keys[6], kurtosis);
        result.insert(keys[7], math::compute_zcr(seg));
        result.insert(keys[8], dom_freq);
        result.insert(keys[9], centroid);
        result.insert(keys[10], lf_e);
        result.insert(keys[11], hf_e);
        result.insert(keys[12], act);
        result.insert(keys[13], mob);
        result.insert(keys[14], cx);
        result.insert(keys[15], entropy);

        // --- Chaotic metrics (Wolf/Rosenstein Lyapunov exponents, Sample
        // Entropy, Correlation Dimension) ---
        // Wolf/Rosenstein need enough samples to embed and evolve reliably;
        // below that threshold they report NaN rather than a noisy estimate,
        // matching the Python sibling project's CHAOS_MIN_SAMPLES guard.
        let wolf_lye = if seg_norm.len() >= config.chaos_min_samples {
            chaos::wolf_lye(&seg_norm, config.fs, config.chaos_tau, config.chaos_dim, config.chaos_evolve)
        } else {
            f64::NAN
        };
        let (ros_short, ros_long) = if seg_norm.len() >= config.chaos_min_samples {
            chaos::rosenstein_lye(
                &seg_norm,
                config.fs,
                config.chaos_tau,
                config.chaos_dim,
                config.chaos_slope_ros,
                config.chaos_mean_period,
            )
        } else {
            (f64::NAN, f64::NAN)
        };
        let samp_ent = chaos::sample_entropy(&seg_norm, config.chaos_sampent_m, config.chaos_sampent_r);
        let corr_dim = chaos::correlation_dimension(&seg_norm, config.chaos_tau, config.chaos_dim);

        result.insert(keys[16], wolf_lye);
        result.insert(keys[17], ros_short);
        result.insert(keys[18], ros_long);
        result.insert(keys[19], samp_ent);
        result.insert(keys[20], corr_dim);
    }

    result
}

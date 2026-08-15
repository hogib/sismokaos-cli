use std::collections::HashMap;

pub fn new_feature_map() -> HashMap<String, f64> {
    let mut feature_map = HashMap::new();
    let features = [
        "sta_lta_mean",
        "sta_lta_max",
        "sta_lta_median",
        "peak",
        "low_freq_energy",
        "high_freq_energy",
        "spectral_centroid",
        "dominant_freq",
        "rms",
        "skewness",
        "kurtosis",
        "zcr",
        "hjort_activity",
        "hjort_mobility",
        "hjort_complexity",
        "permutation_entropy",
    ];

    for feature in features {
        feature_map.insert(feature.to_string(), f64::NAN);
    }

    feature_map
}

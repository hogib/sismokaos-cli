pub fn compute_cross_correlation(segment_1: &[f64], segment_2: &[f64]) -> f64 {
    if segment_1.len() != segment_2.len() || segment_1.is_empty() {
        return f64::NAN;
    }

    let n = segment_1.len() as f64;

    // Combining sums in one zip pass helps caching/vectorization slightly
    let (sum1, sum2) = segment_1
        .iter()
        .zip(segment_2.iter())
        .fold((0.0, 0.0), |(s1, s2), (&x, &y)| (s1 + x, s2 + y));

    let mean1 = sum1 / n;
    let mean2 = sum2 / n;

    let mut cov = 0.0;
    let mut var1 = 0.0;
    let mut var2 = 0.0;

    for (&x, &y) in segment_1.iter().zip(segment_2.iter()) {
        let dx = x - mean1;
        let dy = y - mean2;
        cov += dx * dy;
        var1 += dx * dx;
        var2 += dy * dy;
    }

    if var1 == 0.0 || var2 == 0.0 {
        return f64::NAN;
    }

    cov / (var1 * var2).sqrt()
}

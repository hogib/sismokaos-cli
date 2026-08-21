//! Per-function timing for the chaotic metrics, so optimisation effort goes
//! where the time actually is. Run with:
//!   cargo run --release --example chaos_bench
use std::time::Instant;

#[path = "../src/features/chaos.rs"]
mod chaos;

fn main() {
    // A 200 s window at 5 Hz, z-scored, as compute_window would supply.
    let n = 1000usize;
    let mut s = 12345u64;
    let mut x: Vec<f64> = (0..n)
        .map(|_| {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((s >> 33) as f64 / (1u64 << 31) as f64) - 1.0
        })
        .collect();
    let mean = x.iter().sum::<f64>() / n as f64;
    let sd = (x.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n as f64 - 1.0)).sqrt();
    for v in x.iter_mut() { *v = (*v - mean) / sd; }

    let reps = 20;
    let mut bench = |name: &str, f: &dyn Fn() -> f64| {
        let t = Instant::now();
        let mut acc = 0.0;
        for _ in 0..reps { acc += f(); }
        let per = t.elapsed().as_secs_f64() / reps as f64 * 1000.0;
        println!("  {:<22} {:8.3} ms/call   (checksum {:.6})", name, per, acc);
        per
    };

    let mut total = 0.0;
    total += bench("wolf_lye", &|| chaos::wolf_lye(&x, 5.0, 5, 5, 5));
    total += bench("rosenstein_lye", &|| chaos::rosenstein_lye(&x, 5.0, 5, 5, [0.5, 5.0, 5.0, 20.0], 1.0).0);
    total += bench("sample_entropy", &|| chaos::sample_entropy(&x, 2, 0.2));
    total += bench("correlation_dimension", &|| chaos::correlation_dimension(&x, 5, 5));
    println!("  {:<22} {:8.3} ms/call  x3 components = {:.1} ms/window", "TOTAL", total, total * 3.0);
}

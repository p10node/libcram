//! Standalone microbenchmark: rustc -O --edition=2024 bench.rs -o entropy-bench.
#[path = "mod.rs"]
mod entropy;
use std::{hint::black_box, time::Instant};
fn main() {
    let mut random = [0u8; 4096];
    let mut state = 0x12345678u64;
    for b in &mut random {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *b = state as u8;
    }
    let mut half = [0u8; 4096];
    half[2048..].fill(255);
    for (name, data) in [
        ("uniform", [0u8; 4096]),
        ("two-runs", half),
        ("random", random),
    ] {
        for (kernel, f) in [
            ("scalar", entropy::scalar_entropy as fn(&[u8]) -> f64),
            ("dispatch", entropy::shannon_entropy as fn(&[u8]) -> f64),
        ] {
            for _ in 0..1000 {
                black_box(f(black_box(&data)));
            }
            let mut timings = Vec::new();
            for _ in 0..9 {
                let now = Instant::now();
                for _ in 0..100000 {
                    black_box(f(black_box(&data)));
                }
                timings.push(now.elapsed().as_nanos() as f64 / 100000.0);
            }
            timings.sort_by(f64::total_cmp);
            println!(
                "{name} {kernel}: median={:.2} ns/page min={:.2} max={:.2}, H={:.8}",
                timings[4],
                timings[0],
                timings[8],
                f(&data)
            );
        }
    }
}

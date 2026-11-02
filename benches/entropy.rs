#![deny(clippy::unwrap_used)]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use libcram_core::entropy::{entropy_below, scalar_entropy, shannon_entropy};

/// Migration policy threshold: an allocation is compressible below 4.5 bits/byte.
const POLICY_THRESHOLD: f64 = 4.5;

/// Hardware floor for the calibrated entropy SLA: one scattered u16 store per
/// input byte into the production four-bank layout. It has no counter reload,
/// merge or finalization, so it is cheaper than any exact memory-counter
/// histogram. Word extraction matches the production kernel.
fn scatter_store_floor(data: &[u8], banks: &mut [[u16; 256]; 4]) {
    for bytes in data.as_chunks::<8>().0 {
        let word = u64::from_ne_bytes(*bytes);
        // Keep one word load on AArch64, as in production (no per-byte loads).
        // SAFETY: register-only identity; no memory or flags are changed.
        #[cfg(target_arch = "aarch64")]
        let word = unsafe {
            let mut word = word;
            std::arch::asm!("/* {word} */", word = inout(reg) word,
                options(pure, nomem, nostack, preserves_flags));
            word
        };
        for lane in 0..8 {
            banks[lane & 3][((word >> (8 * lane)) & 255) as usize] = lane as u16;
        }
    }
}

fn entropy(c: &mut Criterion) {
    let mut group = c.benchmark_group("entropy");
    for size in [64, 512, 4096] {
        let mut state = 0x9e3779b97f4a7c15_u64;
        let random: Vec<u8> = (0..size)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect();
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(
            BenchmarkId::new("floor/scatter_store", size),
            &random,
            |b, bytes| {
                let mut banks = [[0u16; 256]; 4];
                b.iter(|| {
                    scatter_store_floor(black_box(bytes), &mut banks);
                    black_box(&mut banks);
                });
            },
        );
        let ascii: Vec<u8> = b"libcram memory tier orchestration "
            .iter()
            .copied()
            .cycle()
            .take(size)
            .collect();
        for (label, data) in [
            ("zero", vec![0; size]),
            ("ascii", ascii),
            ("random", random),
        ] {
            group.throughput(Throughput::Bytes(size as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("dispatch/{label}"), size),
                &data,
                |b, bytes| {
                    b.iter(|| black_box(shannon_entropy(black_box(bytes))));
                },
            );
            group.bench_with_input(
                BenchmarkId::new(format!("scalar/{label}"), size),
                &data,
                |b, bytes| {
                    b.iter(|| black_box(scalar_entropy(black_box(bytes))));
                },
            );
            group.bench_with_input(
                BenchmarkId::new(format!("verdict/{label}"), size),
                &data,
                |b, bytes| {
                    b.iter(|| black_box(entropy_below(black_box(bytes), POLICY_THRESHOLD)));
                },
            );
        }
        // Diagnostic worst case for the verdict: 23 equiprobable symbols give
        // entropy close to the threshold, so no prefix can decide it.
        let mut state = 0x2545f4914f6cdd1d_u64;
        let mid: Vec<u8> = (0..size)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state % 23) as u8
            })
            .collect();
        group.bench_with_input(BenchmarkId::new("verdict/mid", size), &mid, |b, bytes| {
            b.iter(|| black_box(entropy_below(black_box(bytes), POLICY_THRESHOLD)));
        });
        group.bench_with_input(
            BenchmarkId::new("diagnostic/mid", size),
            &mid,
            |b, bytes| {
                b.iter(|| black_box(shannon_entropy(black_box(bytes))));
            },
        );
    }
    group.finish();
}

criterion_group!(benches, entropy);
criterion_main!(benches);

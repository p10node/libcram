#![deny(clippy::unwrap_used)]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use libcram_core::entropy::{scalar_entropy, shannon_entropy};

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
        }
    }
    group.finish();
}

criterion_group!(benches, entropy);
criterion_main!(benches);

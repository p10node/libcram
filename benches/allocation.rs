#![deny(clippy::unwrap_used)]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use libcram_core::allocator::{AccessHint, CramAllocator};

fn allocation(c: &mut Criterion) {
    let allocator = CramAllocator::new().expect("benchmark allocator initialization");
    let mut group = c.benchmark_group("allocation_roundtrip");
    for size in [64, 4096, 65536, 1048576] {
        group.bench_with_input(BenchmarkId::new("cram_core", size), &size, |b, &size| {
            b.iter(|| {
                let pointer = allocator
                    .allocate(black_box(size), 64, AccessHint::Auto)
                    .expect("benchmark allocation");
                black_box(pointer);
                // SAFETY: This iteration exclusively owns the live allocation and has no aliases.
                unsafe { allocator.deallocate(pointer) }.expect("benchmark deallocation");
            });
        });
        group.bench_with_input(BenchmarkId::new("cram_abi", size), &size, |b, &size| {
            assert_eq!(cram::cram_init(), cram::CRAM_OK);
            b.iter(|| {
                let pointer = cram::cram_alloc(black_box(size), 64, AccessHint::Auto as u32);
                assert!(!pointer.is_null(), "benchmark C ABI allocation failed");
                black_box(pointer);
                // SAFETY: The allocation is live and exclusively owned by this iteration.
                assert_eq!(unsafe { cram::cram_free(pointer) }, cram::CRAM_OK);
            });
        });
        group.bench_with_input(BenchmarkId::new("malloc", size), &size, |b, &size| {
            b.iter(|| {
                // SAFETY: malloc returns a fresh block; free uses exactly that pointer once.
                unsafe {
                    let pointer = libc::malloc(black_box(size));
                    assert!(!pointer.is_null(), "benchmark malloc failed");
                    black_box(pointer);
                    libc::free(pointer);
                }
            });
        });
    }
    group.finish();
}

criterion_group!(benches, allocation);
criterion_main!(benches);

# AArch64 entropy bottleneck investigation

## Verdict: PARTIAL — bottleneck identified, latency target still unmet

The committed implementation and the strict 1000 ns acceptance gate are unchanged.
The investigation is a diagnostic, not a replacement for Criterion acceptance.

## Reproduce

On an AArch64 host with NEON, Python 3 and rustc:

```sh
python3 docs/performance/entropy-costs.py
```

This compiles a temporary copy of the actual entropy module with two diagnostic
entry points. It measures histogram construction, final entropy calculation and
the existing public entry point separately. The 4096-byte zero/ASCII/xorshift
inputs match the earlier microbenchmark, with scalar equality checked before
measurement. Warm-up is 10000 calls, followed by nine batches of 100000 calls.
The statistic is the median batch time per call, not a confidence bound.
Temporary source and binaries are removed after execution. No library API or
production source is modified by this diagnostic.

## Recorded component measurements

Apple M1 Pro; rustc 1.99.0; `rustc --edition=2021 -O` without CPU-specific flags.
See `../evidence/2026-10-09/entropy-cost-breakdown.log` for source hash and output.

| Input | Histogram median, ns | Finish median, ns | Public call median, ns |
|---|---:|---:|---:|
| Zero | 88.58 | 64.90 | 151.34 |
| ASCII | 1107.10 | 64.23 | 1198.36 |
| Random | 1079.78 | 64.44 | 1126.38 |

Component calls have different boundaries and black-box operations, so their
medians must not be treated as an exact additive decomposition. Nevertheless,
the histogram by itself exceeds the full-call budget for ASCII and random.
Optimizing only logarithm/weight-table finalization cannot close that gap here.

## Rejected experimental changes

Standalone copies tested counter-bank strides 257, 264 and 272, explicit
four/eight-word unrolling, and a transposed counter layout. None met the strict
1000 ns target for both ASCII and random. Eight-word unrolling measured about
1134.50 ns for ASCII and 1058.82 ns for random in its diagnostic run; transposing
counters was substantially slower. These are exploratory medians, not acceptance
measurements, and have not been integrated into the library.

Generated assembly confirms scalar load/increment/store operations inside the
histogram loop and SIMD reduction when merging the lane histograms. The next
algorithmic investigation should target histogram throughput, not add more
microbenchmarks of the already-small finalization stage.

## Remaining deployment input

Production acceptance needs an authorized native Linux test machine with the
intended CRAM NUMA driver and physical occupancy source, an Intel PMU with verified
PEBS load/store events, and permission for BPF/perf/memory-policy operations on
owned test mappings. No such machine is available in the current environment.
The existing Docker refusal and missing PMU/CRAM remain valid blockers; no host
security settings were changed. A native Linux performance run is also needed:
M1 measurements cannot establish an x86 deployment's latency or overhead.

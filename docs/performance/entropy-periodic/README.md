# Exact short-period histogram follow-up

## Result: partial B improvement, strict gate still OPEN

Later: gate B was redefined on the exact verdict and passes; see [IMPLEMENTATION_STATUS.md](../../IMPLEMENTATION_STATUS.md).

The production banked histogram now has a bounded short-period scout. It takes
only the first recurrence of the input's first byte within offsets 1–64 as a
candidate, then compares **every** byte against its period-shifted counterpart,
including any partial tail. Only after equality succeeds are whole repetitions
counted with exact integer multiplicities. The tail is counted normally.
There is no retained input state, fingerprint or benchmark-content constant.
The existing entropy finalization, mathematical weights, ISA selection and
uniform-prefix handling are unchanged.

A missed candidate is safe: e.g. a first-byte recurrence inside a longer genuine
period can cause the scout to decline and use the ordinary histogram. This is
not an exhaustive period finder. Random/nonperiodic inputs pay a small scout
cost; random remains the bottleneck. Input must still be owned/synchronized as
required by the existing API; this does not authorize reading concurrently
written application memory.

## Unchanged official Criterion gate

Upper bound of the mean confidence interval, ns per 4096-byte dispatch call:

| Machine | zero | ASCII | random | Strict overall result |
| --- | ---: | ---: | ---: | --- |
| Apple M1 Pro/macOS | 152.884099 | 242.730530 | 1080.987290 | FAIL: random |
| Linux x86_64 owned NUMA VM | 243.575357 | 292.325304 | 2972.236380 | FAIL: random |

Both actual benchmarks exited 0; both unchanged `entropy_budget.py` gates exited
1. Threshold remains **strictly below 1000 ns**, using all three distributions.
The official ASCII generator's runtime-discovered period is 33, not 64; its
source and the random seed were not changed. Linux is pinned to guest CPU 0 on
an owned two-DRAM-node VM on a shared Broadwell-X host, not dedicated bare metal
or CRAM. Historical pre-change bounds live in `../entropy-update.md`; they are
not a contemporaneous paired confidence-interval comparison.

## Correctness and review

- Actual RED: new period helper returned zero for a repeated input and failed
  `0 != 1` at length 128 (`red.log`); implemented helper then passed.
- Period-detection test covers widths 1–64, offsets 0–63, 15 lengths through
  131073 bytes, each one-byte corruption across a 4097-byte period-33 input,
  an ambiguous first recurrence and a period outside the scout window.
- Direct histogram differential covers nonzero initial counters, offsets/tails,
  large lengths and each single-byte corruption. Entropy remains bit-for-bit
  equal to the scalar oracle. Existing tests were retained.
- macOS `cargo test --workspace --locked`: 88 passing tests; Linux: 89 passing
  tests. These are default-feature workspace runs, not the earlier SDK matrix.
  All-target/all-feature Clippy with `-D warnings` passed on both machines.
- Three isolated mutations compiled successfully and failed the named tests:
  compare only the first 128 bytes, discard the tail, or omit repeat weighting.
  Baseline/restored copies pass; canonical source hashes stayed unchanged.
- Independent read-only review of the three source/test files found no blocker;
  `review.md` contains exact hashes. It supersedes the preliminary fixed-64
  review and is not a whole-project or performance certification.

`source-sha256.json` identifies the reviewed production files, benchmark, gate
and lockfile. Linux source was transferred into a newly owned source directory,
verified against a complete manifest before running, and did not overwrite the
existing VM checkout. Existing SDK archives from `3edfa98` do **not** include this
production follow-up; no new bindings/SDK or hardware acceptance is claimed.

## Reproduce the shipped candidate

From the repository root, use a fresh Cargo target directory to avoid stale
Criterion records:

```sh
export CARGO_TARGET_DIR="$PWD/target/entropy-periodic-replay"
cargo test --workspace --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo bench --locked --bench entropy -- 'entropy/dispatch/.*/4096'
python3 scripts/entropy_budget.py --sla strict "$CARGO_TARGET_DIR/criterion"
python3 scripts/verify_entropy_periodic_mutations.py
```

The Linux benchmark used `taskset -c 0 cargo bench ...` and Rust 1.89.0.
The mutation runner uses temporary copies and requires actual named tests to run;
compilation errors or missing tests cannot count as successful negative controls.

## Exploration records and limitations

The first throwaway fixed-64 experiment used a distinct period-32 ASCII fixture
and a different random seed. Its approximately 180 ns ASCII result was **not**
official acceptance. The candidate was not promoted. After reading the actual
benchmark generators, the variable-period experiment used those unchanged
sequences and was followed by the official Criterion runs above.

Both diagnostic logs and their historical machine-local Python generators are
retained. Those generators assumed the old base source at `01f794f` and scratch
layout; they are historical records, **not relocatable replay entrypoints for
current main**. Use the Cargo commands above to reproduce the shipped path.
Diagnostic medians are not substituted for the confidence-bound gate. No entropy
approximation, cached-input result, benchmark change or relaxed threshold was
introduced. A–E remains incomplete: random latency, C all-load/total CPU, and
real CRAM/long-duration production acceptance are still open.

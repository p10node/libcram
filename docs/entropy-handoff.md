# Entropy worker handoff

Branch: `task/entropy-budget`; base: `f43fb9e7c64e5177c6e4193112c94c63d6e6f0d1`.
Worktree: `$WORKSPACE/libcram-entropy`.

## Status: PARTIAL, strict target not met

Later: gate B was redefined on the exact verdict and passes; see
[IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md).

* Implemented a shared exact four-bank u16 histogram, 32-byte chunks and bounded
  32768-byte batches. NEON uses it after the unchanged uniform-prefix scan;
  AVX2/AVX512BW now use prefix-only uniform detection and the same helper.
* No approximations, input caching, changed distributions, budget changes, or
  entropy finalization changes. Rust 1.89 AArch64 check passes.
* M1 Pro frozen Criterion upper means (ns): zero **163.5655474418708**, ASCII
  **1129.7400844157053**, random **1120.7620133141659**. Gate exit **1**.
  Baseline: 155.50913324287183 / 1165.2066071892564 / 1127.5274222925211.
* Criterion calls ASCII improved (~3%); random remains within its noise threshold.
  Small-input paired diagnostics show ~20–21% improvement at 64 bytes and ~13–15%
  at 512 bytes for mixed inputs, but are not acceptance statistics.
* No claim of sub-1000 ns performance or impossibility. The physical target
  remains open; the histogram still dominates. No benchmarks remain running.

## Verification

A new additive-count differential test first failed with an empty helper at
`pattern=0, offset=0, len=1` (exit 101), then passed with the real implementation.
It covers collisions, skew, all byte values, unaligned tails, batch overflow
boundaries and input lengths through 131073. Existing exhaustive entropy
comparison tests passed unchanged.

Passed:

```sh
cargo test --locked -p libcram-core entropy
cargo test --locked --release -p libcram-core entropy
cargo clippy --locked -p libcram-core --all-targets -- -D warnings
cargo +1.89.0 check --locked -p libcram-core
cargo check --locked -p libcram-core --target x86_64-unknown-linux-gnu
```

Both test profiles passed four entropy unit tests plus the matching migration
integration test. `git diff --check` passes. Source, benchmark, budget script and
lockfile hashes checked unchanged across the final Criterion run.

## Parent-owned x86 follow-up

The x86 path was compile-checked, **not executed or measured**. A local x86 Apple
binary could not execute (Bad CPU type); no host configuration or remote system
was changed. Parent must run the existing differential tests and unchanged
Criterion command on s5 before treating the x86 code as an established speedup:

```sh
cargo test --locked --release -p libcram-core entropy
cargo clippy --locked -p libcram-core --all-targets -- -D warnings
cargo bench --locked --bench entropy -- 'entropy/dispatch/.*/4096'
python3 scripts/entropy_budget.py --sla strict target/criterion
```

Do not infer VM results from ARM results. The helper is portable Rust; its only
identity assembly is cfg-gated to AArch64. AVX512BW still needs execution on
hardware advertising that ISA; Broadwell alone cannot provide this evidence.

## Files and preservation

Production/test files:

* `crates/libcram-core/src/entropy/mod.rs`
* `crates/libcram-core/src/entropy/differential.rs`

Evidence: `docs/performance/entropy-next.md` contains exact Criterion metadata and
estimates, file hashes, diagnostic results, rejected experiments and the actual
scratch generator sources. This file is the concise handoff.

Before cleaning the worktree, copy `target/entropy-evidence.tar.gz` (ignored):
SHA-256 `1151cd156fcb1385931c1b07e89d9955f9db3df576301a9180bfeedf823c158a`.
It contains both full Criterion trees, RED/GREEN/build/benchmark logs, gate JSON,
source hashes and experiment drivers. Standalone generated variants additionally
remain in `target/entropy-experiments/`. No canonical/sibling/remote writes were
performed by this worker.

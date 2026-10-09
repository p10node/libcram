# Fused histogram finalization spike

## Question

Can avoiding the intermediate usize histogram lower exact 4 KiB Shannon latency?
The prior bank-count/counter-width work retained that intermediate histogram.
This experiment instead sums bank counters directly into the unchanged four
floating-point accumulators. It changes neither data nor the entropy formula.

## Verdict: PARTIAL — not production or strict-budget acceptance

Scalar fusion is slower. Four-bank NEON fusion shows a small diagnostic gain,
but ASCII still exceeds 1000 ns. Eight-bank NEON fusion does not improve enough
to retain. No candidate was integrated into production; no x86 speedup is claimed.

Apple M1 Pro, aarch64-apple-darwin, rustc 1.99.0 / LLVM 23.1.1.
Frozen production entropy source SHA-256:
`fb05adbe76e08f068e0acb2d4691070d76cedec332dd0f02c666901390303709`.

## Final comparison

Median of nine diagnostic batches, nanoseconds per call:

| Variant | Zero | ASCII | Random |
| --- | ---: | ---: | ---: |
| Production baseline | 146.29 | 1113.39 | 1030.34 |
| Four-bank scalar fusion | 146.92 | 1152.96 | 1067.52 |
| Four-bank NEON fusion | 151.02 | 1068.69 | 1004.60 |
| Eight-bank NEON fusion | 147.36 | 1106.37 | 1032.17 |

Every batch performs 50000 black-boxed calls, after 5000 warm-up calls per
input/function; function ordering rotates each round. Inputs exactly reproduce
`benches/entropy.rs` at 4096 bytes, including its random seed. No input results
are cached. Mathematical count weights use the same formula as production.
Uniform-prefix, small and >4096-byte inputs fall back to the original function.

Three successive experiment builds are retained: scalar fusion, addition of
four-bank NEON fusion, then addition of eight-bank NEON fusion. The first NEON
comparison had random medians below 1000 ns, but that is neither a confidence
bound nor permission to discard the later above-budget result. These numbers
are not Criterion measurements; the original strict acceptance result remains
FAIL. No threshold or benchmark file was changed.

## Correctness and scope

Each candidate was compared bit-for-bit with scalar entropy for 2560 inputs:
five patterns, 32 alignments and 16 lengths from empty through 4097 bytes.
All comparisons passed. This is exploratory coverage, not the full production
adversarial suite or RED/GREEN production implementation. The final relocatable
replay wrapper also compiled and passed its check-only mode, exit 0.

The NEON merge performs bounded eight-lane u16 loads. Candidate inputs are at
most 4096 bytes, so all bank sums fit u16. It preserves the production four-sum
floating-point association. The scratch helper uses Apple-host NEON only; the
wrapper rejects other platforms rather than pretending it was measured there.

## Reproduce

From this repository on Apple Silicon with the pinned baseline source:

```sh
python3 docs/performance/entropy-fused-spike/experiment.py --check-only
python3 docs/performance/entropy-fused-spike/experiment.py
```

Generated source/binary/output go to ignored `target/entropy-fused-replay/`, not
this historical evidence directory. The script rejects a changed production
source hash. `recorded-build.py` and `compiled-main.rs` preserve the original
machine-local benchmark build; use experiment.py for replay. Logs and checksums
are alongside this report. No simultaneous benchmark was launched by this run.

## Recommendation

Do not add a separate production NEON entropy path merely for this small pilot
gain while both required distributions still miss the target. Investigate a
materially faster histogram construction method before spending another complete
cross-platform integration cycle. A future candidate still needs full exactness
regressions and unchanged Criterion gates on M1 and x86; this spike closes only
the question about simple finalization fusion, not B or A–E.

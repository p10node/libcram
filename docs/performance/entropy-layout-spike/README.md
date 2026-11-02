# Histogram counter-layout comparison

## Verdict: INVALIDATED for promotion; B remains OPEN

Later: gate B was redefined on the exact verdict and passes; see [IMPLEMENTATION_STATUS.md](../../IMPLEMENTATION_STATUS.md).

Question: can changing the random-path counter layout, without changing Shannon
math or input distributions, reduce the remaining entropy latency?

Three exact alternatives were compared against the current production algorithm:

- `packed4`: four independent 16-bit lanes in a single u64 per symbol.
- `interleaved4`: four u16 counters adjacent within each symbol's row.
- `wide4`: the existing four-bank layout with u32 instead of u16 counters.

None improves the random median on either measured machine. Do not promote these
variants. This result rejects these implementations, not every possible histogram
optimization. No production source or acceptance benchmark has changed.

## Diagnostic results

Median nanoseconds per 4096-byte call across nine fixed-count batches:

| Variant | M1 zero | M1 ASCII | M1 random | x86 VM zero | x86 VM ASCII | x86 VM random |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Baseline | 146.903 | 232.171 | 1045.082 | 253.174 | 294.799 | 2965.020 |
| Packed u64 | 147.302 | 230.962 | 1141.135 | 252.162 | 289.039 | 2987.417 |
| Interleaved u16 | 147.025 | 237.588 | 1211.951 | 250.765 | 295.513 | 3034.249 |
| Four u32 banks | 147.966 | 231.378 | 1073.591 | 253.897 | 290.976 | 3005.196 |

Raw measurements for **every** variant are retained in `measure.log` and
`linux-measure.log`; JSON summaries include min, max and batch count. Inputs use
the unchanged official 4 KiB zero, ASCII and seeded xorshift generators. All four
implementations share one executable per machine, 5000-call warmups and
50000-call measured batches. Execution order rotates across the nine batches.
Both inputs and results are black-boxed. The x86 executable ran inside the
existing owned Linux NUMA VM, pinned to guest CPU 0, with Rust 1.89.0.

These are diagnostic batch medians, **not Criterion confidence bounds**. No new
strict-gate run is claimed. The last official random upper bounds remain
1080.987 ns (M1) and 2972.236 ns (x86 VM), both above the unchanged 1000 ns limit.

## Correctness and limits

The empty-histogram candidate first failed with exit 101 (`stub-red.log`). This
is a deliberate test-first stub failure, not a production regression.
The implemented candidates then passed 665 differential cases each on both
machines, comparing every histogram bin with a scalar oracle and entropy
bit-for-bit with scalar entropy.

Cases combine seven distributions, five alignments and nineteen lengths through
65537 bytes. They include empty/tiny inputs, partial chunks, 32768-byte batch
boundaries, repeated batches, random inputs from two seeds and nearly-uniform
collision-heavy data. Histograms start with nonzero counts to verify additive
semantics. Full-range uniform/periodic fast paths remain unchanged.

For packed counters, each lane receives at most 8192 increments per 32768-byte
batch, so no 16-bit carry crosses a lane. The four lanes are unpacked and added
to the existing histogram before the next batch. Scalar tails remain outside
those counters. The interleaved variant preserves the same u16 bound; u32
widens it without changing the batch size.

This finite differential set is not exhaustive proof. No candidate received
production acceptance or a new full Rust/bindings/SDK suite.

## Replay

From the repository root, with Rust 1.89+ and Python 3:

```
python3 docs/performance/entropy-layout-spike/verify_results.py
python3 docs/performance/entropy-layout-spike/experiment.py --check-only
python3 docs/performance/entropy-layout-spike/experiment.py
```

The verifier checks stored raw results, summaries and artifact hashes without
compilation or timing. `--check-only` runs correctness only; the last command
additionally runs diagnostic timing. The wrapper verifies baseline source and benchmark hashes, refuses
unsupported architectures, and writes generated sources, executable and new logs
under ignored `target/entropy-layout-spike/`. It never edits production files or
opens SSH. Replaying `--stub` intentionally exits nonzero and is not a real
candidate. Run a non-stub invocation afterward to restore scratch sources.

`recorded-source/` preserves the exact generated Rust sources used for the
measurements. The baseline has only an added public histogram diagnostic wrapper;
its production functions are otherwise unchanged. The relocatable experiment
regenerates those sources from the hash-checked baseline. `checksums.json` covers
all retained artifacts except itself. `linux-receipt.json` records the executed
command and removal of the newly created VM directory; no existing VM checkout,
configuration, data or security setting was changed.

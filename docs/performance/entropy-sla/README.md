# Entropy gate B: exact policy verdict, SIMD investigation and calibrated SLA

Date: 2026-11-01. Machines: Apple M1 Pro (macOS, rustc 1.99.1) and the s5 Linux
VM (Xeon E5-2683 v4 Broadwell-EP, rustc 1.89.0). Both machines ran the same
sources for each receipt set. The coarse-bound receipts (`*-coarse-*`) match
the current tree (`source-sha256.txt`). The earlier verdict receipts predate the
coarse bound, and the value receipts predate the verdict (`mod.rs` SHA-256
`f8d8d7bd…`); the counting code is unchanged since.

## Decision

**Gate B now measures what the library actually needs: the exact migration
policy verdict `entropy < 4.5`, computed by `entropy::entropy_below`. It is
bit-for-bit identical to comparing `shannon_entropy` with 4.5. It usually stops
after half the page, and on x86_64 with AVX2 a store-free coarse bound decides
high-entropy pages without any exact count.** The project owner accepted this
definition of gate B on 2026-11-02; the full value is reported for reference.

| Target and SLA             | M1 Pro (3 runs)                   | s5 x86 VM (3 runs)                |
| -------------------------- | --------------------------------- | --------------------------------- |
| Verdict, calibrated (gate) | **PASS**, random 0.76–0.77× floor | **PASS**, random 0.48–0.49× floor |
| Verdict, strict 1000 ns    | **PASS**, random 607–610 ns       | **PASS**, random 797–812 ns       |
| Value, calibrated          | PASS, random 1.33–1.38× floor     | FAIL, random 1.72–1.80× floor     |
| Value, strict 1000 ns      | FAIL                              | FAIL                              |

`just entropy-budget` runs `scripts/entropy_budget.py --target verdict`. Every
run prints both SLAs for both targets.

| SLA        | Rule                                                                                                                           | Exit status    |
| ---------- | ------------------------------------------------------------------------------------------------------------------------------ | -------------- |
| Calibrated | upper mean CI < max(1000 ns, 1.5 × lower mean CI of `floor/scatter_store/4096`) for zero, ASCII and random, same Criterion run | default        |
| Strict     | upper mean CI < 1000 ns for zero, ASCII and random                                                                             | `--sla strict` |

A missing, zero, negative or non-finite floor fails closed, and so does a
missing measurement for the selected target. The budget is never below the
original 1000 ns. `--target value` (the script default) gates
`entropy/dispatch/*`, as before.

`floor/scatter_store/4096` (`benches/entropy.rs`) performs 4096 scattered `u16`
stores into the production four-bank layout. It uses the same random bytes and
word extraction as production. It has no counter reload, merge or finalization.
Any exact histogram that counts in memory must do at least this store work, so
the floor measures what the store path of that machine can sustain during the
same run.

## Exact policy verdict

The only consumer of entropy in the library is the migration policy
(`AccessSample::compressed`), and it needs one bit: is the entropy below 4.5
bits per byte? The full histogram is needed only when the page is close to the
threshold. `entropy::entropy_below(data, threshold)` counts the page in
prefixes (1/2, then 3/4) with the production four-bank kernel. After each
prefix, it bounds the entropy of the *whole* page:

- **Lowest possible entropy:** every unread byte repeats the most frequent
  symbol so far. With counts `c` and `r` unread bytes, the final value of
  `Σ C·log2 C` is a convex function of where the `r` bytes go, so its maximum is
  at a vertex: all `r` on one symbol. The best vertex is the symbol with the
  largest count, because `W(c + r) − W(c)` grows with `c`. This bound is
  attained, not estimated.
- **Highest possible entropy:** the page is a mixture of the prefix (weight
  `q`) and an unknown rest, so `H ≤ q·H(prefix) + (1−q)·log2(min(256, r)) + h(q)`.

If both bounds are on the same side of the threshold by more than 1e-9 bits,
that side is the verdict. The rounding error of the bounds and of `finish` is
below 1e-11 bits (256 terms of at most log2(n) bits per byte, f64 epsilon), so
the margin cannot flip a verdict. Otherwise the whole page is counted and
finalized by the same `finish` as `shannon_entropy`, then compared, so the result
always equals `shannon_entropy(data) < threshold`. Periodic pages, including
uniform pages, are counted from one verified period. Inputs shorter than 256 or
longer than 4096 bytes are counted completely.

For pseudorandom data, the lowest bound after 2048 bytes is ≈4.95 bits, so the
verdict stops at half the page. One set of u16 banks spans every checkpoint, so
no zeroing or scouting is repeated. A checkpoint reads the four banks directly
(vector sum, cached `v·log2 v` table), and `log2` is evaluated only when the
upper bound is needed. If the first checkpoint shows that the 3/4 checkpoint
cannot decide, it is skipped.

Integration (gate A code):

- `migration::Entropy` is either `Value(f64)` (validated to [0, 8]) or
  `Compressible(bool)`. `Entropy::of(page)` returns the exact verdict for
  `COMPRESSIBLE_ENTROPY = 4.5`, the threshold the policy already used.
- `OwnedAllocation::profile` records `Entropy::of(first 4096 bytes)` instead of
  the full value. `observation_token` accepts `impl Into<Entropy>`, so existing
  `f64` callers are unchanged. The C ABI still takes an `f64` value.
- `entropy::shannon_entropy` is unchanged for callers who need the value itself.

Tests (`entropy/verdict.rs`, `tests/migration.rs`):

- The verdict equals `shannon_entropy(data) < t` for 13 lengths (0 to 65537).
- Data covers zero, sequential, periodic text and 12 alphabet sizes.
- It includes geometric skew and the exact worst cases of both bounds: a random
  prefix followed by its most frequent symbol, and a constant prefix followed
  by random bytes.
- Thresholds include `H`, `next_up(H)`, `next_down(H)`, H ± 1e-12…0.1, NaN and
  ±∞.
- Each bound contains every completion tried.
- A profiled verdict drives migration exactly as the value would.

The trade-off is the near-threshold case. The diagnostic `mid` distribution (23
equiprobable symbols, H ≈ 4.52) must be counted completely, and the verdict is
then 8–13% slower than the value (one checkpoint plus finalization). It is
reported, not gated.

### Verdict receipts before the coarse bound (M1 Pro)

Three sequential runs with fresh `CRITERION_HOME` directories:
`criterion-m1-verdict-run{1,2,3}/`, `m1-verdict-gate-run{1,2,3}.json`,
`m1-verdict-bench-run{1,2,3}.log` and `m1-verdict-env.log`. Upper mean CI in ns:

| Run | Floor lower (ns) | Budget (ns) |  Zero | ASCII | Random | Random / floor | Calibrated | Strict |
| --- | ---------------: | ----------: | ----: | ----: | -----: | -------------: | ---------- | ------ |
| 1   |            799.2 |      1198.8 | 215.1 | 239.7 |  614.9 |          0.769 | PASS       | PASS   |
| 2   |            798.8 |      1198.2 | 210.4 | 247.2 |  634.5 |          0.794 | PASS       | PASS   |
| 3   |            802.4 |      1203.7 | 210.5 | 237.5 |  616.6 |          0.768 | PASS       | PASS   |

| Run | Verdict `mid` | Value `mid` | Value random | Value calibrated |
| --- | ------------: | ----------: | -----------: | ---------------- |
| 1   |        1277.2 |      1115.6 |       1093.4 | PASS             |
| 2   |        1262.9 |      1132.9 |       1081.7 | PASS             |
| 3   |        1269.6 |      1183.1 |       1067.2 | PASS             |

### Verdict receipts before the coarse bound (s5 x86 VM)

The same runner as below (manifest-verified copy, `taskset -c 0`, fresh
`CRITERION_HOME`). All 88 `libcram-core` tests passed on the VM with rustc 1.89
(`s5-verdict-cargo-test.log`). Files: `criterion-s5-verdict-run{1,2,3}/`,
`s5-verdict-gate-run{1,2,3}.json`, `s5-verdict-bench-run{1,2,3}.log`,
`s5-verdict-env.log`.

| Run | Floor lower (ns) | Budget (ns) |  Zero | ASCII | Random | Random / floor | Calibrated | Strict |
| --- | ---------------: | ----------: | ----: | ----: | -----: | -------------: | ---------- | ------ |
| 1   |           1641.8 |      2462.8 | 243.7 | 282.6 | 1548.1 |          0.943 | PASS       | FAIL   |
| 2   |           1672.0 |      2507.9 | 245.6 | 285.0 | 1484.2 |          0.888 | PASS       | FAIL   |
| 3   |           1667.7 |      2501.5 | 252.3 | 283.9 | 1493.8 |          0.896 | PASS       | FAIL   |

| Run | Verdict `mid` | Value `mid` | Value random | Value calibrated |
| --- | ------------: | ----------: | -----------: | ---------------- |
| 1   |        3176.4 |      2889.5 |       2934.6 | FAIL             |
| 2   |        3156.3 |      2897.1 |       2837.0 | FAIL             |
| 3   |        3122.5 |      2879.0 |       2859.9 | FAIL             |

The verdict is below the full-histogram store floor on both machines because
it counts only half of a random page. On Broadwell, 2048 bytes still cost
≈1.4 µs, so strict 1000 ns is not met there. These receipts predate the coarse
bound below.

## Store-free coarse bound (x86_64 AVX2)

Why the exact verdict cannot pass strict on Broadwell: its lowest bound needs
about 1850 exactly counted bytes of a random page (≈4.95 bits at 2048), and
every exactly counted byte costs one counter store. Broadwell has one store
port, and the VM core measured 1.94–2.55 GHz (`probes/s5-hybrid-probes.log`),
so the stores alone take 0.7–1.0 µs. The measured checkpoint takes ≈1.5 µs.

The fix needs no exact count at all for high-entropy pages
(`entropy/coarse.rs`):

- Grouping never increases entropy: `H(bytes) ≥ H(g(bytes))` for any `g`.
- Cells are `c32(x) = x >> 3` and `c16(x) = (x >> 3) & 15`. Each c16 cell is
  the union of the two c32 cells that differ only in bit 7.
- Count c32 on a prefix (3/8 of the input in whole 512-byte blocks: 1536 of
  4096 bytes) and c16 on the rest. The completion with the lowest `H(c32)`
  puts each c16 count on the larger c32 cell of its pair, by convexity of
  `x·log2 x`. That value is the exact minimum over completions, so it is a
  lower bound on the entropy of the bytes.
- On the bench page it is 4.671 bits. If the bound is at least
  `threshold + 1e-9`, the verdict is "not compressible". Otherwise the exact
  verdict above runs, so results stay identical to `shannon_entropy < t`.
- A bound from `k` cell counts can never exceed `log2 k` (all of a cell's
  count on one byte value is a valid completion). It therefore needs at least
  23 cells. 16 cells cap at 4 bits; 32 cells on the whole page give 4.99 bits
  but cost twice as much per byte as 16.

Counting without stores: `vpshufb` turns each byte into a one-hot bit of an
8-bit plane (two planes for c16, four for c32 via the sign of the byte).
Harley-Seal carry-save adders count each bit position, with a second
carry-save level across 512-byte blocks. The final levels are reduced by an
8×8 bit transpose and `vpsadbw` instead of `vpmovmskb` + `popcnt`, which
saturated port 0. Two checks only skip work: fewer than 23 c32 cells in the
first 128 bytes, or a prefix whose own c32 entropy is below the threshold.
The order is periodic → coarse (x86_64 with AVX2, 2048–4096 bytes) → staged
exact. Other targets skip the coarse step; the bound is still tested there.

Tests (`entropy/coarse.rs`, `entropy/verdict.rs`):

- AVX2 cell counts equal scalar counts on 9 lengths (2048–4096) of random,
  sequential, constant, small-alphabet, high-bit and worst-case pages.
- The bound never exceeds the entropy, and never certifies `entropy + 1e-6`.
  The worst case (a random prefix, then one c32 cell) reaches it.
- The verdict differential test adds lengths 2047, 2048, 2560, 3413 and 3414.

`probes/hybrid_probe.rs` (`probes/s5-hybrid-probes.log`) shows how the kernel
was chosen (medians, 4096 random bytes unless noted):

| Kernel                                              | Median ns, 3 runs |
| --------------------------------------------------- | ----------------: |
| c16, 4096 B, positional flush per block             |           451–561 |
| c16, 4096 B, second carry-save level                |           411–427 |
| c16, 4096 B, plus transpose and `vpsadbw` (adopted) |           340–361 |
| c32, 4096 B, second carry-save level                |           893–913 |
| c32, 4096 B, plus transpose (adopted)               |           738–760 |
| c32, 4096 B, four shuffles instead of the sign mask |           766–776 |
| c32 on 1536 B + c16 on 2560 B + bound, no transpose |           968–981 |
| c32 on 1536 B + c16 on 2560 B + bound (adopted)     |           636–658 |
| c32 on 2048 B + c16 on 2048 B + bound               |           673–702 |

Per input vector of 32 bytes, c16 costs ≈13 logic operations and c32 ≈28.
The VM sustains 6.2–7.7 such operations per ns. Earlier variants lost time
when LLVM vectorized the `popcnt` of each `vpmovmskb` into `vpinsrd` chains and
when all four c32 planes spilled. Across 2000 `os.urandom` 4 KiB pages the bound
was at least 4.63 bits (mean 4.66), so ordinary random pages take this path.

### Coarse-bound receipts (s5 x86 VM)

Same runner as the other VM receipts (manifest-verified copy, rustc 1.89,
`taskset -c 0`, fresh `CRITERION_HOME`). All 91 `libcram-core` tests passed on
the VM, including the AVX2 cell-equality test (`s5-coarse-cargo-test.log`).
Files: `criterion-s5-coarse-run{1,2,3}/`, `s5-coarse-gate-run{1,2,3}.json`,
`s5-coarse-bench-run{1,2,3}.log`, `s5-coarse-env.log`. Upper mean CI in ns:

| Run | Floor lower (ns) | Budget (ns) |  Zero | ASCII | Random | Random / floor | Calibrated | Strict |
| --- | ---------------: | ----------: | ----: | ----: | -----: | -------------: | ---------- | ------ |
| 1   |           1666.7 |      2500.0 | 247.8 | 290.0 |  800.1 |          0.480 | PASS       | PASS   |
| 2   |           1669.6 |      2504.5 | 249.2 | 294.8 |  811.9 |          0.486 | PASS       | PASS   |
| 3   |           1648.6 |      2473.0 | 247.4 | 286.5 |  796.5 |          0.483 | PASS       | PASS   |

| Run | Verdict `mid` | Value `mid` | Verdict vs value | Value random | Value calibrated |
| --- | ------------: | ----------: | ---------------: | -----------: | ---------------- |
| 1   |        3192.2 |      2892.4 |             +10% |       2874.8 | FAIL             |
| 2   |        3320.8 |      2840.3 |             +17% |       3001.1 | FAIL             |
| 3   |        3166.4 |      2827.3 |             +12% |       2852.1 | FAIL             |

Random dropped from 1484–1548 ns to 797–812 ns, about 190 ns of headroom
under strict 1000 ns. Pages the coarse step cannot decide pay for the part of
it that ran before the exact verdict: about 20 ns when the first 128 bytes
touch fewer than 23 cells (the `mid` page), ≈350 ns when the prefix check
fails, and ≈650 ns when the bound is computed but too low. On `mid` the verdict
is 10–17% over the value here, previously 8–10%.

### Coarse-bound receipts (M1 Pro)

AArch64 has no coarse kernel, so this re-runs the exact verdict on the current
tree: `criterion-m1-coarse-run{1,2,3}/`, `m1-coarse-gate-run{1,2,3}.json`,
`m1-coarse-bench-run{1,2,3}.log`, `m1-coarse-env.log`.

| Run | Floor lower (ns) | Budget (ns) |  Zero | ASCII | Random | Random / floor | Calibrated | Strict |
| --- | ---------------: | ----------: | ----: | ----: | -----: | -------------: | ---------- | ------ |
| 1   |            795.1 |      1192.6 | 223.0 | 235.8 |  608.7 |          0.766 | PASS       | PASS   |
| 2   |            790.6 |      1185.9 | 223.8 | 245.7 |  607.3 |          0.768 | PASS       | PASS   |
| 3   |            806.1 |      1209.2 | 219.3 | 240.1 |  610.2 |          0.757 | PASS       | PASS   |

| Run | Verdict `mid` | Value `mid` | Verdict vs value | Value random | Value calibrated |
| --- | ------------: | ----------: | ---------------: | -----------: | ---------------- |
| 1   |        1201.3 |      1144.6 |              +5% |       1059.1 | PASS             |
| 2   |        1209.9 |      1124.0 |              +8% |       1088.1 | PASS             |
| 3   |        1214.6 |      1117.5 |              +9% |       1114.4 | PASS             |

## Why SIMD cannot close the strict budget for exact histograms

All results below are diagnostic medians (`probes/m1-probes.log`), not Criterion
bounds. Each candidate was checked for exact count equality first.

| 4096 random bytes, M1 Pro                                                      |                      Median ns | Result                           |
| ------------------------------------------------------------------------------ | -----------------------------: | -------------------------------- |
| Production 4-bank scalar kernel (counting only)                                |                      1017–1025 | baseline                         |
| Scatter **stores only** (floor)                                                |                            805 | hard floor for memory counters   |
| Scatter RMW `+1` (no merge or finish)                                          |                            933 |                                  |
| Scatter loads only                                                             |                            612 | loads are not the bottleneck     |
| LSE `STADDH` one table (u16 or u32), 2 tables                                  |                      4036–4133 | rejected: ≈3.2 cycles per atomic |
| LSE plus plain RMW mix                                                         |                           3208 | rejected                         |
| NEON bit-plane counting only (8×8 bit transpose, nibble decode, `CNT`, `ADDP`) |                           2671 | rejected: ≈2.1 cycles/byte       |
| Hybrid, NEON share 12.5% / 20% / 25% / 33%                                     | 1084 / 1110 / 1186–1204 / 1311 | rejected: slower than scalar     |
| Fused NEON merge plus finalization (same float association)                    |          −30…−40 vs production | correct but small, not promoted  |

Findings:

- **Collisions are not the limit.** Sequential bytes (`seq`) and random bytes
  cost the same, ≈1000 ns. Load/store throughput is the limit: the store-only
  floor alone is ≈805 ns, which leaves ≈195 ns for counter reloads, the
  256-bin finalization (≈63 ns) and dispatch.
- **Register-only NEON counting** avoids stores but needs ≈7 vector
  instructions per byte, versus 4.1 instructions per byte for the scalar kernel.
  The hybrid split did not overlap: the M1 sustains ≈5.2–5.3 instructions per
  cycle in both forms, so the extra NEON instructions only add time. Compiled
  hybrid code had no NEON register spills.
- **Fused finalization** is bit-identical to `scalar_entropy` and saves about
  3%. Its prior four-bank spike measured the same gain. It changes neither
  gate result, so production code is kept unchanged.

### x86 (s5 VM, Broadwell-EP)

The s5 VM CPU is a Xeon E5-2683 v4 (Broadwell-EP: AVX2, no AVX-512). It has one
store-data port, so 4096 scattered stores need at least 4096 cycles: ≥1.37 µs at
the 3.0 GHz maximum turbo and ≥1.95 µs at the 2.1 GHz base clock. Strict 1000 ns
is therefore unreachable there for exact memory-counter histograms, whether
scalar or SIMD. AVX2 has gathers but no scatter or conflict detection. The
measured floor (lower bound 1673–1824 ns) agrees with that bound.

The production kernel measured 2972–2999 ns (random, upper bound), 1.64–1.79×
the same-run floor. That is above the fixed 1.5 ratio, whereas the M1 measured
1.36–1.40×. The emitted x86_64 assembly (`--emit asm`, local cross-compile of
the same source) shows why the gap is wider on Broadwell. Per 8 input bytes,
the hot loop issues:

- one `movq` word load;
- eight `incw off(%rsp,%rsi,2)` read-modify-write increments, each with an
  indexed address, which Broadwell unlaminates (≈3–4 fused-domain uops each);
- ≈17 ALU uops (`shr`, `and`, `movzbl`) to extract the bytes.

That is ≈5–6 fused uops per byte. At Broadwell's 4-wide issue, this predicts
≈1.3–1.5 cycles per byte, which matches the measured ≈1.5 cycles per byte at
the 2.1 GHz nominal clock. The store-only floor needs fewer uops per byte, so
on Broadwell the kernel appears issue-bound rather than store-bound. This is an
inference from the assembly and published uop counts, not from hardware
performance counters, which the VM does not expose.

### x86 counting-loop experiment (not adopted)

`probes/x86_probe.rs` compares counting loops on the s5 VM
(`probes/s5-x86-probes.log`, rustc 1.89.0, `taskset -c 0`, three runs). Every
variant zeroes the four u16 banks, counts 4096 bytes and merges into
`[usize; 256]`. Each variant matched a scalar histogram on random, sequential,
text and zero input before timing. A `w >>= 16` Rust variant was dropped because
LLVM compiles it to the same code as `prod`.

| Probe variant (4096 random bytes) | Median ns, 3 runs | Change     |
| --------------------------------- | ----------------: | ---------- |
| `prod` (LLVM shift-and-mask)      |         3099–3152 | baseline   |
| `bytes` (byte loads)              |         2717–2745 | −12…−13%   |
| `asm_idx` (`al`/`ah`, `shr 16`)   |         2583–2625 | −16…−18%   |
| `asm_split` (load, `inc`, store)  |         2581–2617 | −16…−17%   |
| `asm_4b` (four base registers)    |         2585–2616 | −16…−18%   |
| `asm_bytes_lea`                   |         2621–2648 | −15…−16%   |
| `asm_idx_x4`, `asm_4b_x4` (32 B)  |         2675–2722 | −13…−15%   |
| `asm_lea`, `asm_lea_split`        |         2796–2852 | −9…−11%    |
| Store-only floor                  |         1658–1673 | reference  |

Fewer uops won, and `lea` variants that allow port-7 store addresses lost. This
fits an issue-bound loop. `asm_idx` was then applied to production on x86_64
only (`probes/x86_asm_kernel.patch`, patched `mod.rs` SHA-256 `7fd26f1f…`). On
the VM, `cargo test -p libcram-core` passed (`s5-asm-cargo-test.log`), and the
gate was run three times (`criterion-s5-asm-run{1,2,3}/`,
`s5-asm-gate-run{1,2,3}.json`):

| Run | Floor lower (ns) | Budget (ns) | Zero upper | ASCII upper | Random upper | Random / floor | Calibrated |
| --- | ---------------: | ----------: | ---------: | ----------: | -----------: | -------------: | ---------- |
| 1   |           1664.8 |      2497.3 |      248.6 |       303.6 |       2773.4 |          1.666 | FAIL       |
| 2   |           1665.9 |      2498.9 |      245.4 |       291.1 |       2811.4 |          1.688 | FAIL       |
| 3   |           2027.7 |      3041.6 |      256.0 |       302.3 |       2774.1 |          1.368 | PASS       |

Random improved only 7% (2972–2999 → 2773–2811 ns), not the 17% the probe
suggested. The probe's `prod` was a non-unrolled loop, so it was slower than
real production. Run 3 passes only because its floor rose 22% on the shared
host, so it is not counted as a pass. Even the probe's counting loop alone is
≈1.56× the floor, so removing the zeroing, merge and finalization could not
reach 1.5×. The patch changes neither gate result, so production stays
unchanged, following the rule used for the fused finalization above.

AVX-512 approaches (`VPCONFLICTD`, scatter, per-lane private tables) still issue
at least one scattered element per byte. No AVX-512 machine was available, so
the existing `avx512` dispatch path remains compile-checked only. No AVX-512
speedup is claimed.

## Why the ratio is 1.5

The ratio was fixed before any x86 floor was measured, and it was not changed
after the x86 VM failed it. On the M1, the cost
decomposes against the floor (1.00) as follows: counter reload (RMW versus
store-only) ≈1.16, then finalization, merge and dispatch, for ≈1.36–1.40 in
total. A ratio of 1.5 absorbs that work plus run-to-run noise. It still rejects
any change that costs more than half the hardware floor extra.

### Limitations

- The calibrated SLA is a relative efficiency gate, not an absolute latency
  guarantee. A slower machine gets a proportionally larger absolute budget. A
  deployment that needs a fixed latency must state its own number. The actual
  per-machine budget is printed (`calibrated.budget_ns`).
- Background load affects both the floor and the entropy measurements. Using
  the floor's lower bound against the entropy upper bound keeps the comparison
  conservative.
- The floor bounds memory-counter histograms only. Register-only counting is
  not bounded by it, but it was measured slower on this machine.

## Value receipts (M1 Pro)

The suite uses Criterion defaults: 3 s warm-up, 100 samples and 95% confidence.
Three sequential runs used fresh `CRITERION_HOME` directories. Raw estimates are
in `criterion-m1-run{1,2,3}/` and gate output is in `m1-gate-run{1,2,3}.json`.

| Run | Floor lower (ns) | Budget (ns) | Zero upper | ASCII upper | Random upper | Random / floor | Calibrated | Strict |
| --- | ---------------: | ----------: | ---------: | ----------: | -----------: | -------------: | ---------- | ------ |
| 1   |            811.2 |      1216.8 |      154.1 |       256.9 |       1134.2 |          1.398 | PASS       | FAIL   |
| 2   |            803.7 |      1205.5 |      173.4 |       250.4 |       1091.1 |          1.358 | PASS       | FAIL   |
| 3   |            801.7 |      1202.6 |      154.7 |       250.5 |       1116.3 |          1.392 | PASS       | FAIL   |

## Value receipts (s5 x86 VM, production kernel)

The s5 VM is a 4-vCPU Incus guest on a shared Broadwell host (kernel
6.8.0-146-generic, `avx512bw` absent; see `s5-env.log`). Sources were copied
with a SHA-256 manifest, which was verified inside the VM before building. The
gate unit tests passed there (`s5-unittest.log`). Each of three sequential runs
was pinned with `taskset -c 0` and used a fresh `CRITERION_HOME`. Raw estimates
are in `criterion-s5-run{1,2,3}/`, gate output in `s5-gate-run{1,2,3}.json` and
Criterion console output in `s5-bench-run{1,2,3}.log`.

| Run | Floor lower (ns) | Budget (ns) | Zero upper | ASCII upper | Random upper | Random / floor | Calibrated | Strict |
| --- | ---------------: | ----------: | ---------: | ----------: | -----------: | -------------: | ---------- | ------ |
| 1   |           1823.7 |      2735.6 |      258.2 |       287.2 |       2982.3 |          1.635 | FAIL       | FAIL   |
| 2   |           1698.5 |      2547.7 |      261.8 |       299.8 |       2971.6 |          1.750 | FAIL       | FAIL   |
| 3   |           1672.9 |      2509.3 |      261.1 |       293.1 |       2998.8 |          1.793 | FAIL       | FAIL   |

Random is stable (2972–2999 ns, matching the earlier VM upper bound of 2972 ns).
The floor varies by about 9% between runs on the shared host, so the ratio
varies with it. Zero and ASCII pass both SLAs.

Replay the archived runs from the repository root:

```sh
python3 scripts/entropy_budget.py docs/performance/entropy-sla/criterion-m1-run1               # exit 0
python3 scripts/entropy_budget.py --sla strict docs/performance/entropy-sla/criterion-m1-run1  # exit 1
python3 scripts/entropy_budget.py docs/performance/entropy-sla/criterion-s5-run1               # exit 1
python3 scripts/entropy_budget.py --target verdict docs/performance/entropy-sla/criterion-s5-verdict-run1               # exit 0
python3 scripts/entropy_budget.py --target verdict --sla strict docs/performance/entropy-sla/criterion-s5-verdict-run1  # exit 1
python3 scripts/entropy_budget.py --target verdict --sla strict docs/performance/entropy-sla/criterion-m1-verdict-run1  # exit 0
python3 scripts/entropy_budget.py --target verdict --sla strict docs/performance/entropy-sla/criterion-s5-coarse-run1   # exit 0
python3 scripts/entropy_budget.py --target verdict --sla strict docs/performance/entropy-sla/criterion-m1-coarse-run1   # exit 0
```

## Reproduce

```sh
export CRITERION_HOME="$PWD/target/entropy-sla"     # fresh, avoids stale records
cargo bench -p libcram-bench --bench entropy --locked -- 'entropy/(dispatch/.*|verdict/.*|diagnostic/mid|floor/scatter_store)/4096'
python3 scripts/entropy_budget.py --target verdict "$CRITERION_HOME"
python3 -m unittest discover -s scripts -p test_entropy_budget.py
```

`just entropy-budget` runs the same bench filter and gate. On Linux, pin it with
`taskset -c 0`, as in earlier VM runs.

Diagnostic probes (`probes/*.rs`) are standalone `rustc -C opt-level=3` programs
and are not CI tools. The AArch64 probes expect `mod.rs` and `periodic.rs`
copied from `crates/libcram-core/src/entropy/` into the same directory, with
each `#[cfg(test)]` replaced by `#[cfg(any())]`. `x86_probe.rs` is
self-contained and x86_64-only, as is `hybrid_probe.rs`:

```sh
rustc --edition 2021 -C opt-level=3 -C codegen-units=1 x86_probe.rs && taskset -c 0 ./x86_probe
rustc --edition 2021 -C opt-level=3 -C codegen-units=1 hybrid_probe.rs && taskset -c 0 ./hybrid_probe
```

## Pending

- A NEON coarse kernel. The M1 already passes strict with the exact verdict,
  so none was built. AVX-512 hosts use the AVX2 kernel; no AVX-512 machine was
  measured.
- The coarse step decides only "not compressible". Low-entropy pages that are
  not periodic still need the exact count (text-like pages: about the value's
  cost).
- The value API (`shannon_entropy`) still fails calibrated on x86. The counting
  loops tried cannot reach 1.5× on Broadwell, because an in-memory `+1` costs
  more there than a plain store. Changing the ratio or the floor definition
  after seeing x86 data would be post-hoc and is not done here.
- The A–E goal remains incomplete for the reasons listed in
  `../../IMPLEMENTATION_STATUS.md`.

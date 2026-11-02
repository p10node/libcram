# A–E implementation and acceptance status

Frozen SDK/full-bindings verification snapshot: `3edfa98d95e039cdaca20e9906de3d10db301f2a`.
Newer follow-ups have separate evidence below. The existing SDK archives do not
include the subsequent production entropy optimization.
**The whole A–E goal is not complete.** Software integration is not a substitute
for the open performance and physical-device requirements below.

## Feature matrix

| Area                       | Implemented and verified                                                                                                                                                                                                                                                                                                                                                                                                                                                                     | Remaining acceptance                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| -------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| A: managed runtime         | Owned allocation metadata; start/stop; bounded intake and migration; policy and trusted health bridge; stale/lost/offline fail-closed behavior                                                                                                                                                                                                                                                                                                                                               | Production CRAM workload acceptance depends on C/D hardware gates                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| A: evacuation liveness     | Continuous source failure still evacuates; hot observations cannot rewind pressure evacuation; page/record quotas retained                                                                                                                                                                                                                                                                                                                                                                   | Real-device endurance and long-duration soak                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| B: exact Shannon           | Banked histogram plus byte-verified short-period counting; exact differential tests. Gate B measures the exact policy verdict `entropy < 4.5` (`entropy_below`, certified early stop, identical to the full comparison; definition accepted by the project owner on 2026-11-02): calibrated SLA (max(1000 ns, 1.5× same-run scatter-store floor)) and strict 1000 ns both PASS on M1 Pro (random 607–610 ns) and the x86 VM (random 797–812 ns, store-free AVX2 coarse bound), 3/3 runs each | Near-threshold pages cost 5–17% more than the value; the full value still FAILS calibrated on x86 (1.72–1.80× floor) and strict on both; no NEON coarse kernel; AVX-512 unmeasured                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| C: process PEBS            | Owned-thread attachment/cleanup and partial-failure cleanup; explicit Broadwell-X L1-hot attribution; missing capability is an error                                                                                                                                                                                                                                                                                                                                                         | ALL_LOADS DLA FAIL on tested host (root cause: CPU PEBS records microcode-load addresses); audited `0x67d1` Broadwell-X load profile PASSES attribution; PEBS store attribution is biased (write-heavy cached pages look write-free), so writes come from sampled userfaultfd write-protect (`WriteSampler`, Linux ≥ 6.7, fail-closed; end-to-end PASS 3/3); kernel-enforced per-CPU burst limit (≤ 240 samples/100 ms) plus governor, worst steady state 0.90% of one core on s5; NOT covered: ≈40–50 ms one-time load per CPU, read-sample bias toward post-stall loads, per-page `reads/4 > writes` rarely reached at governed rates, Linux ≥ 6.10 limit path unmeasured; only Broadwell-X audited ([procedure](PEBS_PROFILE_AUDIT.md)) |
| D: physical-health adapter | Atomic trusted report convention; provenance, generations, freshness, bounds, symlink/permission controls; fixture tests                                                                                                                                                                                                                                                                                                                                                                     | No real CRAM device available; physical occupancy, compression savings and poison/device behavior not accepted                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| E: callback ownership      | Explicit quiescent unregister barrier; ordinary unregister is not a lifetime barrier                                                                                                                                                                                                                                                                                                                                                                                                         | Callers must release userdata/code only after the waiting API returns CRAM_OK                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| E: bindings                | Native C/C++17/Go; Python; CPU Torch zero-copy; generated-header agreement                                                                                                                                                                                                                                                                                                                                                                                                                   | Does not claim GPU Torch support                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| E: engineering SDK         | Clean-source native macOS/Linux archives; full payload hashes including nested manifests; extracted C/C++ consumers assert actual loaded-library provenance                                                                                                                                                                                                                                                                                                                                  | Not a signed/notarized production certification or real-device qualification                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| E: integration             | Frozen-source macOS/Linux gates, independent bounded static re-review, short fixture soak; archived side worktrees                                                                                                                                                                                                                                                                                                                                                                           | Long-duration production soak remains open                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |

## PEBS host follow-up

[Owned core/event-interaction diagnostic](evidence/pebs-event-interaction/README.md):
ALL_LOADS still returns zero owned-mapping addresses on CPUs 12 and 28, including
co-recording with L1_HIT/ALL_STORES and reversing the load/hit group order.
L1_HIT controls remain valid. Every stream identifies the actual load instruction,
but ALL_LOADS data addresses are wrong. Address-only replay independently confirms
the corrected multi-column parser. This narrows the failure; it does not establish
a CPU/microcode/kernel cause, a repair, or <1% overhead. Production code is unchanged.

[2026-11-01 root cause and cost measurement](evidence/2026-11-01/README.md):
the PEBS DLA field captures microcode-issued loads (constant `0xfeb8xxxx`, once
`TSS.RSP0` in cpu_entry_area) whenever they qualify for the event's umask and
privilege filter. Affected on this host: ALL_LOADS, STLB_MISS_LOADS, L1_MISS
and load latency, plus L1_HIT and ALL_STORES with OS counting. Microcode
`0xb000040` is the newest available. The `0xd1/0x67` union matches the
ALL_LOADS count within 0.5% with valid DLA. The end-to-end cost is
4.2–5.1 µs/sample, almost all of it PEBS assist plus PMI/NMI; the BPF program
cost is not measurable above zero.

[2026-11-01 profile and governor](evidence/2026-11-01/governor/README.md):
`sampling::broadwell_x::configs` (`0x67d1` + `0x82d0`) passes owned attribution
through the BPF path, including DRAM loads. Two PEBS periods must be distinct
and coprime: PEBS format 2 drops nearly all records when equal periods keep
the counters aligned. `ProcessPebsCollector` now caps samples per CPU with a
shared period scale. Measured worst steady state at the 1,200/s default:
0.93% of one core.

[2026-11-01 shared program](evidence/2026-11-01/shared-program/README.md):
threads on one CPU share one loaded BPF object. A thread attach costs 0.13 ms
of collector CPU instead of ≈43 ms. Re-measured worst steady state: 0.89%.

[2026-11-02 write evidence and burst limit](evidence/2026-11-02/README.md):
see [PEBS_HOST_ACCEPTANCE.md](PEBS_HOST_ACCEPTANCE.md#2026-11-02-write-evidence-and-burst-limit)
and the C row above.

## Entropy SLA revision (gate B)

[SIMD investigation and calibrated SLA](performance/entropy-sla/README.md):
NEON bit-plane counting, NEON/scalar hybrids and LSE `STADDH` were all slower
than the scalar four-bank kernel on M1 Pro. A scattered store floor shows that
strict 1000 ns has no headroom there. The production entropy kernel is unchanged.
The gate now passes when 4 KiB latency is below
max(1000 ns, 1.5 × the same-run floor). M1 passed 3/3 runs, with random at
1.36–1.40× the floor. The x86 VM failed 3/3 runs: random was 2972–2999 ns
against a 1673–1824 ns floor (1.64–1.79×). The x86 assembly suggests that
Broadwell is issue-bound on indexed `incw` read-modify-writes and shift
extraction, not store-bound. An x86-only inline-asm counting loop (`al`/`ah`
extraction) passed the core tests on the VM but made random only 7% faster
(2773–2811 ns, 1.67–1.69× floor in the two runs with a normal floor). It was
not adopted. The 1.5 ratio was not changed after the x86 data. Strict results
are still reported and fail on both machines.

Gate B was then redefined around what the policy consumes: the exact verdict
`entropy < 4.5`. `entropy::entropy_below` counts a prefix and stops when
certified bounds on the whole page clear the threshold by more than rounding
error:

- lowest: unread bytes repeat the top symbol;
- highest: the mixture bound.

Otherwise it counts everything and compares exactly. `OwnedAllocation::profile`
records `migration::Entropy::Compressible(..)`, and `observation_token` accepts
values or verdicts. The C ABI is unchanged. On pseudorandom pages the verdict
measured:

- M1: 615–635 ns, strict PASS 3/3;
- x86 VM: 1484–1548 ns, 0.89–0.94× floor, calibrated PASS 3/3, strict FAIL.

Broadwell has one store port, so an exact count of the ≈1850 bytes the lowest
bound needs cannot fit 1000 ns there. A store-free coarse bound now runs first
on x86_64 with AVX2 (`entropy/coarse.rs`):

- `H(bytes) ≥ H(cells)` for any grouping of byte values.
- Count `x >> 3` cells on 3/8 of the page and `(x >> 3) & 15` cells on the
  rest, with `vpshufb` one-hot planes, Harley-Seal adders and a transposed
  `vpsadbw` reduction.
- The exact minimum of the cell entropy over all completions is 4.67 bits on
  the bench page, so random pages are decided "not compressible" without an
  exact count. Undecided pages fall through to the exact verdict, so results
  stay identical.

Current tree, 3 runs each: M1 607–610 ns (unchanged path) and x86 VM
797–812 ns (0.48–0.49× floor). Both pass calibrated and strict. The full value
is still measured and reported.

## Exact entropy follow-up

[Counter-layout comparison](performance/entropy-layout-spike/README.md) rejected
three further exact candidates: packed u64, interleaved u16 and four u32 banks.
All passed 665 differential cases per variant on M1 and Linux/x86, but none
improved random diagnostic medians. No production change or new Criterion
acceptance is claimed; B remained open until the gate B revision above.

[Short-period implementation, source hashes and raw evidence](performance/entropy-periodic/README.md):
- Exact runtime-discovered period, verified against every byte including the partial tail.
- Actual RED→GREEN plus three isolated behavioral mutation controls; bounded independent review.
- Fresh default-feature workspace tests: macOS 88 PASS, Linux VM 89 PASS; all-target/all-feature Clippy PASS on both.
- Unchanged Criterion upper-mean bounds (zero/ASCII/random, ns): M1 152.884/242.731/1080.987; VM 243.575/292.325/2972.236.
- Both strict gates still FAIL on random; benchmark inputs and 1000 ns budget unchanged. Earlier SDK/bindings/soak receipts remain tied to their original snapshot.

## Review repairs

- `76d18bd`: preserve evacuation progress under continuous hot-page observations.
  The regression failed before the fix. Tests vary 3/5-page allocations, 1/2-page
  quotas, first/last hot pages and Critical/Emergency pressure. Residency and
  compressed-byte accounting prove completion; successful move results are not
  miscounted as distinct pages.
- `bc67b0f`: exclude only the archive root MANIFEST.json from payload verification.
  Differential verifier tests reject an unlisted nested manifest and accept a
  correctly hashed one. Both cases failed before repair. Mocked compiler calls
  in these unit tests are not native consumer evidence.
- `ebab5f2`: explicit trusted clock-calibration seam for paired-clock fixtures.
  Default production calibration remains conservative; TTLs and event-time
  checks are unchanged. The prior real-clock queue fixture was intermittently
  rejecting samples at the token-creation boundary; it was not fixed by sleeps
  or increasing freshness tolerance.

[Independent static re-review](evidence/2026-10-09/closure-3edfa98/review-fixes.md)
found no blocker within those six source/test files. It is deliberately bounded,
not an independent hardware or full-repository certification.

## Fresh execution evidence

[Raw logs, source hashes, runners and receipts](evidence/2026-10-09/closure-3edfa98/):

- macOS `just check native-test torch-test header-check`, all-target/all-feature
  workspace Clippy, package construction and archive verification: exit 0.
  Rust logs contain 83 passing tests; Python 10 and Torch 2 pass.
- Linux VM workspace all-feature tests and all-target/all-feature Clippy:
  exit 0; Rust logs contain 85 passing tests. All 175 tracked input files matched
  the frozen local source hashes after execution.
- Linux native C/C++/Go, Python 10 and Torch 2 tests: exit 0. A rerun records its
  own exit receipt because an earlier process handle did not retain a numeric
  exit; an empty/missing exit was not treated as success.
- Owned Linux fixture/stress loop: 25 successful repetitions in 60.747 seconds,
  including runtime, bridge, device reader, callback lifecycle and allocator
  stress tests. This is a short soak, not long-duration or physical CRAM testing.
- Native archives for aarch64-apple-darwin and x86_64-unknown-linux-gnu were built
  from the same clean revision. Both executed C and C++ consumers from an owned
  extraction prefix. Linux archive verification was also rerun in the foreground
  with exit 0. SHA-256 and sizes are retained in results.json.

The earlier broad Linux hardware harness returned nonzero for the unchanged
entropy budget and rejected unsupported guest precise sampling. Neither result
is overwritten by these narrower software gates. See
[entropy measurements](performance/entropy-update.md),
[host PEBS scope](PEBS_HOST_ACCEPTANCE.md) and
[two-DRAM-node NUMA evidence](S5_VM_ACCEPTANCE.md).

## Artifacts and cleanup

Local SDK archives are under `target/dist/`, revision suffix `3edfa98d95e0`.
No release was published remotely.

All five implementation worktrees had clean tracked/untracked status and
patch-equivalent commits already on main. Their tracked source archives, Git
metadata and complete ignored directories were preserved under:

`../libcram-worktree-archive/20261009T091859Z/`

Ignored trees were moved without discarding files and their complete file-hash
inventories were compared before/after. Worktrees were removed without force;
original task branches remain as recovery references. Only the canonical main
worktree remains. All tracked gate processes are exited. The dedicated owned
Linux test VM and its build/evidence caches are retained, not deleted; no user
workload or host security policy was modified for closure.

## Callback regression follow-up

Three state-synchronized callback regressions now cover replacement while an old
callback is in flight, queued generation rejection, competing quiescence and
captured-state destructor reentry/panic. Five isolated mutation controls detect
broken behavior. The macOS workspace has 86 passing tests and all-feature Clippy
passes; production logic and the frozen SDK artifact are unchanged. See the
[raw evidence and limits](evidence/callback-quiescence-regressions/README.md).
This does not close B, C or the real-device/long-soak gates.

## Remaining order

1. C: per-data-source events were measured and rejected; writes now come
   from `WriteSampler`. Open: read samples still favour loads after stalls,
   and per-page `reads / 4 > writes` needs ≥ 4 samples per page per 100 ms,
   which governed sampling rarely gives. Decide whether read-dominance should
   be judged per allocation (or over a longer window).
2. D/E: obtain a real CRAM fixture before physical capacity/poison/compression
   acceptance and long-duration end-to-end production qualification.

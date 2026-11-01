# A–E implementation and acceptance status

Frozen SDK/full-bindings verification snapshot: `3edfa98d95e039cdaca20e9906de3d10db301f2a`.
Newer follow-ups have separate evidence below. The existing SDK archives do not
include the subsequent production entropy optimization.
**The whole A–E goal is not complete.** Software integration is not a substitute
for the open performance and physical-device requirements below.

## Feature matrix

| Area | Implemented and verified | Remaining acceptance |
| --- | --- | --- |
| A: managed runtime | Owned allocation metadata; start/stop; bounded intake and migration; policy and trusted health bridge; stale/lost/offline fail-closed behavior | Production CRAM workload acceptance depends on C/D hardware gates |
| A: evacuation liveness | Continuous source failure still evacuates; hot observations cannot rewind pressure evacuation; page/record quotas retained | Real-device endurance and long-duration soak |
| B: exact Shannon | Banked histogram plus byte-verified short-period counting; unchanged ASCII benchmark now <1000 ns on M1/AVX2 VM; exact differential tests | Strict 4 KiB <1000 ns remains FAIL for random on both measured machines |
| C: process PEBS | Owned-thread attachment/cleanup and partial-failure cleanup; explicit Broadwell-X L1-hot attribution; missing capability is an error | ALL_LOADS DLA FAIL on tested host (root cause: CPU PEBS records microcode-load addresses); audited `0x67d1`/`0x82d0` Broadwell-X profile PASSES attribution; rate governor (default 1,200 samples/s/CPU) gives worst steady state 0.89% of one core on s5; one BPF program per CPU shared by its threads (thread attach ≈0.13 ms); NOT covered: ≈40–50 ms one-time load per CPU, sub-100 ms bursts, sampling bias toward post-stall loads; no universal CPU profile |
| D: physical-health adapter | Atomic trusted report convention; provenance, generations, freshness, bounds, symlink/permission controls; fixture tests | No real CRAM device available; physical occupancy, compression savings and poison/device behavior not accepted |
| E: callback ownership | Explicit quiescent unregister barrier; ordinary unregister is not a lifetime barrier | Callers must release userdata/code only after the waiting API returns CRAM_OK |
| E: bindings | Native C/C++17/Go; Python; CPU Torch zero-copy; generated-header agreement | Does not claim GPU Torch support |
| E: engineering SDK | Clean-source native macOS/Linux archives; full payload hashes including nested manifests; extracted C/C++ consumers assert actual loaded-library provenance | Not a signed/notarized production certification or real-device qualification |
| E: integration | Frozen-source macOS/Linux gates, independent bounded static re-review, short fixture soak; archived side worktrees | Long-duration production soak remains open |

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

## Exact entropy follow-up

[Counter-layout comparison](performance/entropy-layout-spike/README.md) rejected
three further exact candidates: packed u64, interleaved u16 and four u32 banks.
All passed 665 differential cases per variant on M1 and Linux/x86, but none
improved random diagnostic medians. No production change or new Criterion
acceptance is claimed; B remains open.

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

1. B: new exact histogram construction experiments, with real M1 and x86
   measurements against the unchanged distribution/budget contract. The
   [fused-finalization spike](performance/entropy-fused-spike/README.md) found
   only a small NEON diagnostic gain; no candidate was promoted and the strict
   acceptance remains FAIL.
2. C: decide whether the post-stall PEBS sampling bias needs per-data-source
   load events, and whether sub-100 ms load bursts need a shorter governor
   interval.
3. D/E: obtain a real CRAM fixture before physical capacity/poison/compression
   acceptance and long-duration end-to-end production qualification.

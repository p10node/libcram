# Kernel-enforced PEBS sample budget (s5, 2026-11-02)

Same host and rules as `../source-split`. `budget.c` arms one PEBS event with
`PERF_EVENT_IOC_REFRESH(budget)` instead of `PERF_EVENT_IOC_ENABLE`: the kernel
disables the event after `budget` overflows. Each 100 ms interval runs an
L1-streaming burst; the event is re-armed only after it has stopped
(`time_enabled` frozen), so the remaining budget never exceeds one grant.
Raw output: `budget.log`.

| Event  | Period | Unbudgeted samples per 100 ms | Budget | Delivered per interval | Result |
| ------ | -----: | ----------------------------: | -----: | ---------------------: | ------ |
| 0x67d1 | 200003 |                       538–685 |    120 |              120 (6/6) | HELD   |
| 0x67d1 |  20011 |                     5162–7152 |    120 |              120 (6/6) | HELD   |
| 0x67d1 |   2003 |                   71241–71709 |    120 |              120 (6/6) | HELD   |
| 0x82d0 |  20011 |                     6872–7215 |    120 |              120 (4/4) | HELD   |

The budget is exact and independent of the burst rate, unlike the 100 ms
`RateGovernor`, which can only react after an interval. With re-arming only
after exhaustion, any window of one interval holds at most two grants.

## Through libcram (`examples/pebs_burst.rs`)

Production `ProcessPebsCollector` (default cap 1,200 samples/s per CPU), base
period 20011, workload on CPU 30, collector on CPU 25, 10 s per run, 3 runs.
"HEAD" is the committed tree (governor only, BPF object `d0ac4091…`). "New"
grants kernel limits: events start with 2 samples; every 100 ms each event is
topped up by what it used and events that ran dry share the rest of the CPU's
budget (`process_pebs::sample_budget` = 2 × cap × 100 ms = 240). The BPF
object (`1efb4ece…`) returns 1. Logs: `pebs-burst-head.log`,
`pebs-burst-new.log`; hashes in `../binaries.sha256`.

| Workload                   | HEAD mean/s         | HEAD worst 100 ms | New mean/s | New worst 100 ms |
| -------------------------- | ------------------- | ----------------- | ---------- | ---------------- |
| Streaming 10 s             | 1269; 2 runs failed | 2884              | 1042–1116  | 234–236          |
| 30 ms burst, 400 ms idle   | 1664–1724           | 848–896           | 617–634    | 472–473          |
| 30 ms burst, 70 ms idle    | 845–956             | 625–933           | 676–920    | 236              |

- HEAD failed two of three streaming runs with "PEBS ring loss": at the base
  period the first interval overflowed the BPF ring before the governor
  reacted.
- After idle, HEAD averaged 1664–1724 samples/s, above its own cap, because
  every burst started at a relaxed period. New stays below the cap.
- The worst window of event time is at most one grant per interval (≤ 240).
  A sliding 100 ms window can span two intervals (≤ 473 measured, bound 480).

Governed steady state (`pebs_overhead`, base 200003, cap=1200, 20 ABBA pairs;
`gov-cap1200.log`, `gov-cap1200-analysis.txt`):

- settled 1,066 samples/s (CI 1,065–1,073, max 1,084), scale 23.6;
- collector 0.28% of a core (CI 0.272–0.280%, max 0.30%).

Worst steady state stays at the cap: 1,200 × 4.99 µs (upper CI of the
30-pair study in `../../2026-11-01/governor`) + 0.30% = **0.90% < 1%**.

BPF return value (`bpf-return{0,1}-fixed.log`, `bpf-return-analysis.txt`,
15 pairs, periods 200003 and 20011). Linux 6.10+ skips the limit unless the
program returns nonzero (`__perf_event_overflow`, v6.12); on 6.8 the limit is
decremented before the handler either way:

| BPF object        | Cost per sample, ns | 95% CI    |
| ----------------- | ------------------: | --------- |
| returns 0 (HEAD)  |                4959 | 4374–5639 |
| returns 1 (new)   |                4954 | 4465–5406 |

No measurable difference. The aya `#[perf_event]` macro always returns 0, so
the program's entry points are written without it. The ≥ 6.10 path has not
been run on hardware.

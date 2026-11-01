# One BPF program per CPU (s5, 2026-11-01)

Same host and rules as [../README.md](../README.md). The binaries are listed in
`binaries.sha256`. The analysis script is
[../governor/analyze.py](../governor/analyze.py).

## Change

`PebsSampler` used to load a full libcram BPF object for every
(thread, CPU) attachment. Each load costs ≈43 ms of CPU: kernel BTF parse plus
verifier. Now:

- `pebs::PebsProgram::load` loads the object once: both programs, the EVENTS
  ring and the DROPPED map.
- `PebsProgram::attach(pid, cpu, configs)` returns `PebsEvents`: perf FDs, PMU
  counts and `set_periods`. The cost is `perf_event_open` plus two ioctls.
- `ProcessPebsCollector` loads one program per selected CPU at that CPU's
  first attachment. Every thread attached on that CPU shares it. Drain goes
  round-robin over the CPU rings. Any failure drops every program, so queued
  records are never replayed.
- `PebsSampler` keeps its API as a wrapper over one program and one
  attachment.

Programs are deliberately not shared across CPUs. The kernel BPF ring
reserves with a trylock in NMI context, so CPUs sampling concurrently into
one ring could lose reservations. With one ring per CPU, the ring sees the
same producer pattern as before.

## Results (`acceptance.log`)

| Check                                             | Result                                  |
| ------------------------------------------------- | --------------------------------------- |
| First poll: program load + main-thread attach     | 38.9 ms CPU                             |
| Poll attaching a new thread to the loaded program | **0.129 ms** CPU (was ≈43 ms)           |
| Thread churn, attach + detach cycle (mean of 20)  | **0.151 ms** CPU                        |
| Programs loaded for 2–3 threads on one CPU        | 1                                       |
| FDs after churn / after drop                      | unchanged / back to baseline            |
| Managed lifecycle + attribution + governor        | PASS (1,075 samples/s at cap 1,200)     |
| `0x67d1` l1 / mixed attribution                   | PASS (99.92% / 98.90% in owned regions) |
| ALL_LOADS control                                 | FAIL (0%), as expected                  |
| Harness sample delivery, fixed periods            | 520 / 520                               |

## Governed CPU cost at cap 1,200 (`analysis.txt`)

| Run       | Steady samples/s (max) | Block CPU delta incl. transient | Collector steady CPU | Attach CPU, 2 threads |
| --------- | ---------------------- | ------------------------------- | -------------------- | --------------------- |
| streaming | 1,057 (1,110)          | +0.60% (CI +0.32, +1.28)        | 0.27% (max 0.29%)    | 49.9 ms (was 85.8)    |
| jitter    | 1,007 (1,044)          | +0.60% (CI +0.40, +0.67)        | 0.28% (max 0.29%)    | 50.0 ms (was 85.7)    |

Gate:

- Worst steady state at the cap: 1,200 × 4.99 µs + 0.29% = **0.89% of one
  core: PASS**.
- Typical measured: 1,057 × 4.73 µs + 0.27% = 0.77%.

The one-time cost is now ≈40–50 ms per selected CPU plus ≈0.15 ms per thread
lifecycle. Attach and detach alone reach 1% of a core only at about 66 thread
lifecycles per second. Threads attached after the governor has scaled start
at the governed period, so they cause no base-period transient.

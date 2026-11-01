# Broadwell-X load profile and sample-rate governor (s5, 2026-11-01)

Same host and rules as [../README.md](../README.md): physical `s5`, Xeon
E5-2683 v4, microcode `0xb000040`, Linux 6.8.0-146, owned processes only,
workload on CPU 30, collector on CPU 25. Binaries were built in the owned
`libcram-numa-test` guest:

- `binaries-run1.sha256`: acceptance probe, `fixed-jitter.log`, cap=1500 logs
- `binaries-run2.sha256`: `managed-final.log` and cap=1200 logs. The only
  source change between the two runs is `DEFAULT_MAX_SAMPLES_PER_CPU_SEC`
  going from 1500 to 1200.

## What changed in libcram

- `sampling::broadwell_x::configs(load_period)` is the audited profile:
  `0x67d1` retired loads and `0x82d0` stores, user-only.
- Two PEBS events must have distinct, coprime periods. `companion_period(p)`
  gives the store period, ≈ p·65/64.
- `RateGovernor` scales every period by one shared factor.
  `ProcessPebsCollector` applies it by default, reading counts at most every
  100 ms and reprogramming with `PERF_EVENT_IOC_PERIOD`.
- `PebsSampler` loads the BPF object with `EbpfLoader::new()`. aya's
  `Ebpf::load` parsed the 6 MB kernel BTF twice per attach.

## 1. Profile acceptance through the real PebsSampler + BPF path

Results are in `acceptance-final.log`, produced by `pebs_acceptance`. The tool
knows exactly how many loads and stores each workload retires. Workloads:

- `l1`: 16 KiB, 1 load + 1 store per step
- `mixed`: 8 L1 loads + 1 dependent 64 MiB random-chase load + 1 store per step

| Event, workload         | Load count / retired loads | Read samples in owned regions | Result         |
| ----------------------- | -------------------------- | ----------------------------- | -------------- |
| `0x67d1`, l1, P=2000003 | 1.0005                     | 99.97%                        | PASS           |
| `0x67d1`, l1, P=200003  | 1.0014                     | 99.86%                        | PASS           |
| `0x67d1`, mixed         | 1.0094                     | 99.05%                        | PASS           |
| ALL_LOADS, l1 / mixed   | 1.0005 / 1.0087            | 0%                            | FAIL (control) |
| L1_HIT, mixed           | 0.8891 (= L1 share 8/9)    | 99.55%                        | n/a (L1 only)  |

Sample delivery is now complete. At P=200003, 61,557 samples arrived out of
61,558 expected from the PMU counts.

**Known bias:** in the `mixed` workload only 0.09% of `0x67d1` samples hit the
L1 buffer, although 88.9% of retired loads are L1 hits. PEBS records the first
qualifying load to retire after the counter arms, and after a long stall that
is usually the stalled DRAM load. Addresses are correct, but sample counts
over-represent loads that retire after stalls. Counts are not proportional to
loads across pages with different cache behaviour. ALL_LOADS would share this
mechanism; L1_HIT avoids it only because it never arms on non-L1 loads.

## 2. PEBS fmt2 collision: why paired periods must differ

With equal load and store periods, the overhead harness's lockstep loop
(1 load + 1 store per step) lost almost all samples. Neither the BPF `DROPPED`
counter nor the ring showed it. `collide.c` reproduces this without BPF: it
opens the same two events and reads the kernel's own perf ring
(`collide.log`, `collide-delta.log`).

| Load P / store P       | Counter phase offset | Samples delivered (load, store) | Kernel LOST_SAMPLES |
| ---------------------- | -------------------- | ------------------------------- | ------------------- |
| 200003 / 200003        | 254 events           | 0 / 5,368, 0 / 5,368            | 15, 15              |
| 20011 / 20011 (CPU 12) | 776 events           | 72 / 53,540, 72 / 53,540        | 77, 77              |
| 2000003 / 2000003      | 5,973 events         | 536 / 536, 536 / 536            | 0, 0                |
| 200003 / 323609        | –                    | 5,368 / 5,368, 3,317 / 3,317    | 0, 0                |

On PEBS format 2 the kernel drops records whose status shows both counters, and
silently drops records whose status shows none
(`intel_pmu_drain_pebs_nhm`). Equal periods keep the overflows aligned forever
whenever the start offset is under the collision window, which is between 776
and 5,973 events here. `companion_period` makes the phases drift by p/64 per
sample. At P=200003, harness delivery went from 0–18 of 524 samples to 520 of
520.

## 3. CPU cost

Results are in `analysis.txt`, produced by `analyze.py`.

**Per sample, at fixed periods** (`fixed-jitter.log`, 0x67d1 + companion store,
30 ABBA pairs × 4 periods):

- CPU-time regression: **4,726 ns** (95% CI 4,468–4,986)
- stolen-time regression: 4,106 ns (3,683–4,532)

This matches the L1_HIT measurement in [../README.md](../README.md).

**Governed, worst case.** The tight L1-streaming loop is the highest event
rate. Ungoverned, base P=200003 gives about 20k samples/s. Blocks last 4.6 s
and are re-attached every block (20 pairs). The jitter loop uses base
P=20011, with 15 pairs of 3.7 s.

| Run                 | Steady samples/s (max) | Final scale | Block CPU delta incl. attach transient | Collector steady CPU |
| ------------------- | ---------------------- | ----------- | -------------------------------------- | -------------------- |
| cap 1500, streaming | 1,320 (1,352)          | 17.5        | +1.04% (CI +0.23, +1.25)               | 0.30% (max 0.32%)    |
| cap 1500, jitter    | 1,252 (1,357)          | 8.4         | +0.67% (CI +0.46, +0.79)               | 0.30% (max 0.33%)    |
| cap 1200, streaming | 1,050 (1,130)          | 21.6        | +0.70% (CI +0.11, +1.53)               | 0.29% (max 0.31%)    |
| cap 1200, jitter    | 1,008 (1,018)          | 10.4        | +0.70% (CI +0.22, +0.93)               | 0.29% (max 0.30%)    |

Steady state starts at the first governor update, the second poll. Collector
steady CPU excludes attach time. The governor collector costs more than the
bare-sampler harness (0.08–0.18%) because every poll also scans
`/proc/self/task`, reads the DROPPED maps and, every 100 ms, the PMU counts.

**Gate.** The governor lets the rate reach the cap before raising periods, so
the worst steady state is at the cap:

- cap 1500: 1,500 × 4.99 µs + 0.33% = **1.08% → FAIL**
- cap 1200: 1,200 × 4.99 µs + 0.33% = **0.93% → PASS**

At cap 1200, typical steady state measured 1,050 × 4.73 µs + 0.29% = 0.79%.
The default is therefore 1,200 samples/s per monitored CPU. The direct paired
block deltas agree with the projection: median +0.70%, against a projected
0.50% steady plus 0.21% transient. Their CIs are too wide to prove <1% on their
own.

**One-time costs per attachment, not in the steady figure:**

- **Collector:** ≈43 ms CPU per (thread, CPU) sampler, 86 ms for the two
  samplers here. This is the BPF object load, BTF parse and verifier.
- **Workload:** the first governor interval runs at the base period, ≈1,000
  samples (jitter) or ≈2,000 (streaming), ≈5–10 ms of CPU.

At that cost, a process creating more than about one thread per 4 s per
monitored CPU would exceed 1% from attach cost alone. **Superseded:** one BPF
program is now shared per CPU, so a new thread costs ≈0.13 ms and the gate
improves to 0.89%; see [../shared-program](../shared-program/README.md).

**Not covered:**

- A workload that switches between idle and streaming faster than the
  100 ms governor interval can exceed the cap in each burst. The kernel's
  `perf_event_max_sample_rate` is the only hard bound.
- The governor caps samples per monitored CPU. Collector cost grows with
  the number of samplers.

## Reproduce

```sh
gcc -O2 -o collide collide.c
./collide 30 67d1 200003 200003 1073741824   # collision
./collide 30 67d1 200003 323609 1073741824   # independent periods
pebs_acceptance program.bpf 30 2000003 retired-loads l1
pebs_acceptance program.bpf 30 20011 retired-loads mixed
managed_pebs_acceptance program.bpf 30
pebs_overhead program.bpf 30 25 retired-loads 2000003,200003,50021,20011 30 100 100 jitter
pebs_overhead program.bpf 30 25 retired-loads 200003 20 40000 100 cap=1200
pebs_overhead program.bpf 30 25 retired-loads 20011 15 1500 100 jitter cap=1200
python3 -I analyze.py fixed-jitter.log gov-*.log
```

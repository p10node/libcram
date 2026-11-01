# PEBS DLA root cause and collector CPU cost (s5, 2026-11-01)

Host: physical `s5`, Xeon E5-2683 v4 (family 6, model 79, stepping 1), microcode
`0xb000040`, Linux 6.8.0-146-generic, `PEBS fmt2+`, `max_precise=3`, HZ=1000,
`perf_event_max_sample_rate=14000`, PTI + MDS/TAA `VERW` mitigations active.
Every measurement samples only a newly started process's own thread, pinned to
CPU 30 (collector on CPU 25). No sysctl, microcode, kernel, VM or other workload
was changed. Binaries were built in the owned `libcram-numa-test` guest; hashes
are in `binaries.sha256`.

## 1. ALL_LOADS data address: root cause

**Conclusion:** the fault is in the CPU's PEBS implementation, meaning hardware
plus microcode. It is not caused by the perf event configuration, the kernel or
libcram. It is also not a missing Data Linear Address (DLA) capability. On this
part, the PEBS DLA field holds the address of the last load that the programmed
event qualified, and microcode-issued loads can qualify. For some events and
privilege filters, those microcode loads land after the tagged user load,
overwrite its address, and the record stores a non-architectural address.

Evidence (`dla-root-cause/dla-matrix.log`, tool `pebsdiag.c`, which reads the perf
ring directly, so no `perf script` parsing is involved):

| Event (raw)                     | `:u` DLA in owned range | `:uk` DLA in owned range |
| ------------------------------- | ----------------------- | ------------------------ |
| ALL_LOADS 0x81d0                | 0% (`0xfeb80008/40/48`) | 0% (`0xfeb819c8`)        |
| STLB_MISS_LOADS 0x11d0          | 0% (`0xfeb80008`)       | 0% (`0xfeb81048/88`)     |
| L1_MISS 0x08d1                  | 0% (`0xfeb80008/40/48`) | 0%                       |
| LOAD_LATENCY 0x01cd ldlat=3     | 0% (`0xfeb80f98..1988`) | not run                  |
| UOPS_RETIRED.ALL 0x01c2 (no DLA) | 0% (`0xfeb819c8`)       | 0% (`0xfeb819c8`)        |
| L1_HIT 0x01d1                   | 100%                    | 0% (`0xfeb819c8`)        |
| L2_HIT 0x02d1                   | 100%                    | 99.9%                    |
| L3_HIT 0x04d1                   | 99.9%                   | 99.4%                    |
| L3_MISS 0x20d1                  | 100%                    | 99.5%                    |
| SPLIT_LOADS 0x41d0              | 99.9%                   | 99.9%                    |
| LOCK_LOADS 0x21d0               | 100%                    | 100%                     |
| ALL_STORES 0x82d0               | 100%                    | 0% (`0x803b`)            |
| d1 umask 0x67 (union)           | 98.7–100%               | not run                  |

The rows for 0x67d1 and the cache-level events miss a few samples that are
genuine loads outside the owned buffers, such as the tool's own ring reads.

Why each alternative is ruled out:

- **perf/kernel config:** for user-only sampling, ALL_LOADS and L1_HIT use identical
  attrs and the same kernel constraint flags (`DATALA_LD`). The kernel copies
  `pebs->dla` verbatim. It does so even for UOPS_RETIRED.ALL, which has no
  DLA semantics, so the kernel cannot be the source of `0xfeb8xxxx`. Changing
  precise level (1/2/3), period, large-PEBS (`wakeup_events=0`), CPU (12, 30), or
  socket does not change the result.
- **missing DLA:** records carry valid DLA for L1/L2/L3_HIT, L3_MISS, SPLIT, LOCK
  and stores on the same host, record format and kernel path.
- **workload:** heap, `.bss` (`0x405100`), stack, syscall-heavy and DRAM-chase
  workloads all give the same constants. The eventing IP is always the real load
  instruction.
- **what the bad values are:** `0xfeb8xxxx` is the same for every process and CPU.
  It is not mapped in the process, it is not System RAM, and no `/proc/iomem`
  entry contains `0xfeb80000`. Two samples carried `0xfffffe7942873004`, a
  cpu_entry_area address at page offset 4, which is the x86-64 `TSS.RSP0` slot.
  Only microcode reads that slot, during event delivery from ring 3. This is
  direct evidence that microcode-issued loads land in the DLA field.
- **why some events are immune:** an event's DLA is correct only when the microcode
  loads do not qualify for that event's umask and privilege filter. Enabling OS
  counting adds a second class of microcode loads (`0xfeb819c8`, store `0x803b`)
  that qualify for L1_HIT, ALL_LOADS, STLB_MISS and ALL_STORES.
- **microcode update:** `0xb000040` (2021-05-19) is the newest revision for
  sig `0x406f1`/pf `0xef` in intel-microcode 3.20260210. No newer microcode
  exists to test. No public BDM/BDF erratum describing this was found.

**Working substitute on this host:** `event=0xd1, umask=0x67` covers
L1_HIT, L2_HIT, L3_HIT, L3_MISS and HIT_LFB, with `exclude_kernel=1`. Its retired
count per round matches ALL_LOADS within +0.04% (mixed), +0.0% (L1), +0.3–0.5%
(L2/L3/DRAM chase), it counts no stores, and 98.7–100% of its addresses fall in
the owned buffers. Intel does not document this combined umask, and it excludes
loads with an unknown data source, such as UC or IO loads. libcram adopted it
as `sampling::broadwell_x::configs`; see [governor/](governor/README.md) for
acceptance through the libcram BPF path.

## 2. CPU cost of PEBS + BPF + collector

Harness: `crates/libcram-core/examples/pebs_overhead.rs`. It uses the real
`PebsSampler` and BPF ELF, with L1_HIT plus ALL_STORES, `:u`, precise 2,
`wakeup_events=1`. The workload thread runs interleaved ABBA blocks of
identical work, 30 pairs per period, with several periods interleaved. Two
quantities are measured:

- the workload thread's CPU time (`CONFIG_IRQ_TIME_ACCOUNTING` is unset, so
  NMI/PMI time is charged to the thread)
- the time stolen from it, measured by a per-iteration RDTSC gap detector

The collector thread's CPU time is measured directly and is not differenced
against a baseline. Full output is in `overhead/analysis.txt`.

Cost per sample, from three independent estimators:

| Estimator                             | ns/sample | 95% CI      |
| ------------------------------------- | --------- | ----------- |
| stolen-time (RDTSC gap) regression    | 4177      | 3608–4770   |
| thread CPU-time regression, jitter loop | 4516    | 3973–5045   |
| thread CPU-time regression, tight loop | 5145     | 3798–6625   |

The BPF share of that cost was isolated with `overhead/jit.c`, which uses the
same loop with no BPF program:

| Kernel path                          | stolen time per sample |
| ------------------------------------ | ---------------------- |
| PEBS + PMI/NMI, sample discarded     | 4376 ns (3859–4523)    |
| PEBS + PMI/NMI + native perf ring    | 5468 ns (5206–5935)    |
| libcram BPF program + BPF ringbuf    | ≈ 4200–4500 ns         |

The BPF program and ringbuf add no cost that this method can resolve, which
puts it below about 0.3 µs per sample. Almost all of the cost is the PEBS assist
plus the PMI/NMI round trip on this mitigated Broadwell host.

The userspace collector, polling every 100 ms, used 0.08–0.32% of one core. That
figure includes the warm-up from re-attaching every block, so it is an upper
bound.

Measured overhead at the default period of 2,000,003:

| Workload                            | samples/s | thread CPU delta (95% CI) | collector | total        |
| ----------------------------------- | --------- | ------------------------- | --------- | ------------ |
| light (1 load + 1 store + RDTSC/iter) | 105     | +0.38% (+0.04, +0.69)     | 0.13%     | ≈0.5% (<1%)  |
| L1 streaming (1 load + 1 store/iter) | 2242     | +1.29% (+0.16, +2.12)     | 0.18%     | ≈1.2–1.5% (>1%) |

The regression gives 2242 samples/s × 3.8–6.6 µs = 0.85–1.48%. Adding the
collector cost puts the streaming case above 1% even at the lower bound.

**Gate:** total cost is about samples/s × 4.5 µs plus about 0.15% for the
collector. Staying below 1% of one core requires **≤ ~1,700 samples/s per
monitored core**. That limit uses the upper CI of 5.05 µs per sample, and
1,500/s is recommended for margin. A fixed period cannot guarantee this,
because a BDX core can retire more than 4×10⁹ loads/s. Meeting the gate needs
a rate governor: adjust the period from `event_counts()`, or with
`PERF_EVENT_IOC_PERIOD`, to stay under the cap. That governor is now
implemented. Its collector costs 0.29–0.33% of a core, which lowers the
default cap to 1,200 samples/s per CPU (worst steady state 0.93%); see
[governor/](governor/README.md).

## Reproduce

```sh
# on s5, in a fresh owned directory
gcc -O2 -fno-pie -no-pie -o pebsdiag pebsdiag.c && sh run_matrix.sh > dla-matrix.log
gcc -O2 -o jit jit.c && ./jit 30 20011 30 26214400 nomap > jit-nomap.log
pebs_overhead program.bpf 30 25 l1-hit 2000003,200003,50021,20011 30 100 100 jitter
pebs_overhead program.bpf 30 25 l1-hit 2000003,1000003,500009 30 600 100
python3 analyze.py jitter-l1hit.log work-l1hit.log
```

Shared-host caveat: block-to-block noise is about ±5–10% from turbo and
co-tenants. The conclusions above rest on paired ABBA medians, bootstrap CIs and
regression across sample rates, not on any single difference.

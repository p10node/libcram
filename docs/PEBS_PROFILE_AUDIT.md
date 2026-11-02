# Auditing a PEBS profile for a new CPU model

libcram has no portable PEBS event selector. Each CPU model needs its own
audited profile (`telemetry::sampling::<model>`), and only Broadwell-X
(family 6, model 79) has one today. This procedure is how that profile was
accepted. Repeat all of it for any new model, on the machine that will run
the workload.

## Rules

- Use a physical host. A guest may report `max_precise=0` or virtualize PEBS
  differently (`S5_VM_ACCEPTANCE.md`).
- Sample only processes you start and own, pinned to fixed CPUs (workload and
  collector on different cores). Change no sysctl, microcode, kernel or other
  workload.
- Record, with the evidence:
  - CPU family, model, stepping and microcode;
  - kernel version and `PEBS fmt` (from `dmesg`);
  - `/sys/bus/event_source/devices/cpu/caps/max_precise`;
  - `perf_event_paranoid` and `perf_event_max_sample_rate`;
  - SHA-256 of every binary and of the BPF object.

## Steps

Each step names the tool used for Broadwell-X and its pass criterion.
Examples are in `crates/libcram-core/examples/` and build on Linux x86_64
with `--features ebpf`. They refuse any CPU but Broadwell-X
(`broadwell_x::detected()`), so adapt them to the new profile.

1. **Candidates.** Take retired-load and retired-store events with a data
   linear address (DLA) from the vendor's event list for the exact model. On
   Broadwell-X, `MEM_UOPS_RETIRED.ALL_LOADS` was the obvious choice and was
   wrong. Do not assume it is right on a new model either.
2. **Data addresses.** Use `evidence/2026-11-01/dla-root-cause/pebsdiag.c` to
   sample owned heap, `.bss`, stack and DRAM-chase buffers, user-only.
   - Pass: at least 99% of samples land in the owned buffers.
   - Record kernel-inclusive results separately. libcram always excludes the
     kernel.
3. **Coverage.** Run the `pebs_acceptance` example with the `l1` and `mixed`
   workloads.
   - Pass: the load event counts at least 0.99 of the retired loads, and the
     store event at least 0.99 of the stores.
   - Pass: no dropped or rejected samples, and at least 95% of read samples in
     the read regions.
4. **Paired periods.** Run `evidence/2026-11-01/governor/collide.c` with equal
   periods and with `companion_period`. Equal periods must reproduce the
   loss, or the model does not share PEBS fmt2's behaviour, which you then
   record.
   - Pass: companion periods deliver at least 99% of the expected samples.
5. **Attribution bias.** Run `evidence/2026-11-02/source-split/srcsplit.c`
   and record the per-region estimates. Store sample counts are not used as
   write evidence on any model. Writes come from `WriteSampler`, which needs
   Linux 6.7 or newer.
   - Pass: the `write_policy_acceptance` example prints `WRITE_POLICY_PASS`
     (A and B last reported write-free; C and D never write-free or
     read-dominant).
6. **Burst bound.**
   - Pass: `evidence/2026-11-02/burst-budget/budget.c` holds the
     `PERF_EVENT_IOC_REFRESH` limit under a burst.
   - Pass: the `pebs_burst` example never exceeds
     `process_pebs::sample_budget` samples per CPU in one governor interval
     (two intervals in a sliding 100 ms window).
   - On Linux 6.10 or newer this also shows that the BPF program's nonzero
     return keeps the limit working.
7. **CPU cost.** Run the `pebs_overhead` example: fixed periods with `jitter`
   for the per-sample cost regression, then `cap=N` for the governed steady
   state. Analyse with `evidence/2026-11-01/governor/analyze.py`.
   - Choose the cap so that cap × per-sample cost (upper 95% CI) + collector
     CPU stays below 1% of one core, and pass it to
     `ProcessPebsCollector::with_rate_limit`. The default,
     `sampling::DEFAULT_MAX_SAMPLES_PER_CPU_SEC` (1,200), was derived for
     Broadwell-X.
8. **Profile code.** Add a `sampling::<model>` module like `broadwell_x`:
   - raw configs: `configs`, plus `load_configs` for write-tracked pipelines;
   - an exact `detected()` check on vendor, family and model;
   - doc comments that cite the evidence directory.

   Never select a profile from a family-wide heuristic.

A failed step leaves the model unsupported. Callers must check the profile's
`detected()` before passing its configs to `ProcessPebsCollector`. Without an
audited profile, leave PEBS sampling off; `OwnedAllocation::profile` and
explicit samples still work.

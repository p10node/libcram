# Hardware acceptance checklist

These are deployment gates, not claims that ordinary CI has physical CRAM/PEBS.
Current recorded outcome: see `IMPLEMENTATION_STATUS.md` (earlier gates:
`VERIFICATION.md` and `evidence/2026-10-09/`). The available Linux container
lacks CPU PMU/CXL/CRAM; real BPF load returned EPERM. The entropy verdict and
PEBS cost gates pass only on the measured machines, and no CRAM device was
available. These checklist items are therefore still open, not waived by
passing CI.

Run only against an explicitly owned test workload and the intended Linux kernel.
Do not change system-wide NUMA/perf settings or induce pressure on production data.

1. Record CPU model, kernel, page size, online/allowed NUMA nodes, CXL driver,
   CRAM capability provenance, physical and logical capacity, and compression
   accounting semantics. An ordinary CXL memory expander is a negative control.
2. Allocate a small owned block on each admitted tier and query actual placement
   through `move_pages` (query mode). Verify fallback and exact error reporting
   when placement is denied by the deployment's seccomp/cpuset policy.
3. Migrate a bounded owned batch between two real nodes. Inspect every page's
   status, not just the syscall return. Confirm content and virtual addresses are
   unchanged. Repeat while unrelated owned allocations are created/freed.
4. Supply synchronized zero, ASCII and pseudorandom snapshots to entropy/policy.
   Verify write-heavy/high-entropy stays on DRAM and low-entropy/read-heavy
   placement changes only when CRAM is positively identified and healthy.
5. Load the built Aya BPF ELF on a supported PMU. Configure independently verified
   load/store event encodings (`PEBS_PROFILE_AUDIT.md`). Store samples are
   biased on Broadwell-X, so take write evidence from `WriteSampler`
   (`Bridge::track_writes`, Linux ≥ 6.7). Execute known read-only and
   write-only workloads over separate page ranges, then compare rolling
   per-page observations. Check dropped samples, privilege errors, CPU hotplug
   and process teardown.
6. Measure profiler CPU consumption over sustained sampling. The <1% single-core
   target must include kernel BPF cost and userspace collection; an idle process
   or missing sampler is not evidence of compliance.
7. Benchmark the 4KB exact entropy policy verdict (`entropy < 4.5`) for zero,
   realistic text and pseudorandom data with `just entropy-budget`. Require the
   calibrated SLA, max(1 microsecond, 1.5x the same-run scatter-store floor),
   for every distribution, not just the zero fast path. Record the strict
   1 microsecond result, the full entropy value and the near-threshold
   diagnostic as well.
   Retain compiler flags and confidence intervals; emulation results are not
   native CPU performance evidence.
8. Exercise Normal/High/Critical/Emergency with a hardware-approved fault/pressure
   fixture. Verify async callbacks, bounded work, fallback and recovery. Do not
   equate a canary overwrite with hardware poison, nor process allocation bytes
   with global compressed-device occupancy.
9. Validate C, C++ PMR, Python buffer views and Go Close behavior against the same
   release library. Exercise callbacks during allocation and concurrent teardown.
10. Run a sustained workload plus fault injection and inspect tail latency,
    memory leaks, map capacity limits and per-page migration failures before
    promoting the SDK to production.

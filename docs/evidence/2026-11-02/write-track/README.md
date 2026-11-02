# Sampled write tracking with userfaultfd async write-protect (s5, 2026-11-02)

Same host and rules as `../source-split`; no PMU is used. `wptrack.c` runs the
same four-region workload. Each 100 ms interval write-protects a random
sample of pages (stratified per region, without replacement) with
`UFFDIO_WRITEPROTECT`. At the end of the interval it reads the uffd-wp bit
(`/proc/self/pagemap` bit 57); a page that lost the bit was written.
`UFFD_FEATURE_WP_ASYNC` makes the kernel resolve the write fault itself, and
the fd uses `UFFD_USER_MODE_ONLY`, so no privilege beyond the process's own
memory is needed. Raw output: `wptrack.log` (20 intervals, 3 repetitions).

| Region | Truth                  | Written fraction, all runs and sample sizes |
| ------ | ---------------------- | ------------------------------------------- |
| A      | read-only, L1          | 0.0000                                      |
| B      | read-only, DRAM chase  | 0.0000                                      |
| C      | written, L1            | 1.0000                                      |
| D      | written after a stall  | 1.0000                                      |

The policy-relevant classification is exact where PEBS stores failed
(`../source-split`). An earlier run sampled with replacement and reported
false writes for re-sampled pages; that probe bug is fixed and its log was
replaced.

Cost (tracking syscalls; the in-kernel write faults are inside the workload):

| Sampled pages per 100 ms | Tracking CPU  | Workload steps/s (k=0: 7.27–7.52 M) |
| -----------------------: | ------------- | ----------------------------------- |
|                       16 | 0.070–0.079%  | 7.44–7.55 M                         |
|                       40 | 0.155–0.156%  | 7.39–7.54 M                         |
|                      136 | 0.476–0.493%  | 7.41–7.51 M                         |

≈3.6 µs per sampled page (protect, pagemap read, unprotect), so the cost is
set by the sample budget, not by the write rate. Workload throughput is within
run-to-run noise.

## End to end through the bridge (`examples/write_policy_acceptance.rs`)

The same four regions as owned allocations, with governed PEBS retired loads
(`broadwell_x::load_configs`, no store event) and `Bridge::track_writes`
(`WriteSampler`, 16 pages per 100 ms). Every bridge observation is checked
after the first second; log `write-policy.log`, three 10 s runs:

| Region | Observations | Reported write-free | Last evidence | Expected     |
| ------ | -----------: | ------------------: | ------------- | ------------ |
| A      |       84–123 |              84–122 | write-free    | write-free   |
| B      | 15492–15716  |         15308–15520 | write-free    | write-free   |
| C      |      505–541 |                   0 | written       | never free   |
| D      |      213–254 |                   0 | written       | never free   |

`WRITE_POLICY_PASS` in 3/3 runs. The few A and B observations not reported
write-free come before three write-free checks exist.

No observation of any region satisfied `reads / 4 > writes`. That needs four
read samples on one page within the bridge's 100 ms window. At the governed
rate (≈1,000 samples/s per CPU) spread over 32 K pages this does not happen.
This is a property of the existing per-page policy, independent of the write
signal, and is reported as open.

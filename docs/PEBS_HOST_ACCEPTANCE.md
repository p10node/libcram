# Owned PEBS host evidence, 2026-10-09

## Boundary and result

This report concerns an explicitly launched, CPU-pinned process on physical
`s5`, not the existing guest PMU or unrelated host workloads. Host: Linux
6.8.0-146-generic, GenuineIntel family 6/model 79/stepping 1, Xeon E5-2683 v4.
The host reports `max_precise=3`; its unchanged `perf_event_paranoid=4` did not
prevent this privileged, process-scoped probe. The guest still reports no usable
precise sampling. No global perf/security settings, existing VM definitions or
other workloads were changed.

**PASS (narrow):** six executions using `MEM_LOAD_UOPS_RETIRED.L1_HIT` (raw
0x01d1) and `MEM_UOPS_RETIRED.ALL_STORES` (0x82d0), period 2,000,003, attributed
nonzero read and write samples to distinct owned mappings, with zero stores to
the read-only workload region and zero reads from the write-only workload region.
The mappings were initialized before attaching the sampler. The program checks
all those conditions and exits nonzero on missing or cross-attributed samples.
The workload issues real volatile loads/stores; measurements are not synthesized.

**FAIL (kept):** the original `MEM_UOPS_RETIRED.ALL_LOADS` (0x81d0) configuration
produced PMU counts and BPF events but no reads attributable to the owned region.
Independent `perf record -d` also showed suspicious low addresses for this event
and the kernel `mem-loads` alias. L1-hit sampling produced normal userspace
addresses. This is evidence against accepting the all-loads configuration on
this host, not a proven diagnosis of the CPU/firmware/kernel address issue. The
probe keeps all-loads as its default and does not silently switch to the passing
but narrower event. L1-hit counts exclude misses and cannot justify an unbiased
all-memory read/write ratio, CRAM workload attribution or universal CPU support.
(Current `pebs_acceptance` defaults to `retired-loads`, `0x67d1`; see 2026-11-01 below.)

**NOT ESTABLISHED:** total active telemetry below 1% of one core. Three ABBA
cycles compared six sampled and six baseline executions of the same workload.
Median elapsed time per round was about 841,442 ns baseline and 830,498 ns active:
a measured change of -1.30%. A negative observed cost demonstrates noise, not
negative collector cost or compliance. The shared-host throughput diagnostic
includes elapsed workload slowdown but is not an isolated, attributed accounting
of all kernel sampling and userspace CPU work. The strict acceptance gate remains
open; it was not replaced by this proxy or by a replay-only collector benchmark.

## 2026-11-01: root cause and measured CPU cost

See [evidence/2026-11-01](evidence/2026-11-01/README.md). Two findings:

- **ALL_LOADS:** the CPU (hardware plus microcode) writes the address of a
  microcode-issued load into the PEBS DLA field whenever that load qualifies for
  the event's umask and privilege filter. The perf config, the kernel, libcram,
  and missing DLA support are each ruled out. `event=0xd1,umask=0x67`
  (user-only) matches the ALL_LOADS count and has valid addresses on this host.
- **CPU cost:** each sample costs ≈4.5 µs (95% CI 3.6–5.1). The BPF part is
  below what the method can resolve. The collector adds ≤0.3% of a core at a
  100 ms poll. Staying <1% requires ≤~1,700 samples/s per monitored core. At
  the fixed period of 2,000,003, a light workload costs ≈0.5% (PASS) and an
  L1-streaming workload costs ≈1.2–1.5% (FAIL).

## 2026-11-01: profile adopted and rate governor

See [evidence/2026-11-01/governor](evidence/2026-11-01/governor/README.md).

- **Profile:** `sampling::broadwell_x::configs` uses `0x67d1` loads and
  `0x82d0` stores, user-only. Through the real PebsSampler + BPF path, load
  counts match the loads the workload retires (1.0005 L1, 1.009 L1+DRAM), and
  99.0–99.97% of read samples land in the owned read regions. ALL_LOADS still
  gets 0%. With mixed-latency loads, PEBS samples concentrate on loads that
  retire after a stall (0.09% of samples are L1 hits against 88.9% of loads).
- **Period pairing:** with equal load and store periods, PEBS format 2 dropped
  almost every record of a loop with balanced loads and stores (0 of 5,368 per
  event, kernel path without BPF). libcram now rejects pairs that are not
  distinct and coprime, and `companion_period` gives store ≈ load·65/64.
- **Governor:** `ProcessPebsCollector` caps samples per monitored CPU (default
  1,200/s) with one shared period scale via `PERF_EVENT_IOC_PERIOD`. On the
  worst-case L1-streaming loop the steady rate was 1,050/s (max 1,130) and the
  collector cost 0.29% of a core. Worst steady state = 1,200 × 4.99 µs
  (upper CI) + 0.33% = **0.93% of one core: PASS**. A cap of 1,500 measured
  1.08% worst case and FAILS.
- **Shared program per CPU**
  ([evidence](evidence/2026-11-01/shared-program/README.md)): threads on one
  CPU share one loaded BPF object. Attaching a new thread costs 0.13 ms of
  collector CPU instead of ≈43 ms, and a full attach + detach cycle costs
  0.15 ms. Re-measured gate at cap 1,200: steady 1,057/s, collector 0.27%
  (max 0.29%), worst steady state **0.89%: PASS**.
- **Not covered by <1%:** ≈40–50 ms of collector CPU once per selected CPU,
  the first ≤100 ms at the base period, and load bursts shorter than the
  100 ms governor interval (bounded only by the kernel's sample-rate limit).

## 2026-11-02: write evidence and burst limit

See [evidence/2026-11-02](evidence/2026-11-02/README.md).

- **Store bias:** on a four-region workload, PEBS store samples landed on
  the store retiring after a DRAM stall. A write-heavy, cache-resident region
  was estimated write-free and classified read-dominant in 6/6 runs. Splitting
  loads by data source (`0x41d1`/`0x06d1`/`0x20d1`) did not remove the load
  bias and lost 8–50% of samples silently.
- **Write evidence:** `WriteSampler` write-protects one random page per
  allocation every 100 ms (async userfaultfd, Linux ≥ 6.7, no privilege) and
  reads the uffd-wp bit back. `Bridge::track_writes` uses it instead of store
  samples and fails closed until three write-free checks exist. End to end
  with governed PEBS loads only: read-only regions reported write-free,
  write-heavy ones never (`WRITE_POLICY_PASS` 3/3). Cost ≈3.6 µs per sampled
  page, 0.070–0.079% of a core for 16 pages per 100 ms (standalone probe).
- **Burst limit:** `ProcessPebsCollector` arms every event with
  `PERF_EVENT_IOC_REFRESH` limits and regrants them each interval, so no CPU
  takes more than 240 samples per 100 ms (2 × cap). The governor alone allowed
  625–2,884 per window, averaged 1,664–1,724 samples/s after idle (above its
  cap), and failed 2/3 streaming runs with ring loss. Governed steady state:
  1,066 samples/s, collector 0.28%, worst case **0.90%: PASS**.
- **BPF:** the programs now return 1 (Linux ≥ 6.10 skips the limit on 0);
  per-sample cost 4,954 vs 4,959 ns, no measurable change.
- **Procedure:** [PEBS_PROFILE_AUDIT.md](PEBS_PROFILE_AUDIT.md) lists every
  step and criterion for a new CPU model.

## Independent all-load diagnostic follow-up

A [standalone C/perf diagnostic](evidence/pebs-address-diagnostic/README.md)
reproduced zero owned ALL_LOADS addresses at precise levels 1, 2 and 3 with
userspace-only sampling, while its L1-hit control attributed all 1733 samples.
This removes libcram/BPF as a necessary cause of that reproduction, not the
remaining CPU/firmware/kernel uncertainty. The initial non-user-filtered run
also failed L1-hit and is retained separately. No event substitution or overhead
acceptance is inferred from this investigation.

## Production fix discovered by the probe

Initially the raw counters advanced but no BPF events arrived. Linux's Intel PMU
can choose large-PEBS auto-reload batching with zero `wakeup_events`; context-switch
batch drains can bypass the BPF overflow handler. Setting `wakeup_events=1`, with
watermark mode disabled, disqualifies that batching path. The focused perf-attr
regression failed on the previous zero value, then passed after the fix. Native
execution subsequently produced BPF events. This is distinct from the remaining
all-loads address-attribution failure.

References inspected:
- https://github.com/intel/perfmon/blob/main/BDX/events/broadwellx_core.json
- https://github.com/torvalds/linux/blob/v6.8/arch/x86/events/intel/core.c
- https://github.com/torvalds/linux/blob/v6.8/arch/x86/events/intel/ds.c

The sampler now exposes raw event counts to distinguish an unscheduled counter
from missing samples. `AccessWindow::snapshot` provides a bounded, expiring,
stably ordered diagnostic view without exposing its mutable retained state.

## Managed own-process collector follow-up

The new `managed_pebs_acceptance` example executed on native `s5` and in the
owned guest. [Logs and source hashes](evidence/2026-10-09/managed-pebs/) record:

- Thread sampler membership **1 → 2 → 1** around an owned helper thread.
- Native hot-page attribution: **20,889 reads**, **24,243 writes**, no opposite-kind
  samples in either controlled range in the recorded run.
- Actual `/proc/self/fd` counts return to baseline after exited-thread cleanup,
  collector destruction and deliberate partial-attachment failure.
- Guest precise-sampling rejection is reported as `MissingCapability`, not a
  healthy zero-sample stream; failed attachment leaves no extra descriptors.

Both recorded commands exited 0. These checks qualify lifecycle and this
explicit L1-hot profile only; all-load attribution and total CPU overhead remain
open. The follow-up has its own source manifest, separate from earlier probes.

## Reproduction and retained evidence

Build on Linux x86_64 with Rust 1.89+:

```sh
cargo build -p libcram-core --example pebs_acceptance --features ebpf \
  --release --locked
# ELF must be built from crates/libcram-ebpf by the existing ebpf gate.
# Run only on an owned process on an audited Broadwell-X host:
target/release/examples/pebs_acceptance /owned/program.bpf 12 2000003 all-loads
target/release/examples/pebs_acceptance /owned/program.bpf 12 2000003 l1-hit
target/release/examples/pebs_acceptance /owned/program.bpf 12 2000003 l1-hit baseline
```

The first command's attribution failure is an expected **failed acceptance
case**, not a suite PASS. Baseline mode explicitly prints `BASELINE_ONLY` and
never claims sampling acceptance. Another CPU model is refused before attachment.
The process attaches only to its own TID, pins only itself, and owns/unmaps both
buffers and perf descriptors. It does not attach system-wide.

Raw logs, exit statuses, full timing values, hashes, host inventory and the exact
one-off runner are in `evidence/2026-10-09/pebs-host/`. `results.json` hashes each
log and the tested source files; `host-info.json` hashes the actual executable and
BPF ELF. The runner text records this run's owned temporary paths; recreate those
artifacts or substitute newly owned paths for a later reproduction.

No node-specific CRAM driver paths or CXL devices were discovered on this host.
The passing L1-hit probe therefore does not establish CRAM migration, physical
compression savings or device-health validation.

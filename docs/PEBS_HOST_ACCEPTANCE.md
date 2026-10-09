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

**NOT ESTABLISHED:** total active telemetry below 1% of one core. Three ABBA
cycles compared six sampled and six baseline executions of the same workload.
Median elapsed time per round was about 841,442 ns baseline and 830,498 ns active:
a measured change of -1.30%. A negative observed cost demonstrates noise, not
negative collector cost or compliance. The shared-host throughput diagnostic
includes elapsed workload slowdown but is not an isolated, attributed accounting
of all kernel sampling and userspace CPU work. The strict acceptance gate remains
open; it was not replaced by this proxy or by a replay-only collector benchmark.

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

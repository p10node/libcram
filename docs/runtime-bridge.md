# Bounded A–D runtime bridge

`runtime_bridge::Bridge<R, D>` implements the unchanged `runtime::Collector`.
`RawSource` and `DeviceSource` are bounded synchronous interfaces for injection;
the real implementations delegate to Linux+`ebpf` `ProcessPebsCollector::poll`
and Unix `DirectoryDeviceAdapter::read`, respectively. No implicit PMU event,
CPU discovery, telemetry rewrite, payload sampling, or worker is added.

## Minimal application wiring

Deployment-specific values below are supplied by the caller, not defaults:

```rust,ignore
use libcram_core::{
    allocator::AccessHint,
    device_adapter::DirectoryDeviceAdapter,
    runtime::{Runtime, RuntimeConfig},
    runtime_bridge::{Bridge, BridgeConfig},
    telemetry::process_pebs::ProcessPebsCollector,
};

// Linux + ebpf. Only selected CPUs; ONLY explicitly audited PMU configurations.
let raw = ProcessPebsCollector::new(
    trusted_object_bytes, selected_cpus, audited_configs, max_attachments, 8192,
)?;
let device = DirectoryDeviceAdapter::new(trusted_absolute_node_directory)?;
let (bridge, publisher) = Bridge::new(raw, device, BridgeConfig::default())?;
let mut runtime = Runtime::new(allocator, bridge, RuntimeConfig::default())?;
let mut allocation = runtime.allocate_owned(size, align, AccessHint::WriteHeavy)?;
// Unique borrow: no concurrent unsynchronized reads of mutable application data.
allocation.as_mut_slice().fill(0);
// Known initialized zeros have entropy zero; real applications supply independently
// synchronized, current entropy metadata (finite [0,8]), not an invented constant.
publisher.register(allocation.observation_token(0.0)?)?;
runtime.tick()?; // or consume runtime.start() for the managed worker
// Refresh metadata before token TTL, after synchronizing with payload mutation.
// Check errors/status; successful publication is enqueue, not registration ACK.
```

Set bridge `max_output <= RuntimeConfig.max_observations`; defaults both use 32.
A batch not fully drained before the next `device_update` is rejected/cleared,
not silently replaced. Bridge token TTL should be no longer than runtime sample
TTL. Tokens are weak, expose only `range() -> Option<Range<usize>>` and
`created_at()`, and never keep mappings alive between calls. Runtime rechecks
actual allocator registry identity at migration time (including foreign tokens).
Publisher/bridge registration does not itself prove membership in that runtime.

## Admission, clocks and generations

Every update drains at most `input_capacity` token commands, reads a new device
snapshot, then polls the raw source once and prepares the entire output batch.
`poll` only drains that batch. Invalid device data/raw failure discards pending
observations and all history before returning an error. Runtime revokes cached
health on **every** failed tick, including oversized poison lists, before any
observation migration. Successful physical snapshots map exact used/capacity
values. More than 64 poison entries is rejected before dedup, never truncated.

Events must belong to the creating process TGID (forked bridge use fails), lie
within live registered payload bounds, be no earlier than token creation, and
fall in `(now - 100ms, now]`. Future, stale, pre-token, foreign TGID and out-of-range
events are dropped. A per-batch calibration captures `Instant` immediately before
`CLOCK_MONOTONIC`; event time is that Instant minus the original raw timestamp's
age, **not receipt time**. Calibration latency conservatively shifts the result
earlier and may reject near-token-boundary samples; it cannot make an old event
younger. The raw and Instant clocks must advance together (the supported Linux
path uses monotonic clocks). These are not wall-clock device report timestamps.

Newest metadata replaces an allocation's old registration and rolling history;
out-of-order older tokens cannot roll it back. Internal checked generation IDs
and weak Arc identity separate registrations; allocator address reuse cannot
inherit a previous allocation's history. Events predating a replacement token
are rejected even if the virtual address matches. Allocation death/TTL prunes
registration/history on the next update. No explicit unregister is required.

Counts are unweighted samples aggregated per payload page and token generation
across a rolling 100ms window, not estimated memory-operation totals. Each batch
can re-emit still-valid window counts; its timestamp remains the newest original
event timestamp, never the current poll time. Runtime's existing policy performs
migration. Test fixtures demonstrate actual read→write policy reversal through
this bridge and a fake NUMA backend, not actual page movement by the OS.

## Bounds and loss

| Limit | Default | Accepted maximum |
|---|---:|---:|
| registrations | 128 | 4096 |
| input token queue / commands per tick | 128 | 4096 |
| raw events per read | 8192 | 8192 |
| total retained history events | 8192 | 65536 |
| output observations per batch | 32 | 128 |

All limits must be positive. Retained objects and temporary output maps are
bounded; vector spare capacity/allocator overhead and caller-owned source Vecs
are not a byte-exact process memory cap. A custom source must itself bound its
allocation, I/O and work before returning. Association scans are bounded but
linear in registrations for each event; no overhead target is claimed.

Input queue or registration overflow latches failure permanently; rebuild the
bridge/runtime to resume. This includes publisher send after consumer shutdown.
Raw loss, history/output overflow, device errors, and incomplete prior batches
clear history/output and fail the current tick; a later valid new snapshot/raw
batch can recover. Real ProcessPebsCollector reports observed loss as error;
undetectable PMU scheduling loss remains outside this contract. A concurrent
publisher failure is observed at the next bridge boundary, not an atomic fence
against a runtime batch already handed off. No guarantee of complete sampling.

Poison IDs deduplicate within each successful snapshot and against the last
successfully returned snapshot, including across failed reads. An ID absent in
one successful snapshot then reappearing is reported again. This bounded
consecutive-snapshot dedup is not a permanent device-wide identity database;
the allocator's existing poison latch remains authoritative.

## Runtime lifecycle and evacuation

Runtime destruction (including worker unwind/stop) invalidates cached device
health. A retained ManagedRuntime may still allocate after stop but cannot use
stale healthy admission. Stop does not synchronously evacuate all old mappings.

After a successful tick, pressure/stale-health with no observations now invokes
the existing migration engine using at most the remaining page quota and
`max_observations` registry records (engine capped at 128). Cursor-based passes
progress over later ticks. Managed mode prohibits legacy metadata; temporary
observation samples are cleared, so this path never revives undated promotion
metadata. Failed intake ticks revoke health and independently evacuate using
that tick's page/record quotas, so a continuously failing producer cannot strand
CRAM mappings indefinitely. The failed observation batch is never promoted.
Errors after event migration has started do not trigger another scan: accounting
may be incomplete, so retrying could exceed the quota. This is bounded
best-effort evacuation, not a hard completion deadline or full stop evacuation.
Syscalls/custom collectors/filesystem reads may block; Rust cannot enforce a
hard latency bound. Actual per-page failure accounting is still engine-owned.

## Evidence boundary

Owned fixture reports, including real DirectoryDeviceAdapter file-reader tests,
are **not real CRAM hardware**. Trust of physical capacity, producer provenance,
freshness, filesystem ancestry and report publication follows DEVICE_ADAPTER.md.
A new report is required every successful update; replay must not renew health.
Parent retains native Linux/perf/hardware and packaging acceptance. No universal
raw event profile, all-load coverage, <1% overhead or realCRAM result is claimed.

Local RED: oversized poison health retained; stop retained health; no-observation
pressure did not evacuate (each exit 101). Initial bridge API test was compile
RED (missing module); bridge differential subsequently passed. Boundary/limit
regressions were then run against the implementation, not claimed as individual
behavioral REDs. Full verification commands are reported in the commit handoff.

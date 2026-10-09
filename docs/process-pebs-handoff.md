# Managed process PEBS handoff

## API

Portable `telemetry::sampling::{AccessEvent, decode_event, ingest_event}`:
`AccessEvent { time_ns: u64, address: u64, tgid: u32, kind: AccessKind }`.
The decoder requires exactly 32 native-endian bytes, a nonzero address, a
u32-representable TGID and operation 1 or 2. Ingestion retains AccessWindow
admission semantics. Original timestamps/TGIDs are not rewritten.

Linux + `ebpf`: `telemetry::pebs::PebsSampler::poll_events(budget, callback)`
returns `PollReport`; malformed records consume budget without callback delivery.
Legacy `poll(window, budget)` shares the bounded drain and counts both decoder
and window rejection, including expired samples.

Linux + `ebpf`: `telemetry::process_pebs::{ProcessPebsCollector, ProcessPebsStatus}`:

```rust,ignore
let mut source = ProcessPebsCollector::new(
    trusted_object_bytes, selected_cpus, audited_configs,
    max_attachments, max_events,
)?;
let events = source.poll()?;
let status = source.status();
```

Construction validates inputs but does NOT attach; first poll attaches. Status
starts NotPolled, then NoSamples/Sampling or an explicit error status. Attach
errors distinguish known Unsupported/PermissionDenied (MissingCapability) from
AttachFailure. Aya errors that erase the underlying OS error remain AttachFailure
with their diagnostic text; neither is reported as NoSamples.

Caller must map events to owned allocation generations and reject samples older
than allocation creation. This API makes no allocator/runtime decisions.

## Bounds and lifecycle

- One sampler per `(TID, /stat starttime, selected CPU)`, maximum 32 samplers.
  Each sampler has one or two explicitly configured perf-event FDs; this is a
  sampler/ring quota, not a total-FD quota. BPF loader/map FDs are additional.
- Maximum 8192 drain attempts and output events per poll. Empty-ring attempts
  also spend budget. One record per ring turn with a persistent round-robin
  cursor prevents a permanently busy first ring starving later stable rings.
- Trusted object bytes must be nonempty and at most 4 MiB. CPU/config counts,
  duplicates, CPU integer range, periods, PMU shape and all limits are checked
  before internal allocation/attachment. Input Vecs already belong to caller;
  this is not a bound on their existing spare capacity or on arbitrary BPF map
  declarations. Only use the trusted libcram eBPF object, never untrusted ELF.
- Only `/proc/self/task` is scanned, bounded by remaining tuple capacity.
  Each stat read is limited to 4097 bytes, rejecting more than 4096. No other
  process inventory is traversed. Membership/starttime are checked again after
  new attachment. Existing generations are reconciled on every poll; exits and
  reuse drop old samplers before replacements are admitted.
- Any refresh, capacity, attachment, decode or observed ring-loss error clears
  all attachments and discards the whole current output batch. A subsequent
  poll retries. Drop owns/drops every sampler/FD without workers. A collector
  carried across fork is rejected when polled in the child.

## Honest limitations

Sampling covers **only selected CPUs** and **only explicitly audited raw events**.
No architecture profile is guessed. The parent's native Broadwell-X evidence
for L1-hit loads `0x01d1` and stores `0x82d0` is not evidence for all-loads
`0x81d0`, other CPUs, or universal access coverage.

Proc scans are not atomic. Threads created after a scan wait until the next
poll; short-lived threads can be missed. Churn can force fail-closed retries
and lose queued observations. TID/starttime is clock-tick identity, not a kernel
thread handle: post-open checks narrow, but cannot eliminate, the TID reuse
race around numeric perf_event_open. The final TGID check rejects foreign
observations, not an atomic guarantee against transient attachment during reuse.
Exits after a scan are recognized next poll. No process-wide snapshot or complete
coverage claim is made.

DROPPED is a best-effort cumulative per-CPU diagnostic, checked on all retained
rings after draining; it is not atomic and does not measure PMU scheduling loss
or kernel recursion suppression. A successful empty poll is not evidence of
hardware availability, workload inactivity, or zero memory accesses. Sampling
and attachment overhead are not benchmarked; there is no <1% CPU claim.

## Executed checks

Raw final output: [process-pebs-checks.log](process-pebs-checks.log).

All passed on the macOS host:

```
cargo test -p libcram-core --features ebpf
cargo clippy -p libcram-core --all-targets --features ebpf -- -D warnings
cargo check -p libcram-core --features ebpf --target x86_64-unknown-linux-gnu --all-targets
cargo clippy -p libcram-core --features ebpf --target x86_64-unknown-linux-gnu --all-targets -- -D warnings
git diff --check
```

RED observed: typed decoder missing; pure identity/planner/cursor functions
missing; bounded drain missing. GREEN: portable decoder differential and strict
malformation tests, old-window rejection accounting, bounded budget/zero budget,
owned identity fixtures covering exit/reuse/new identities/capacity, stat name
parentheses, and round-robin cursor. Final host suite includes 18 unit tests
plus integration tests. Linux paths typechecked/clippy-checked, **not executed**.

Parent still must run root native Linux attachment tests: new/exiting threads,
FD cleanup/partial attachment failure, permission denial, actual loss and decoded
address attribution with the audited events. Pure planning tests do not prove
kernel FD teardown or hardware behavior. No SSH/native test was attempted here.

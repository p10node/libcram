# Managed runtime handoff (A)

## Public API and integration

- `runtime::Collector: Send + 'static` retains `poll(&mut self) -> Result<Option<Observation>, Error>` and `device_update(&mut self) -> Result<Option<DeviceUpdate>, Error>`.
- `Runtime::new(CramAllocator, collector, RuntimeConfig)` takes exclusive allocator ownership, stops/joins its legacy migration monitor, clears undated legacy samples, and permanently switches metadata admission to observation tokens. No allocator reference or restart-monitoring escape is exposed.
- Manual mode: `Runtime::tick`, `allocate_owned`, `status`, `telemetry`. Background mode: consume it with `start() -> Result<ManagedRuntime, Error>`.
- `ManagedRuntime::allocate_owned` delegates to the same private `Arc<CramAllocator>` used by the worker. Existing and newly created owned allocations can outlive the worker. Allocation remains available after stop; collection does not restart.
- `ManagedRuntime::status() -> RuntimeStatus` reads shared last-completed-tick counters. `telemetry() -> TelemetrySnapshot` reads live allocator accounting. These are individual snapshots, not an atomic joint snapshot or hardware compression measurement.
- `ManagedRuntime::stop(&mut self)` wakes the interval wait, joins, and releases the owned collector and its descriptors. Repeated stop succeeds; `Drop` also stops/joins. Runtime destruction publishes `stopped`; successful stop returns only after collector destruction. Worker panic unwinding publishes stopped, and join returns existing `Error::Poisoned`; Drop cannot report that error.
- `OwnedAllocation::observation_token(entropy)` creates cloneable metadata with a **weak** allocation identity, validated finite entropy in `[0,8]`, and creation time. It does not extend the mapping lifetime or read payload bytes. Obtain a token before collecting events, refresh it within the configured TTL, and use the event's original monotonic timestamp.
- Once managed, legacy `record_sample` (including `OwnedAllocation::profile`, which submits a legacy sample) returns `Error::Unsupported`, even after stopping. Normal payload borrows and new observation tokens remain available.
- No actual hardware attachment and no production channel collector adapter are included. Parent-owned device/perf adapters should implement this unchanged trait, with their own bounded nonblocking intake (e.g. `sync_channel`/`try_send`). The lifecycle test demonstrates bounded channel ingestion without adding an adapter API.

## Admission and limits

Defaults: interval **100 ms**, sample/token TTL 1 second, at most 32 polled observations and 256 attempted pages per tick. Config accepts 1–128 observations, 1–4096 pages, nonzero TTL, interval at least 10 ms. The first worker tick is immediate; subsequent intervals are waits after work, not fixed-rate deadlines.

Each event must have `token.created <= observed_at <= now`, and both event and token age must be within TTL. Weak identity must upgrade, match the current allocator registry by Arc identity, and contain the sampled address in payload (not canary). Foreign allocations, expired owners, stale/future/pre-token samples and out-of-range addresses are dropped. Identity never relies on address reuse alone.

Only the payload page containing each accepted event is considered, using existing migration policy and page-result accounting. At most one page per observation is attempted; therefore the observation cap may be stricter than the page cap. Already-target pages may require no syscall. Observations already drained when the page budget runs out are counted as dropped; entries beyond the observation cap remain with the collector. This is per-tick bounded work, not a total allocation/queue/memory cap.

A device update with more than **64 poison entries** is rejected as `InvalidSample` before usage updates, poison reporting, observation polling or migration; it is never silently truncated. Runtime error/degraded counters update, but cached device health is unchanged for this rejection (and remains subject to its existing freshness policy). Ordinary collector failures revoke cached health. A poll failure prevents migration of the entire collected observation batch; device updates already applied before polling are not rolled back.

## Precise remaining limitations

- Collector methods **must be nonblocking and bounded**. Rust cannot force-abort a blocking trait implementation or NUMA backend. Stop waits for current work; the wakeable interval test does not establish a hard shutdown deadline for arbitrary collectors/syscalls.
- Allocation identity is held under the registry mutex during a per-event migration. Concurrent allocation/deallocation/telemetry can wait for that bounded operation; this is not lock-free or a real-time API. Backends must not reenter allocator registry APIs during migration.
- No concurrent raw payload reads occur in the runtime. Caller-provided entropy/counts are trusted synchronized metadata, not verified hardware evidence. Safe mutation uses the unique owned allocation borrow.
- Runtime does not migrate the entire registry on a pressure transition without observations; only admitted event pages are considered. Device pressure/freshness gates still apply to allocation and each migration decision. No automatic hardware discovery, perf attach, retry/backoff, collector queue metrics or producer supervision is supplied here.
- `errors` counts failed ticks, not individual kernel page failures. `pages_attempted`/`pages_moved` reflect migration reports, but detailed per-page/syscall error reports are not exposed by this runtime API. Accepted is admission/processing, not proof that migration succeeded.
- Legacy-monitor joining is tested separately; other allocator internals such as the pressure callback dispatcher are not replaced here. No edits to `allocator/pressure.rs`, telemetry adapters, or sibling/canonical worktrees were made.
- Evidence below is local macOS with the existing fake NUMA backend and real worker/ownership lifecycle. Linux and real CXL/perf hardware remain independent parent integration gates.

## Executed evidence / raw exits

New regression RED runs (each `cargo test -p libcram-core --test runtime <filter>`):

| Filter | RED exit | Reason | GREEN exit |
| --- | --- | --- | --- |
| `event_before_token_creation_is_rejected` | 101 | dropped 0 instead of 1 | 0 |
| `dynamic_owned` | 101 | missing ManagedRuntime allocation/status/telemetry methods (API compile RED) | 0 |
| `oversized_poison` | 101 | reached poll instead of rejecting oversized poison batch | 0 |
| `managed_ownership` | 101 | legacy sample accepted under managed ownership | 0 |
| `page_quota` | 101 | discarded drained event not counted | 0 |

Final commands executed after implementation:

```text
cargo fmt --all && cargo fmt --all -- --check
exit: 0

cargo test -p libcram-core --test runtime
exit: 0
14 passed; 0 failed; 0 ignored

cargo clippy -p libcram-core --all-targets -- -D warnings
exit: 0

cargo test -p libcram-core
exit: 0
All unit/integration/stress tests passed; no ignored tests; doc-tests passed.
```

The original eight runtime tests are retained (policy reversal, source failure, legacy monitor handoff, addressed-page migration, budgets, lifetime/identity/TTL and stop wakeup). Additional coverage exercises new allocation after start, shared live counters/accounting, explicit and implicit source destruction, repeated stop, retained owned payload, lower timestamp bound, expired token with fresh event, future event, foreign identity, invalid entropy and per-tick observation/page quotas. Additional TTL/foreign/default assertions passed directly against the existing admission logic; they are not claimed as new RED evidence.

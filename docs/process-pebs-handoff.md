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

### Broadwell-X profile, period pairing and rate governor

- `sampling::broadwell_x::configs(load_period)` returns the audited pair
  `0x67d1` (retired loads: L1/L2/L3 hit, L3 miss, LFB hit) and `0x82d0` (all
  stores), user-only, after `broadwell_x::detected()` confirms family 6
  model 79. It replaces ALL_LOADS `0x81d0`, whose PEBS address is wrong on
  this part. `broadwell_x::load_configs(load_period)` returns the load event
  alone, for pipelines that take writes from `WriteSampler`
  (`Bridge::track_writes`, see `runtime-bridge.md`).
- Two configured events must have distinct, coprime periods
  (`sampling::periods_independent`). `PebsSampler::attach`, `set_periods` and
  `ProcessPebsCollector::new` reject other pairs. Reason: PEBS format 2 drops
  both records when the two counters overflow within ~1,000 events of each
  other; equal periods on a loop with balanced loads and stores lost every
  sample. `companion_period(p)` gives the store period ≈ p·65/64, so read and
  write sample weights differ by only ~1.6%.
- `pebs::PebsProgram::load` loads the object once; `PebsProgram::attach`
  returns `PebsEvents` (perf FDs, counts, `set_periods`, sample limits) for
  one TID/CPU. `PebsSampler` is the single-target wrapper over both.
- `ProcessPebsCollector` runs a `RateGovernor` by default, capping each
  monitored CPU at `DEFAULT_MAX_SAMPLES_PER_CPU_SEC` (1,200). It reads PMU
  counts at most every `RATE_INTERVAL` (100 ms) inside `poll`, scales every
  period by one shared factor and reprograms samplers with
  `PERF_EVENT_IOC_PERIOD`; new samplers start at the governed periods.
  `with_rate_limit(None)` disables it before the first poll. `period_scale()`
  and `periods()` report the current weights: each sample stands for about
  `periods()[i]` events of its kind.
- With the governor on, every event also gets a kernel sample limit
  (`PebsEvents::limit_samples`, `PERF_EVENT_IOC_REFRESH`) starting at
  `INITIAL_GRANT` (2). Each interval, `PebsEvents::account` estimates the
  overflows used and `PebsEvents::grant` tops events back up, so one CPU's
  outstanding limits stay within `process_pebs::sample_budget` (2 × cap ×
  `RATE_INTERVAL`, plus one per spent event). An event that reaches its limit
  is disabled by the kernel until the next grant. The BPF programs return 1:
  Linux ≥ 6.10 skips the limit when they return 0.

## Bounds and lifecycle

- One `PebsProgram` (loaded BPF object, 256 KiB ring, DROPPED map) per
  selected CPU, loaded at that CPU's first attachment and shared by every
  thread attached there. One attachment per `(TID, /stat starttime, selected
  CPU)`, maximum 32, each with one or two explicitly configured perf-event
  FDs. This is an attachment quota, not a total-FD quota; BPF loader/map FDs
  are additional. Programs are not shared across CPUs: the kernel BPF ring
  takes a trylock in NMI context, so concurrent CPUs on one ring could lose
  reservations.
- Maximum 8192 drain attempts and output events per poll. Empty-ring attempts
  also spend budget. One record per CPU ring turn with a persistent
  round-robin cursor prevents a permanently busy CPU starving the others.
- Trusted object bytes must be nonempty and at most 4 MiB. CPU/config counts,
  duplicates, CPU integer range, periods, PMU shape and all limits are checked
  before internal allocation/attachment. Input Vecs already belong to caller;
  this is not a bound on their existing spare capacity or on arbitrary BPF map
  declarations. Only use the trusted libcram eBPF object, never untrusted ELF.
- Only `/proc/self/task` is scanned, bounded by remaining tuple capacity.
  Each stat read is limited to 4097 bytes, rejecting more than 4096. No other
  process inventory is traversed. Membership/starttime are checked again after
  new attachment. Existing generations are reconciled on every poll; exits and
  reuse drop old attachments before replacements are admitted.
- Any refresh, capacity, attachment, decode or observed ring-loss error clears
  all attachments and programs, discarding queued ring records and the whole
  current output batch. A subsequent poll reloads and retries. Drop
  owns/drops every program/FD without workers. A collector
  carried across fork is rejected when polled in the child.

## Honest limitations

Sampling covers **only selected CPUs** and **only explicitly audited raw events**.
No architecture profile is guessed or auto-selected. The audited Broadwell-X
profile (`0x67d1` loads, `0x82d0` stores) is not evidence for other CPUs.
ALL_LOADS `0x81d0` stays wrong on that part. With mixed-latency loads, PEBS
samples concentrate on loads that retire after a stall, so per-page sample
counts over-represent cache-missing loads
([evidence](evidence/2026-11-01/governor/README.md)). Store samples are biased
the same way, so a write-heavy cache-resident page can look write-free
([evidence](evidence/2026-11-02/source-split/README.md)).

Proc scans are not atomic. Threads created after a scan wait until the next
poll; short-lived threads can be missed. Churn can force fail-closed retries
and lose queued observations. TID/starttime is clock-tick identity, not a kernel
thread handle: post-open checks narrow, but cannot eliminate, the TID reuse
race around numeric perf_event_open. The final TGID check rejects foreign
observations, not an atomic guarantee against transient attachment during reuse.
Exits after a scan are recognized next poll. No process-wide snapshot or complete
coverage claim is made.

DROPPED is a best-effort cumulative per-CPU diagnostic, checked on all retained
rings after draining; it is not atomic and does not measure PMU scheduling loss,
kernel recursion suppression or PEBS records the kernel drops before the BPF
handler (format-2 collisions, which independent periods prevent). A successful
empty poll is not evidence of hardware availability, workload inactivity, or
zero memory accesses. Sampling cost on s5 Broadwell-X: ≈4.7 µs per sample on
the sampled core plus ≈0.3% of a core for the governed collector. At the
default cap of 1,200 samples/s per CPU the worst steady state is 0.90% of one
core ([evidence](evidence/2026-11-02/burst-budget/README.md)). One-time costs
are not in that figure: ≈40–50 ms of collector CPU per selected CPU (program
load), ≈0.15 ms per thread attach + detach, and up to 100 ms at the base
period after the first attachment. Bursts shorter than
one interval are bounded by the kernel sample limits above, not by the
governor.

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

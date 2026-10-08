# Core lane evidence and integration notes

## Delivered behavior

- NUMA sysfs discovery and injectable topology/backend. CXL membership does not
  establish compression; explicit operator/hardware provenance is required.
- Real Linux `mbind` and `move_pages`; page-range validation, negative syscall
  errno preservation, individual page status and partial migration reporting.
- Page-owned mmap allocations with checked sizes/alignment, registry ownership,
  best-effort binding fallback, trailing poison canary and owned Rust buffers.
- Bounded background migration (32 allocations / 256 pages per tick), circular
  allocation/page cursors, per-page accounting, mapping Arc leases, and diagnostic
  reports. Explicit access/entropy metadata only; no background payload reads.
- Owned `profile` borrows the buffer and samples at most its first 4096 bytes.
  This prefix estimate is not claimed to characterize all allocation contents.
- PSI-driven asynchronous callbacks on a separate worker, capacity-one queue,
  generation-based replacement, panic isolation, no callback under internal locks.

## Test-first evidence

Each API slice was introduced with a failing test invocation before its
implementation: topology, NUMA wrappers, poison guard, allocation, owned migration,
pressure callbacks, explicit owned profiling, background diagnostics. New Rust API
slices initially failed to compile because the requested API did not yet exist.
Additional behavioral RED -> GREEN regressions:

- `raw_sample_reversal_reclaims_first_page_immediately`: failed with 16384 CRAM
  bytes instead of zero; resetting the page cursor on new raw samples fixed it.
- Linux allocation failed with missing NUMA sysfs. The explicit
  `unavailable_sysfs_falls_back_to_uncompressed_uma` test also failed, then passed
  after adding the missing-sysfs-only UMA fallback.
- An early test assumed 4096-byte pages and failed on the macOS 16384-byte host;
  the fixture was corrected to use the runtime page size (test harness correction,
  not a production behavior fix).

Further regression/stress coverage exercises all canary bytes, ownership rejection,
invalid layouts, large alignments, actual payload writes, 64 concurrent threads
with 128 allocation/write/verify/free cycles each, partial migration errors,
background bounds and lease survival after owner drop, callback reentrancy and a
real callback panic followed by successful delivery from the same worker.

## Executed validation

- macOS ARM: `cargo test -p libcram-core --all-features`: 19 passed.
- Linux amd64 Docker on ARM host: same command: 21 passed.
- Linux real syscall probe: both `mbind` and `move_pages` returned kernel EPERM
  (errno 1). This proves error propagation, NOT successful physical migration.
- `cargo clippy -p libcram-core --all-targets --all-features -- -D warnings`
  passed on both platforms with a temporary module-scoped exemption for
  `clippy::chunks_exact_to_as_chunks` in the other worker's entropy/telemetry code.
  The unexempted composed gate failed on that external lint. No exemption is
  committed; the integration owner must resolve/re-run the composed gate.
- Owned Rust sources formatted and checked with rustfmt. Child traversal was
  disabled while testing read-only sibling module paths, to avoid modifying
  another worker's files.

The test workspace was temporarily narrowed to core. Final integration tests used
actual entropy/telemetry modules from the profiling worktree via temporary path
attributes (not fake PSI/entropy output). Earlier allocator-only slices used a
minimal PSI contract shim returning Unsupported from real reads. All temporary
attributes/shims and root workspace edits are removed before commit. The final
core crate therefore requires the profiling lane modules to be composed before
building. Aya 0.13.1 is optional and Linux-target-only under `ebpf`.

## Explicit limits

`cram_bytes` counts payload covered by accepted CRAM binding policy / successful
migration statuses; it is not compression ratio or an independent residency probe.
No CRAM hardware, successful physical NUMA migration, PEBS attachment, performance,
Miri, or sanitizer acceptance is claimed. The count/work bounds are not hard
wall-clock deadlines for kernel calls. No unsynchronized mutable payload scanning
is performed. Raw users must provide independently synchronized access/entropy
samples and stop accesses before free or allocator drop.

Callback replacement/unregistration is NOT quiescent: a selected/in-flight callback
may finish after unregister or allocator destruction. Its captured state must stay
valid through delivery; use Weak allocator references to avoid cycles. The bounded
queue drops notifications under load. Monitor destruction joins its worker;
callback workers are not joined, so arbitrary callback code cannot block allocator
teardown. Poison is a trailing canary checked on free, not arbitrary memory-error
containment or SIGBUS recovery. FFI should map the distinct Error variants, not
classify every error as OOM.

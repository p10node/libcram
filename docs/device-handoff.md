# D software handoff

## Scope and integration

Worktree verified before edits:
`$WORKSPACE/libcram-device`;
branch `task/device-adapter`; clean base
`f43fb9e7c64e5177c6e4193112c94c63d6e6f0d1`.
No SSH, canonical/sibling edits, manifest/lock changes, agents, runtime dependency,
or live sysfs writes were used.

Export: `libcram_core::device_adapter` (only `pub mod device_adapter;` added to
core `lib.rs`; parent resolves sibling module exports when merging).

Frozen API delivered:

```rust
pub struct DeviceReading {
    pub used_bytes: u64,
    pub capacity_bytes: u64,
    pub poison_pages: Vec<u64>,
}
pub trait DeviceAdapter: Send {
    fn read(&mut self) -> std::io::Result<DeviceReading>;
}
```

Additional API:

* Unix `DirectoryDeviceAdapter::new(node_directory: impl AsRef<std::path::Path>)
  -> std::io::Result<Self>`; implements `DeviceAdapter`.
* `parse_report(bytes: &[u8], now_unix_ms: u64)
  -> std::io::Result<DeviceReport>`.
* `DeviceReport { pub generation: u64, pub observed_unix_ms: u64,
  pub reading: DeviceReading }`.
* `MAX_REPORT_BYTES: usize = 16384`, `MAX_POISON_RECORDS: usize = 256`,
  `MAX_AGE_MS: u64 = 1000`.

Parent should wrap this trait in A's independent DeviceSource/DeviceUpdate
contract, retain one adapter across polls, submit physical usage and poison only
on success, invalidate immediately on every error, and never refresh healthy TTL
from a cached/replayed report. Duplicate poison IDs intentionally remain bounded;
existing allocator unique latch semantics must remain authoritative. No A runtime
files were present/required/modified.

## Protocol and failure behavior

See `docs/DEVICE_ADAPTER.md` for exact token schema, filesystem contract,
trust assumptions, errors, source clock rules and downstream TTL caveat.
`<absolute node>/cram/compression_enabled` must positively equal `1` or `1\n`.
`cram/libcram-report-v1` is our explicit adapter-specific **physical** report,
not an asserted generic kernel ABI. Producer acquires a coherent hardware
snapshot and publishes an immutable inode via atomic rename. Final components
are not followed through symlinks; directory is fd-pinned. Root/effective-UID
ownership and no group/other write bits are enforced. Trusted ancestry, ACLs,
local filesystem and producer semantics remain deployment preconditions.

Malformed, zero capacity, over-capacity usage, stale/future time, nonpositive or
nonincreasing generation/time, oversize input/poison batches, missing/disabled
provenance, nonregular files, permission violations and unavailable I/O fail
closed. Accepted watermarks survive failed reads. A new valid report can recover;
old accepted generations cannot. File reads are byte bounded, not hard latency
bounded on hostile filesystems.

## Exact verification and observed failures

Behavioral RED was observed before implementation for:

1. `preserves_distinct_physical_snapshots`: callable parser scaffold returned
   InvalidData instead of the first exact expected report (exit 101).
2. `rejects_invalid_bounds_and_freshness`: parser accepted generation zero
   (exit 101); validation then implemented.
3. `actual_file_reads_preserve_snapshots_and_reject_replays`: callable reader
   scaffold rejected the initial actual-file report (exit 101).
4. Three actual-reader tests failed against the basic reader: marker `0` was
   accepted, symlink report was followed, and replacing the directory changed
   the selected report (111 versus expected pinned 321). Implemented descriptor
   pinning, bounded child reads, no-follow and positive provenance.
5. `rejects_untrusted_write_permissions`: world-writable report was accepted
   (exit 101); owner/mode checks then implemented.

Additional differential negative control temporarily removed the **real public
parser's** byte-limit guard in this worktree only, ran
`cargo test --locked -p libcram-core --test device_adapter_parser rejects_invalid_bounds_and_freshness`,
and observed exit 101 at `one byte over budget`. The test first accepts the same
valid report padded to exactly 16384 bytes, then rejects 16385. Guard was restored
in a finally block before final verification; no mutation remains.

Final commands (native macOS owned-filesystem evidence):

```sh
rustfmt --edition 2021 crates/libcram-core/src/device_adapter.rs \
  crates/libcram-core/tests/device_adapter_parser.rs \
  crates/libcram-core/tests/device_adapter_reader.rs
cargo test --locked -p libcram-core \
  --test device_adapter_parser --test device_adapter_reader
cargo clippy --locked -p libcram-core --all-targets -- -D warnings
git diff --check
```

Results: **2 parser + 7 reader tests passed**, clippy all-targets passed with
warnings denied, diff check passed. No broad repeated test suite was run.
Reader targets cover distinct snapshots, replay/regression/error recovery,
provenance malformed/missing/restored, symlinks, permission changes, directory
pinning, oversized/stale/future/missing reports, FIFOs/directories, excessive
poison batches, and concurrent atomic publications with exact field pairing.
Tests use owned temporary directories and clean up through Drop. No user data.

## Changed files and remaining limits

* `crates/libcram-core/src/device_adapter.rs`
* `crates/libcram-core/src/lib.rs` (module export only)
* `crates/libcram-core/tests/device_adapter_parser.rs`
* `crates/libcram-core/tests/device_adapter_reader.rs`
* `docs/DEVICE_ADAPTER.md`
* `docs/device-handoff.md`

Software is ready for parent integration. **D hardware acceptance remains
blocked**: supplied context reports no CXL or node CRAM on s5. This worker did
not probe s5 or claim Linux-driver/hardware execution. Native Unix file API tests
are not proof of a vendor ABI, physical occupancy accuracy, hardware poison,
Linux deployment or cancellation of blocked kernel I/O. A conforming real driver
producer, trusted deployment configuration and hardware acceptance are still
needed. No generic sysfs ABI or fabricated hardware results were introduced.

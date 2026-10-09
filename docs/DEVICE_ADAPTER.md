# Explicit physical CRAM driver bridge

`libcram_core::device_adapter` is an opt-in, read-only adapter protocol, **not a
standard Linux kernel CRAM ABI**. There is no automatic discovery, fallback to
process allocation counters, or inference that ordinary CXL/NUMA memory is CRAM.
A deployment must supply a trusted driver-specific producer that actually reads
physical device occupancy and atomically publishes this schema. This repository
does not supply that producer or establish hardware availability.

## Public API

```rust
pub struct DeviceReading {
    pub used_bytes: u64,
    pub capacity_bytes: u64,
    pub poison_pages: Vec<u64>,
}
pub trait DeviceAdapter: Send {
    fn read(&mut self) -> std::io::Result<DeviceReading>;
}
// Available on Unix (Linux deployment; macOS supports owned filesystem tests):
// DirectoryDeviceAdapter::new(node_directory: impl AsRef<Path>) -> io::Result<Self>
// Implements DeviceAdapter.
```

The constructor requires an absolute node-directory path, opens its `cram`
subdirectory, and pins that directory by file descriptor. It does not claim the
device is healthy: only a successful `read()` establishes an observation.
The public pure parser is
`parse_report(bytes: &[u8], now_unix_ms: u64) -> io::Result<DeviceReport>`.
`DeviceReport` exposes `generation: u64`, `observed_unix_ms: u64`, and
`reading: DeviceReading`. Parsing alone neither authenticates provenance nor
checks a previous observation; use the stateful adapter for runtime ingestion.

## Files and exact schema

Within the configured node directory:

* `cram/compression_enabled`: exactly `1` or `1\n` (at most 16 bytes read plus
  one overflow byte). This positive provenance matches the existing topology
  convention; absent, disabled or malformed markers fail closed. A bridge mirror
  must derive this marker from the actual driver, not fabricate classification.
* `cram/libcram-report-v1`: one immutable, atomically published regular file:

```text
CRAM-PHYSICAL-V1 GENERATION OBSERVED_UNIX_MS PHYSICAL_USED_BYTES PHYSICAL_CAPACITY_BYTES POISON_COUNT PAGE_ID...
```

Fields are ordered ASCII-whitespace-separated tokens. Unsigned decimal integers
must contain digits only, fit `u64`, and have no signs or fractions. Leading zeros
are allowed. Exactly `POISON_COUNT` page IDs must follow, with no extra tokens.
Example **synthetic schema illustration**, not measured hardware data:

```text
CRAM-PHYSICAL-V1 17 1791500000000 123456 1048576 2 42 97
```

* Occupancy is the device's **physical compressed backing bytes consumed** and
  physical available capacity in the same domain, not process RSS, virtual bytes,
  allocator live bytes, logical uncompressed capacity, or a compression estimate.
* Capacity must be nonzero; `0 <= used <= capacity`.
* Generation and observation timestamp must be positive. A producer must acquire
  a new physical observation before advancing either; republishing cached counters
  with a new timestamp/generation is not compliant.
* Timestamp is same-host Unix epoch milliseconds at physical acquisition, not
  publication time. It cannot be in the future or older than `MAX_AGE_MS = 1000`.
* Both generation and timestamp must strictly increase against the last accepted
  report. Equal generations, equal timestamps, regressions, and replay are errors.
  Generation exhaustion/restart requires explicit operator reconciliation, not a
  silently reset reader. Do not recreate a reader for every poll.
* `MAX_REPORT_BYTES = 16384`; `MAX_POISON_RECORDS = 256`. These bounds are fixed,
  public constants. Overflow fails the entire observation, never truncates it.
* Page IDs are physical page identifiers in the exact domain expected by the
  allocator's poison reporting API. The producer must establish that mapping.
  Duplicates remain in the bounded vector: the allocator owns its unique poison
  latch semantics. This adapter does not clear poison or count duplicate IDs as
  distinct events. No unbounded poison history is retained.

## Coherence and trust contract

The trusted producer must acquire used/capacity/poison together from a coherent
driver observation, write a new inode completely, then publish it by atomic rename
on the **same local filesystem**. Published inodes must never be modified in
place, including through hard links or retained writable descriptors. A reader
opens the report once, reads that inode with a `limit + 1` byte cap, and parses it
as a single snapshot. Thus rename cannot mix old used with new capacity. Reading
separate uncontrolled sysfs occupancy files and assembling them without a driver
snapshot protocol is not a conforming producer. No generic sysfs file is assumed
to implement this protocol. A future direct driver backend needs its own verified
atomic-snapshot/generation contract.

`compression_enabled` is checked before and after reading. A trusted producer
must coordinate disabling/re-enabling and fresh report publication; two marker
reads are not protection against a malicious producer's ABA changes.

The selected directory and opened files must be owned by root or effective UID,
with neither group nor other write mode bits. Directory trust is rechecked each
read. The final `cram` component and child files cannot be symlinks. Child opens
are descriptor-relative with `O_NOFOLLOW`, `O_CLOEXEC`, and `O_NONBLOCK`; nonregular
files (including FIFOs) are rejected. All ancestor directories, mount namespace,
ACLs and producer credentials must separately be trusted by the deployment.
Mode-bit checks are hardening, not cryptographic driver authentication, ACL
validation, or protection from another process running as the same UID/root.
An explicitly configured local file is not itself proof of CRAM hardware.

The adapter never writes the selected tree, sysfs, or live configuration. It has
bounded input bytes and poison allocation, no retry loop, and no background
thread. Regular-file I/O is synchronous: byte bounds and FIFO rejection are **not
an OS-level latency deadline**. Do not configure hostile FUSE/network mounts or
claim this reader can cancel a hung filesystem operation.

## Freshness and runtime integration

Freshness is checked after the read, using the larger of current wall time and
constructor wall time plus monotonic elapsed time. Backward clock changes cannot
keep extending an old observation. Clock corrections may conservatively reject
otherwise useful reports. Invalid reports do not erase or advance the accepted
watermark. A subsequent genuinely newer valid report can recover; previously
accepted data cannot recover by replay.

The managed runtime should keep one `DeviceAdapter`, submit both physical byte
fields only on successful `read`, then report the bounded poison IDs. On **any**
error it must immediately invalidate device health, without refreshing its TTL.
The adapter never returns a cached success: even an unchanged report within its
one-second age window is a replay error. Polling faster than producer publication
therefore intentionally produces unavailable observations. Choose cadence and
producer update rate accordingly. The allocator's own downstream TTL is separate;
this frozen API does not transmit the original observation age, so a downstream
one-second TTL can outlive the source's one-second admission window. Parent-owned
runtime integration must not describe these as one shared end-to-end deadline.

I/O errors preserve their OS error kinds. Invalid schema, stale/future observation,
replay, wrong marker, and nonregular files produce `InvalidData`; unsafe owner or
write permissions produce `PermissionDenied`; relative configuration produces
`InvalidInput`. Missing files are errors, not empty or zero-usage reports.

## Evidence boundary

Owned temporary-directory tests exercise actual open/read/rename APIs, provenance
revocation, replay/regression/recovery, differing physical snapshots, symlink and
FIFO rejection, permission changes, bounded reports, and concurrent atomic
publication. Public parser tests preserve distinct complete results and check
malformed/future/stale/overflow inputs plus exact byte/poison limits. These tests
are not hardware-driver interoperability evidence. No live sysfs data is written.
The reported s5 environment has no CXL or `node*/cram` capability; hardware
acceptance remains blocked pending a real device and a conforming producer.

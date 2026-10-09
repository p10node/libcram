# libcram

Rust SDK for Linux NUMA memory-tier orchestration, with a C ABI and C++17, Python
and Go bindings. Verified on Linux x86_64 with real two-node NUMA placement and
page migration in a dedicated KVM/Incus VM. macOS is also supported for DRAM-only
development and binding tests.

This is a userspace SDK, not a compressed-memory kernel driver. It requires an
actual accessible CRAM NUMA node to place memory on compressed RAM. Ordinary CXL
memory is not automatically treated as CRAM. See docs/KERNEL_INTEGRATION.md.

## Acceptance status

Validation includes real Linux NUMA execution, not only macOS development or
mock-topology/container fallback tests:

- **Linux x86_64 NUMA VM:** core/FFI tests and 64-thread stress pass on Ubuntu
  24.04 with two NUMA nodes. Real `mbind` placement and `move_pages` migration
  of 16 owned pages from node 0 -> 1 -> 0 pass, with independent placement queries
  and byte-for-byte payload verification. This testing found and fixed an
  `mbind` nodemask boundary bug.
- **Kernel BPF and PMU:** guest verifier/basic counters pass; the guest still
  rejects precise sampling (`max_precise=0`). On the physical host, an explicitly
  audited L1-hot load/store profile passes address attribution, owned-thread
  arrival/exit and descriptor cleanup checks. All-load attribution and total
  sampling+collector overhead below 1% of one CPU core remain open.
- **Managed Rust runtime:** bounded event/device intake, weak allocation-generation
  tokens, rolling access counts, worker shutdown and pressure/source-failure
  evacuation are implemented with deterministic integration tests. The optional
  PEBS source manages own-process thread lifetimes on explicitly selected CPUs.
- **Trusted device adapter:** bounded atomic-report parsing, provenance, replay,
  symlink and freshness checks are tested using owned fixtures, not CRAM hardware.
- **Bindings and portability:** software correctness and binding gates pass on
  macOS and Linux, including real CPU PyTorch alias/lifetime tests.

The tested NUMA nodes are DRAM, not CRAM. Physical CRAM placement, compression
and device-occupancy integration still require compatible hardware and a driver.
The unchanged 4 KiB entropy budget of 1000 ns still fails for pseudorandom input
on both the Apple M1 Pro and the Linux x86_64 VM. Exact runtime-verified periodic
counting brings the unchanged ASCII benchmark below budget on both. VM timings
are not isolated bare-metal performance results. This is not a production-ready
or all-requirements-complete claim.

See [current A–E feature matrix and open acceptance gates](docs/IMPLEMENTATION_STATUS.md),
[Linux NUMA VM acceptance](docs/S5_VM_ACCEPTANCE.md),
[native PEBS scope/evidence](docs/PEBS_HOST_ACCEPTANCE.md),
[latest strict entropy measurements](docs/performance/entropy-periodic/README.md), and
[verification history](docs/VERIFICATION.md). These separate software correctness,
ordinary DRAM NUMA movement and actual hardware/performance acceptance.

## Managed integration and SDK archives

The new opt-in **Rust** path connects `ProcessPebsCollector`,
`DirectoryDeviceAdapter`, `runtime_bridge::Bridge` and `Runtime`.
[Wiring, bounds and lifecycle](docs/runtime-bridge.md) documents synchronized
entropy metadata, original event timestamps and fail-closed operation.
[Device report contract](docs/DEVICE_ADAPTER.md) is an explicit driver/adapter
convention, not a universal kernel ABI. Existing C/C++/Python/Go allocation APIs
do not implicitly enable this new managed pipeline.

`cram_unregister_pressure_callback_and_wait()` adds a quiescent C callback
barrier; release userdata only after `CRAM_OK`. See [binding contracts](docs/BINDINGS.md).

`just sdk-package` creates a clean-source native C/C++ engineering archive;
`just sdk-verify path/to/archive.tar.gz` verifies its manifest and runs consumers
against the actual extracted library. See [packaging](docs/RELEASING.md).
Packaging success is not production acceptance.

## Build

Prerequisites: Rust 1.89 or newer, a C/C++ compiler, and `just`. Python and Go are
needed only for their bindings; Docker is optional for Linux validation on macOS.

```sh
just build
just check
just header-tool
just header-check
just bench
just linux
```

Build artifacts are under `target/release`. The C header is `include/libcram.h`;
link against `libcram` (`libcram.so` on Linux, `libcram.dylib` on macOS).
`just --list` lists all recipes. Cargo dependencies are locked in Cargo.lock.

## C allocation

```c
#include "libcram.h"
#include <string.h>

int main(void) {
    if (cram_init() != CRAM_OK) return 1;
    void *buffer = cram_alloc(4096, 64, CRAM_AUTO);
    if (!buffer) return 2;
    memset(buffer, 0, 4096);
    return cram_free(buffer) == CRAM_OK ? 0 : 3;
}
```

Memory must be freed through the same SDK, not libc `free`. The caller owns
synchronization and must stop all accesses before freeing. Read the generated
header for callback lifetime and telemetry contracts.

## Design

- `crates/libcram-core/src/hal`: read-only NUMA/CXL discovery and Linux memory-policy
  syscalls, with explicit fallback/error handling.
- `entropy`: exact byte Shannon entropy (0..8 bits), runtime SIMD dispatch and a
  scalar reference; constant buffers and arbitrary tails are covered.
- `allocator`: aligned owned/raw allocations, tier policy, thread-safe ownership
  tracking and DRAM fallback.
- `migration`: bounded page batches and individual syscall outcomes; application
  observations avoid unsynchronized background reads of mutable payloads.
- `guard`: pressure watermarks and safety mechanisms. Logical allocated bytes
  are not a substitute for hardware compressed-capacity accounting.
- `telemetry`: memory PSI and bounded page-access samples; optional Linux Aya/PMU
  support is enabled with the core `ebpf` feature.
- `crates/libcram-ebpf`: separately built kernel BPF program. Building an ELF does
  not imply the running kernel/PMU can load or use it.
- `crates/libcram-ffi`, `bindings/`: language ownership and ABI adapters.
- `benches/`: Criterion entropy and allocation/free round-trip comparisons,
  including the C ABI versus malloc/free.

## Verification and deployment

[Linux NUMA VM acceptance](docs/S5_VM_ACCEPTANCE.md) records real placement,
migration, kernel verifier and PMU results. [Verification history](docs/VERIFICATION.md)
records earlier cross-platform gates and their limits. Real DRAM NUMA migration
has been tested; neither it nor passing mock-topology/container fallback tests
establishes physical CRAM support.
The requested <1 microsecond per 4KB entropy and <1% sampling CPU targets are
hardware/workload acceptance gates, not unconditional API guarantees.

Read docs/SAFETY.md before using raw pointers, pressure callbacks or migration.
Use docs/HARDWARE_ACCEPTANCE.md to qualify a real deployment. In particular,
this SDK cannot promise prevention of OOM, every kernel stall, or hardware poison
without supported device telemetry and deployment-specific testing.

## License

MIT OR Apache-2.0; see LICENSE-MIT and LICENSE-APACHE.

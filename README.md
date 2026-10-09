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
- **Kernel BPF and PMU:** both `sample_load` and `sample_store` pass the Linux
  kernel verifier; basic PMU cycle counting works. Precise sampling is refused
  by the guest PMU (`max_precise=0`), so PEBS attribution and active sampling
  overhead below 1% CPU remain unverified.
- **Bindings and portability:** software correctness and binding gates pass on
  macOS and Linux, including real CPU PyTorch alias/lifetime tests.

The tested NUMA nodes are DRAM, not CRAM. Physical CRAM placement, compression
and device-occupancy integration still require compatible hardware and a driver.
The unchanged 4 KiB entropy budget of 1000 ns still fails for ASCII and
pseudorandom input on both the Apple M1 Pro and the Linux x86_64 VM. VM timings
are not isolated bare-metal performance results. This is not a production-ready
or all-requirements-complete claim.

See [Linux NUMA VM acceptance](docs/S5_VM_ACCEPTANCE.md) for the latest real
Linux results and archived evidence, and [verification history](docs/VERIFICATION.md)
for earlier cross-platform gates and their scope.

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

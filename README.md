# libcram

Rust SDK for Linux NUMA memory-tier orchestration, with a C ABI and C++17, Python
and Go bindings. macOS is supported for DRAM-only development and binding tests.

This is a userspace SDK, not a compressed-memory kernel driver. It requires an
actual accessible CRAM NUMA node to place memory on compressed RAM. Ordinary CXL
memory is not automatically treated as CRAM. See docs/KERNEL_INTEGRATION.md.

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

`docs/VERIFICATION.md` records executed gates and their limits. Do not equate
passing mock-topology or container fallback tests with physical CRAM support.
The requested <1 microsecond per 4KB entropy and <1% sampling CPU targets are
hardware/workload acceptance gates, not unconditional API guarantees.

Read docs/SAFETY.md before using raw pointers, pressure callbacks or migration.
Use docs/HARDWARE_ACCEPTANCE.md to qualify a real deployment. In particular,
this SDK cannot promise prevention of OOM, every kernel stall, or hardware poison
without supported device telemetry and deployment-specific testing.

## License

MIT OR Apache-2.0; see LICENSE-MIT and LICENSE-APACHE.

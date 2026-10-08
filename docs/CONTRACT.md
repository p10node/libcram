# Implementation contract

All code comments English. Rust crates deny clippy::unwrap_used. No performance or hardware acceptance claims without measurement. Linux is production; macOS supports DRAM-only development fallback.

## Core public API (allocation lane owns crates/libcram-core except entropy and telemetry)
`pub mod hal; pub mod allocator; pub mod migration; pub mod guard; pub mod entropy; pub mod telemetry;`
`allocator::AccessHint` repr(u32): Auto=0, ReadHeavy=1, WriteHeavy=2.
`allocator::CramAllocator::new() -> Result<Self, crate::Error>` discovers topology and starts bounded monitoring if supported.
`CramAllocator::allocate(&self, size: usize, align: usize, hint: AccessHint) -> Result<std::ptr::NonNull<u8>, crate::Error>`
`unsafe CramAllocator::deallocate(&self, ptr: NonNull<u8>) -> Result<(), crate::Error>` validates owned pointer via allocation registry. Caller must stop accesses before freeing.
`CramAllocator::telemetry(&self) -> allocator::TelemetrySnapshot` Copy with public fields `allocated_bytes: u64, cram_bytes: u64, allocation_count: u64, pressure_level: u32, psi_some_avg10: f64, psi_full_avg10: f64`.
`CramAllocator::register_pressure_callback(&self, callback: Option<std::sync::Arc<dyn Fn(u32) + Send + Sync>>) -> Result<(), crate::Error>` callbacks async, no internal locks held during invocation. Document callback replacement/lifetime semantics.
Core Error Display + std::error::Error; variants allocation lane decides. FFI map errors conservatively (do not label all errors OOM).

## Profiling lane owns only core src/entropy, src/telemetry and crates/libcram-ebpf
`entropy::shannon_entropy(data: &[u8]) -> f64`; `entropy::scalar_entropy(data: &[u8]) -> f64`; empty returns 0. Entropy for general slice; optimized for 64..4096.
`telemetry::PsiSample` Default Copy: `some: PsiLine, full: PsiLine`; `PsiLine` Default Copy: `avg10:f64, avg60:f64, avg300:f64, total:u64`.
`telemetry::read_memory_pressure() -> std::io::Result<PsiSample>`
`telemetry::parse_memory_pressure(text: &str) -> std::io::Result<PsiSample>`
Aya dependency must be target Linux and optional core feature `ebpf`. Profiling lane may communicate required core manifest additions via its handoff but does NOT change core manifest or lib.rs. Standalone eBPF workspace excluded from root. Never infer read/write access from unrelated tracepoints. Exact capability/error reporting for unavailable PEBS.

## ABI lane owns crates/libcram-ffi, bindings, cbindgen.toml, include, tests/bindings
C opaque global singleton allocator behind OnceLock; cram_init returns int32 status. cram_alloc(size_t size, size_t alignment, uint32_t hint) returns void* null on error. cram_free(void*) returns int32; null free success. cram_register_pressure_callback(callback(uint32_t level, void* user), void* user) returns int32. cram_get_telemetry(cram_telemetry* output) returns int32. Add cram_last_error() integer thread-local if useful. No panic crosses ABI, validate input tags and alignment before allocation. Callback userdata caller-owned and valid through async delivery; expose unregister quiescence or explicitly document stronger lifetime requirement. Do not claim unregister is quiescent without implementing it. C++ aligned pmr and correct identity; Python owned buffer protocol pool prevents use-after-free; Go owned wrapper and explicit Close, no Go pointers retained by C.

## Integration owner
Root manifests, Justfile, README, docs, CI, criterion benches, composed gates and Linux execution. Worker local manifests may temporarily narrow workspace membership to test; never commit root manifest alterations. All worker code via vertical test-first RED/GREEN slices; preserve command evidence in handoff. No fabricated syscall/performance evidence.

# Native bindings

The C ABI is generated from `crates/libcram-ffi` with **cbindgen 0.29.2**.
The Rust core contract is in `docs/CONTRACT.md`. Build shared and static libraries
with `cargo build -p libcram-ffi` (add `--release` for production).

## Ownership and errors

* `cram_init` lazily initializes a singleton. Normal initialization failures are
  sticky. `cram_alloc` also initializes it, rejects zero size, bad alignment,
  oversized layouts and unknown hints, and returns null on error.
* All statuses are integer constants in `include/libcram.h`; generic core errors
  are **not** classified as OOM. `cram_last_error` is thread-local and is reset by
  successful operations. Reading it does not clear it. Unwinding Rust panics are
  caught (including hostile panic-payload destructors). Process aborts, invalid
  pointers, C++ exceptions through C, and panic=abort builds are not recoverable.
* Caller owns every returned pointer until `cram_free` succeeds. No accesses may
  race with free. Null free succeeds. Foreign addresses are rejected by the core
  registry; this is **not** protection against stale pointers/address reuse.
* Telemetry output must be valid aligned writable storage. See the generated
  header for Rust safety contracts, which apply equally to C consumers.
* Pressure callbacks run asynchronously, must be thread-safe and must not unwind,
  throw or longjmp. Registration replaces the previous callback. **Replacement and
  NULL unregister are not quiescent**: queued delivery may still execute.
  Callback code and userdata must remain valid until
  `cram_unregister_pressure_callback_and_wait()` returns `CRAM_OK`, or until
  process exit. Do not unload the callback library early. No Go pointer may be
  userdata.

## C++17

`#include <libcram.hpp>` provides `cram::memory_resource`, a noncopyable PMR
resource. It supports over-aligned objects, maps zero-byte PMR requests to one
byte, throws `std::bad_alloc` on allocation failure, and uses object identity for
resource equality. The resource must outlive its containers. Failed deallocation
terminates rather than throwing from a container destructor.

## Python

Build/install with a virtualenv and maturin:

```sh
python3 -m venv .venv
. .venv/bin/activate
python -m pip install 'maturin>=1.9,<2' pytest
maturin develop --manifest-path bindings/python/Cargo.toml
python -m pytest tests/bindings/test_python.py -q
```

PyO3 is pinned to 0.29.3, supporting Python 3.14. This module requires the GIL,
including on free-threaded Python (it does not claim GIL-free execution).

```python
import libcram
with libcram.MemoryPool() as pool:
    with pool.allocate(4096, alignment=64, hint=0) as buf:
        with memoryview(buf) as view:
            view[0] = 42
```

Buffers are writable, contiguous unsigned bytes and are zero-initialized before
exposure. An export retains its buffer; slices also keep the export alive.
`Buffer.close()` raises `BufferError` while exports exist, then frees idempotently.
A closed buffer cannot be exported. `MemoryPool.close()` stops new allocations,
**not** existing buffers: it is an allocation factory, not a bulk-free arena.
Dropping the pool never invalidates a buffer. Drop/GC frees unexported buffers;
context managers are preferred for deterministic release.

## Go

The module in `bindings/go` uses cgo and an installed/shared `libcram` (or the
local target directory through `CGO_LDFLAGS`). It exposes `NewBuffer`, `ReadAt`,
`WriteAt`, `Close`, `Init`, and `Telemetry`. No native slice or pointer escapes.
Read/write copy while holding the ownership lock, so they cannot race with Close.
Copies of the Buffer wrapper share one close state. Zero-value Buffer is closed.
Call Close explicitly; a finalizer is only a fallback. The allocation error is
captured in the same C call, avoiding goroutine/OS-thread TLS migration bugs.
Only pointer-free temporary scalar/telemetry outputs cross cgo; C retains none.
There is intentionally no Go callback registration API.

```sh
export CGO_LDFLAGS="-L$PWD/target/debug -Wl,-rpath,$PWD/target/debug"
(cd bindings/go && GOEXPERIMENT=cgocheck2 go test -race -count=1 ./...)
```

## Reproducible checks

```sh
cargo install cbindgen --version 0.29.2 --locked
cbindgen --config cbindgen.toml --crate libcram-ffi --output include/libcram.h
bash tests/bindings/check-header.sh
cargo test -p libcram-ffi
cargo clippy -p libcram-ffi -p libcram-python --all-targets --no-deps -- -D warnings
bash tests/bindings/native-smoke.sh
```

The native smoke script compiles/runs C11 pthread and C++17 PMR consumers, then
runs Go race/cgocheck2 tests. It requires a C++17 standard library with PMR.
Python tests require a freshly rebuilt/installed extension, not a stale wheel.
Callback bridge unit tests inject delivery on a native Rust thread; they do not
prove that production memory-pressure monitoring emits callbacks. That requires
composed core/monitor tests. No hardware, migration or performance claim is made.

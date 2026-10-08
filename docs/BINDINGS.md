# Language binding ownership

## Python

Build and exercise the extension with `just python-tools python-test`.
The extension implements the Python buffer protocol, so standard CPU-buffer
consumers can retain an exported memoryview without copying.

```python
import libcram

with libcram.MemoryPool() as pool:
    buffer = pool.allocate(4096, alignment=64)
    view = memoryview(buffer)
    view[:4] = b"CRAM"
    assert bytes(view[:4]) == b"CRAM"
    view.release()
    buffer.close()
```

A pool is an allocation factory, not a bulk-free arena. Closing the pool prevents
new allocations but does not invalidate existing buffers. Closing a buffer while
it has exported views is rejected. Derived views can keep the export alive even
after the original view is released.

For PyTorch/vLLM CPU offload integration, `torch.frombuffer(buffer, dtype=...)`
is a possible consumer of this protocol. The consumer must retain the owning
buffer/export for its full lifetime. This does not create a CUDA tensor, pin host
memory, install a vLLM cache backend, or make arbitrary vLLM versions automatically
compatible. A production GPU KV-cache integration needs a separately validated
allocator/offload adapter. No GPU/vLLM integration is claimed by buffer-protocol
tests alone.

## C and callbacks

Initialize once with `cram_init`, allocate with `cram_alloc`, release with
`cram_free`, and query a caller-owned `cram_telemetry` using `cram_get_telemetry`.
`cram_last_error` is thread-local. Raw-pointer output and callback arguments must
obey the safety conditions in the generated header.

Do not assume callback replacement cancels a callback already queued or running.
The current C contract requires callback code and userdata to remain valid until
process exit. Use static or otherwise process-lifetime storage; do not pass stack
addresses or Go-managed pointers. The callback must not unwind, throw, or longjmp.
Callbacks should enqueue short application notifications and return promptly.

## C++

The PMR resource uses the same process-wide native allocator. The resource must
outlive every container using it. Do not destroy a resource while allocations or
concurrent operations still refer to it. Allocation hints affect placement policy,
not type layout or ownership.

## Go

The Cgo buffer owns C memory, not Go memory. Use explicit `Close`; a finalizer is
only a leak backstop. The wrapper's copy-based accessors avoid exposing an
unbounded unsafe Go slice whose lifetime could outlive `Close`. Normal Go calls
must not retain pointers into Go-managed memory on the C side.

Run `just go-test` to include the Go race detector and the actual C library.
The race detector covers Go synchronization, not arbitrary data races inside C
or Rust. Native ownership/migration tests remain necessary.

# Safety and deployment boundaries

libcram is a userspace placement SDK. It does not create compressed RAM, provide a
kernel compression implementation, or turn an ordinary CXL NUMA node into a CRAM
node. A real deployment needs a kernel/device integration exposing compressed
memory as NUMA memory and a documented hardware-specific compression capability.
CXL alone is never sufficient evidence of compression support.

## Allocation ownership

Allocated pointers belong to the allocator that produced them. Do not pass them
to `free`, `delete`, another allocator, or a foreign runtime's garbage collector.
Do not free memory while any thread, DMA operation, Python memoryview, or other
consumer still uses it. Alignment and integer overflow must be validated before
mapping memory. Migration can change placement but must not change the virtual
address or ownership of a block.

The allocation registry detects many invalid frees; it cannot make stale raw
pointers safe after address reuse (the ABA problem). C and C++ callers retain the
normal responsibility to avoid double-free and use-after-free.

## Entropy and access observations

A background thread must not read arbitrary caller-writable bytes to compute
entropy. Doing so races with application writes and is undefined behavior in
Rust even if the intention is approximate telemetry. Applications must supply
observations taken under their own synchronization, or an explicitly supported
instrumentation mechanism must obtain them safely. Read/write classification
must come from an appropriate PMU event, sampled write-protection of owned
pages (`telemetry::write_sampler::WriteSampler`), or explicit application
observation; generic page-fault counts and scheduler samples are not
substitutes.

PMU event encodings are CPU-specific. Precise memory samples require supported
hardware, a compatible kernel, suitable capabilities/perf policy, and actual
sample addresses. An unsupported sampler must report that it is unsupported,
not return fabricated counters. Do not disable system-wide security settings to
make a benchmark pass.

## Pressure and hardware errors

Allocator-owned byte counters are not global device occupancy. Other processes,
compression-ratio changes, kernel reservations, and hardware poison can exhaust
a device even when this process's requested bytes are small. Production watermarks
must incorporate a trustworthy device/node source and conservative handling of
missing or stale information. Fallback to ordinary memory is a best-effort policy,
not an OOM guarantee. An asynchronous monitor cannot guarantee avoidance of every
kernel stall or poison event.

Pressure callbacks run on a background thread; they must return promptly, must
not unwind across the C ABI, and must obey the documented userdata lifetime.
Callbacks are notifications, not a real-time safety interlock.

## Operational validation

Before production use, validate the actual CRAM node identity and capacity,
measure successful and failed migration statuses, verify poison reporting,
exercise pressure transitions, and test every ABI in the deployed toolchain.
CPU overhead and tail latency must be measured under the intended concurrency,
page sizes, data distribution, NUMA distances, and PMU sample period.

Tests in an unprivileged container establish parsing, policy, ownership and
fallback behavior. They do not certify physical CRAM compression, PEBS accuracy,
remote-NUMA placement, or performance on deployment hardware.

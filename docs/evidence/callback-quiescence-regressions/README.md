# Callback quiescence regression closure

This follow-up adds three Rust unit regressions to the existing callback contract;
it does not change production behavior or renew hardware/performance acceptance.
The new tests run in `allocator::pressure::tests`, using the real callback worker
and a DRAM-only fixture allocator without starting a monitoring thread.

- Hold callback A in flight, replace it with B, observe the actual `quiescing`
  transition, then require the barrier to remain pending. Both Some/None
  registration and a competing barrier must return precisely WouldBlock. Release
  A, require completion and prove B was never invoked.
- Queue an A-generation event while A is blocked, replace with B, and send a
  distinct B-generation event through the owned FIFO. The positive B delivery is
  the drain witness; B must not receive the old event. This is intentionally at
  the queue-owning unit layer, not an unreliable best-effort public send loop.
- Remove the registration from its own callback, then exercise captured-state
  destructor reentry, rejected self-wait and a deliberate destructor panic.
  A fresh distinct callback delivery proves the worker survived.

The state wait uses yield plus a two-second watchdog, not a sleep to infer thread
ordering. Callback gates use channels. The intentional panic remains visible in
raw logs; success means containment and subsequent delivery, not absence of panic.

## Negative controls

`scripts/verify_callback_mutations.py` exports HEAD to an owned temporary tree,
overlays the current pressure source/tests, and mutates only that copy. It builds
before running the test binary directly under an eight-second watchdog, so build
errors cannot masquerade as behavioral failures and timeout kills the owned test
process rather than leaving a Cargo-spawned test behind. The script rejects
Python -O, verifies canonical input hashes, and removes the disposable tree.

| Variant | Observed result |
| --- | --- |
| Baseline | exit 0 |
| Skip in-flight wait | test failure, exit 101 |
| Ignore queued generation | test failure, exit 101 |
| Allow registration during wait | test failure, exit 101 |
| Allow competing wait | test failure, exit 101 |
| Stop worker after captured-state panic | watchdog timeout after deliberate panic |
| Restore original source | exit 0 |

These are mutation RED controls for existing correct behavior, not claims of a
new production bug repaired in this change. Source hashes and raw runs are in
`results.json`, `checksums.json` and adjacent logs.

## Canonical verification

- `cargo fmt --all --check`: exit 0.
- `cargo test --workspace --locked`: exit 0, 86 tests passed on macOS.
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`:
  exit 0.
- `git diff --check`: exit 0 for source changes; raw evidence logs retained as-is.

No new Linux, native C/C++ packaging, long soak, real CRAM or throughput gate was
run for this test-only addition. Existing API lifetime requirements still apply:
only a successful blocking unregister permits release, management must be
externally serialized, and callers must not hold locks needed by callbacks.
These tests do not promise containment of double-panic/abort or C unwinding.

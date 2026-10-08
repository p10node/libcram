# Binding lane execution evidence

These commands ran on macOS ARM with Python 3.14.5. **The core was an uncommitted
contract-only allocator stub**, using a registry and native Rust allocations.
It has been removed from the deliverable. Native consumers and PyO3 really ran;
these results certify the binding layer against that stub, **not composed core,
Linux production behavior, pressure monitoring, migration, or hardware**.

## RED / GREEN observations

1. `cargo test -p libcram-ffi`: failed with missing cram_init/cram_alloc/cram_free
   and CRAM_OK; after ABI allocation implementation: 1 integration test passed.
2. Telemetry/callback tracer: failed with missing cram_telemetry,
   cram_get_telemetry, cram_register_pressure_callback; after implementation:
   3 ABI integration tests passed (including 8-thread allocation/TLS checks).
3. C++17 compilation failed because libcram.hpp did not exist; after PMR
   implementation the consumer compiled and ran, including 256-byte alignment,
   zero-byte allocation and resource identity checks.
4. Python smoke initially failed importing missing libcram; built the actual
   extension using maturin/PyO3 0.29.3: owning buffer lifecycle tracer passed.
   Context-manager test then failed with missing __exit__; implementing context
   methods made all 8 tests pass. Final cross-thread export/Close coverage passed.
5. Go tests failed with missing NewBuffer/ReadHeavy/ErrClosed; after implementation
   native cgo tests passed with `-race` and `GOEXPERIMENT=cgocheck2`.
   Telemetry tracer then failed with missing Init/Telemetry; after implementation
   all 4 Go tests passed.
6. Panic-payload destructor regression **behaviorally failed**: catch_unwind's
   payload was dropped and panicked again. Forgetting only exceptional panic
   payloads fixed the test. Callback bridge tracer initially lacked the helper;
   after extraction it passed native-thread userdata delivery and allocator
   reentry. This is bridge injection, not production monitor delivery.

## Final executed gates

* `cargo test -p libcram-ffi`: **6 passed** (3 unit + 3 integration).
* `cargo clippy -p libcram-ffi -p libcram-python --all-targets --no-deps -- -D warnings`:
  passed.
* `bash tests/bindings/check-header.sh`: generated cbindgen 0.29.2 header matches.
* `bash tests/bindings/native-smoke.sh`: C11 pthread and C++17 PMR programs exit 0;
  Go `-race -count=1` with cgocheck2 passed. macOS linker reports duplicate rpath
  (cgo applies flags more than once); this is not a failed gate.
* Fresh `maturin develop --manifest-path bindings/python/Cargo.toml`, followed by
  `python -m pytest tests/bindings/test_python.py -q`: **9 passed**.

## Required after integration

Re-run every gate against real core and canonical workspace lockfile, rebuild
Python rather than reuse this stub-linked wheel, and run C/C++/Go on Linux.
Exercise actual pressure monitor emission, replacement and shutdown in core
acceptance. C callback unregister deliberately promises no quiescence; userdata
and callback code have process lifetime. No test claims otherwise.

The root Cargo.toml was never changed. Temporary core files and generated
Cargo.lock were removed before committing only lane-owned artifacts.

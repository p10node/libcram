# Verification and acceptance status

Date: 2026-10-09. Tested code: `74a15524530a8ab9c4a0fe42e49adab0c78acf1f`.
The later documentation/evidence commit does not change the tested implementation.

**Software correctness/integration gates pass. Overall performance and physical
hardware acceptance remain open. This is not a production-ready declaration.**

## Completed repairs

- The access window uses timestamp-ordered bounded storage and a monotonic expiry
  watermark. A query no longer rejects valid delayed samples from another ring.
  RED/GREEN regressions cover overlapping timestamps, queries between partial
  drains, six arrival permutations, separate pages/processes, capacity eviction,
  expired samples and backward queries. A read-only closure review found no
  blocking defect in the repaired telemetry/probe and Python tensor scope.
- The verifier example is part of the locked core workspace. Read/load/map/verifier
  failures propagate as nonzero exits; non-Linux execution returns 2 instead of
  printing an unsupported message and reporting success. Loading is not attaching.
- Rust 1.89 Linux/x86_64 builds now account for that compiler's unsafe CPUID
  signature, with a narrowly scoped compatibility allowance for newer Clippy.
- Physical-device admission and ownership fixes are integrated. The migration
  reversal regression now asserts actual positive promotion before demotion;
  previously its final zero-CRAM assertion could pass without any promotion.
- Optional CPU tensor storage retains an active memoryview export across aliases.
  Real PyTorch lifetime/value tests pass on macOS and Linux. This is not a CUDA
  allocator, pinned-memory implementation or a complete vLLM cache backend.
- The NEON kernel now scans uniform prefixes in cache-line batches and uses eight
  bounded histogram lanes. Differential tests cover every length through 4097,
  unaligned starts, changing uniform runs and larger batch boundaries.

## Executed gates

Host: Apple M1 Pro, macOS aarch64, rustc 1.99.0. Linux checks: `linux/amd64`
container image `libcram-sdk-test:local`, kernel 7.0.14-linuxkit. Container results
are functional evidence, not native x86 performance measurements.

| Gate | Result | Evidence |
|---|---|---|
| Workspace tests, including 64-thread allocation stress | PASS | `final-host.log` |
| Formatting, workspace/all-target Clippy with warnings denied | PASS | `final-host.log` |
| Generated C header comparison, C/C++17/Go race + cgo pointer checks | PASS | `final-host.log`, `final-linux.log` |
| Python buffer suite | PASS: 10 tests on each OS | same logs |
| Real CPU PyTorch tensor/alias suite | PASS: 2 tests on each OS | same logs, `final-linux-torch.log` |
| Separate BPF ELF build and Criterion executables | PASS | `final-host.log` |
| Linux core/FFI tests, `ebpf` feature tests + strict Clippy, release build | PASS | `final-linux.log` |
| Rust 1.89 host workspace/all-target check | PASS | `final-msrv-host.log` |
| Rust 1.89 Linux x86_64 core/all-target + `ebpf` check | PASS | `final-msrv-linux.log` |
| 4096-byte entropy latency budget | FAIL | `final-entropy-budget.json` |
| Actual BPF load attempt in available Linux container | BLOCKED: EPERM, exit 1 | `hardware-probe.log` |
| Real two-tier CRAM migration, PEBS attribution, active profiler CPU <1% | NOT ACCEPTED: hardware unavailable | hardware checklist remains open |

Logs and their SHA-256 manifest are in `docs/evidence/2026-10-09/summary.json`.
Repeated blank lines at EOF were removed from two logs for Git whitespace checks;
the manifest also retains their original hashes. No diagnostic or measurement
content was changed.
One Darwin linker warning about a duplicate rpath is retained in the host log;
it did not fail the binding tests. The passing MSRV cross-target check is a
compile check, not execution under that compiler on Linux.

## Performance: preserve the failure

Criterion: release build, normal repository compiler flags, warm-up 1 second,
measurement 2 seconds, 30 samples. The gate uses the **upper confidence bound of
the mean**, with an unchanged strict budget of 1000 ns. These are warm steady-state
measurements; lazy dispatch/table initialization is not a cold-call guarantee.

| 4096-byte input | Upper mean bound, ns | Status |
|---|---:|---|
| Zero | 153.365 | PASS |
| ASCII | 1159.222 | FAIL |
| Pseudorandom | 1110.068 | FAIL |

Several histogram lane layouts and native CPU tuning were explored; they did not
make all distributions meet the target. The SIMD improvement is integrated, but
**further performance work is still required**. Do not re-label this as complete
merely because the zero case is fast or the benchmark process exits successfully.

Raw Criterion estimates and benchmark identities are archived. This command
reproduces the recorded failure, with all three distributions present:

```sh
python3 scripts/entropy_budget.py docs/evidence/2026-10-09/criterion
# exit 1: ASCII and random exceed 1000 ns
```

The measurements were taken at `002458d50bdf19eb748f8ad60172c042d1fc2690`.
Git comparison confirmed that crates, bindings, benches, headers and Cargo
manifests/lockfile are unchanged in the final tested code commit; only the Linux
Torch gate was added afterward.

Allocation/free comparisons also ran successfully on this implementation;
`final-allocation.log` records all core/C ABI/malloc confidence intervals. These
short, sometimes noisy runs show nontrivial SDK overhead. They do not establish
compression savings, workload acceleration or a hardware latency SLA.

## Actual hardware limits

The available Linux container reported no CPU PMU, no CXL devices and no CRAM
adapter flags. Loading the real built BPF object failed while creating `EVENTS`:
`Operation not permitted`. No host security/perf settings were relaxed.

Consequently, verifier acceptance, PEBS load/store attribution, two-tier physical
migration and total kernel-plus-collector CPU overhead remain unverified. Run
`docs/HARDWARE_ACCEPTANCE.md` on an authorized Linux machine with the intended
CRAM driver, physical accounting source and compatible Intel PMU. Fixture topology,
BPF ELF generation, macOS fallback and CPU tensor tests cannot substitute for this.

## Reproduction

After installing the tools described by `just --list` and building the Linux image:

```sh
just check bindings torch-test header-check ebpf bench-build
LIBCRAM_LINUX_IMAGE=libcram-sdk-test:local just linux linux-torch
cargo +1.89.0 check --workspace --all-targets --locked
cargo +1.89.0 check --target x86_64-unknown-linux-gnu \
  -p libcram-core --all-targets --features ebpf --locked
cargo bench -p libcram-bench --bench entropy --locked -- \
  'entropy/dispatch/.*/4096' --warm-up-time 1 --measurement-time 2 --sample-size 30
python3 scripts/entropy_budget.py
```

Optional dependency setup: `just python-tools torch-tools`; local header/BPF tools:
`just header-tool ebpf-tools`. `just linux-torch` includes the native and ordinary
Python binding checks, installs CPU Torch in the dedicated container test venv,
and runs the real tensor tests. Linux source mounts remain read-only.

## Worktree closure and recovery

Only canonical `main` remains registered. The `task/core`, `task/profiling` and
`task/bindings` worktrees and branches were removed after:

1. Confirming each historical branch commit was patch-equivalent to an integrated
   commit on main, and independently checking the final entropy delta was imported.
2. Archiving every tracked, untracked and ignored entry, including build outputs
   and worker-only stub/harness files; no untracked file was silently discarded.
3. Creating and verifying a Git bundle for each task branch.
4. Comparing every archived payload against its source SHA-256 inventory, then
   checking the source had not changed during archiving.
5. Rechecking archive hashes after the original worktree paths were removed.

Archives are outside the repository, at
`../libcram-worktree-archive/20261009T033928Z/` relative to the repository root.
`report.json` records original branches, commits, status, paths and checksums;
per-lane manifests inventory all entries. The archives are recovery artifacts,
not active Git worktrees, and are not stored under a disposable Cargo target.

To recover a lane, clone its bundle with the explicit task branch into a separate
owned directory, then extract the matching tar there **excluding `.git`**. The
archived `.git` file points at the removed worktree metadata and must not be reused.

#!/usr/bin/env bash
# Run only in the disposable two-node Linux VM, not on the Incus host.
set -euo pipefail
[[ "${LIBCRAM_OWNED_VM_TEST:-}" == 1 ]] || { printf 'Set LIBCRAM_OWNED_VM_TEST=1 inside the owned test VM.\n' >&2; exit 2; }
cd "$(dirname "$0")/../.."
export CARGO_BUILD_JOBS=4
out="${LIBCRAM_VM_EVIDENCE:-/root/libcram-vm-evidence}"
mkdir -p "$out"
: > "$out/status.tsv"
run() {
    local name="$1"; shift
    local rc=0
    "$@" > "$out/$name.log" 2>&1 || rc=$?
    printf '%s\t%s\n' "$name" "$rc" | tee -a "$out/status.tsv"
    return 0
}
run inventory bash -c 'uname -a; lscpu; numactl --hardware; python3 -c '\''from pathlib import Path; print("max_precise:", Path("/sys/bus/event_source/devices/cpu/caps/max_precise").read_text()); print("CXL devices:", list(Path("/sys/bus/cxl/devices").glob("*"))); print("CRAM adapter flags:", list(Path("/sys/devices/system/node").glob("node*/cram*")))'\'''
# Refuse to overwrite an example already owned by another workflow.
probe=crates/libcram-core/examples/numa_vm_probe.rs
[[ ! -e "$probe" ]] || { printf 'Probe path already exists: %s\n' "$probe" >&2; exit 2; }
trap 'rm -f "$probe"' EXIT
cp docs/performance/numa_vm_probe.rs "$probe"
run core-ffi cargo test -p libcram-core -p libcram-ffi --features libcram-core/ebpf --locked
run numa-migration cargo run -p libcram-core --example numa_vm_probe --locked
run bpf-verifier cargo run -p libcram-core --features ebpf --example pebs_probe --locked -- /root/libcram/libcram-pebs.bpf
run pmu-counter perf stat -e cycles:u -- sleep 0.1
run pmu-precise perf record -o "$out/precise.data" -e cycles:upp -- sleep 0.1
run entropy-build cargo bench -p libcram-bench --bench entropy --locked --no-run
run entropy taskset -c 0 cargo bench -p libcram-bench --bench entropy --locked -- 'entropy/dispatch/.*/4096' --warm-up-time 1 --measurement-time 2 --sample-size 30
run entropy-budget python3 scripts/entropy_budget.py target/criterion
# This aggregate captures any refusal/failure, including unavailable PEBS.
python3 - "$out/status.tsv" <<'PY'
from pathlib import Path
import sys
rows = [line.split('\t') for line in Path(sys.argv[1]).read_text().splitlines()]
raise SystemExit(1 if any(int(code) != 0 for _, code in rows) else 0)
PY

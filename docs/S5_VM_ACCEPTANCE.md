# s5 Incus NUMA acceptance — 2026-10-09

## Outcome

A dedicated VM has been created and tested. Real Linux NUMA placement and
migration now pass after fixing an mbind argument bug. This is not complete
CRAM/PEBS/performance acceptance.

Implementation tested: `bad1b1a` (source hashes independently compared between
canonical checkout and VM). The initial snapshot was `0d96e6f`; only the NUMA
fix and owned probe/runner were subsequently transferred.

## Isolated environment

- SSH host alias: `s5`; Incus 6.0.6.
- VM: `libcram-numa-test`, Ubuntu 24.04, kernel 6.8.0-146-generic.
- Four vCPUs pinned to host CPUs `12-13,28-29`; guest exposes two NUMA nodes.
- 8 GiB memory; 30 GiB root disk on `disk1`; private NAT network `incusbr0`.
- No inherited profiles; `boot.autostart=false`. VM remains running for follow-up.
- Guest CPU: Xeon E5-2683 v4, AVX2, no guest PEBS flag; PMU `max_precise=0`.
- Rust 1.89.0; build concurrency limited to four. Benchmark execution pinned to
  guest CPU 0. This is native x86 KVM execution, not cross-ISA emulation, but
  the host is shared and these are not isolated bare-metal latency results.
- The 15 pre-existing instances retained their original running/stopped states.
  No existing profile, instance, host perf policy or host security setting was changed.

Access:

```sh
ssh -t s5 'incus exec libcram-numa-test -- bash'
# Inside the VM:
cd /root/libcram
source /root/.cargo/env
LIBCRAM_OWNED_VM_TEST=1 LIBCRAM_VM_EVIDENCE=/root/libcram-vm-next \
  bash docs/performance/run-numa-vm.sh
```

The runner returns nonzero when any gate fails, including unavailable precise
sampling and an over-budget entropy result. It removes its temporary Cargo
example afterward. Original/final guest logs are preserved under
`/root/libcram-vm-evidence`, `/root/libcram-vm-final`, and
`/root/libcram-vm-fixed`; the first two include provisioning/harness failures.

To stop this owned VM without deleting its evidence:

```sh
ssh s5 'incus stop libcram-numa-test'
```

## Actual results

| Gate | Result |
|---|---|
| Rust core + FFI tests, including 64-thread stress, with `ebpf` enabled | PASS |
| Real `SystemNuma.bind`, 16 owned pages | PASS after fix |
| Migration 0 -> 1 -> 0, independent `move_pages` placement queries | PASS |
| Byte-for-byte payload preservation after each migration | PASS |
| BPF `sample_load` and `sample_store` kernel verifier load | PASS |
| Basic user-space PMU cycle counting | PASS |
| Precise `cycles:upp` sampling | FAIL: guest PMU refuses sampling/overflow interrupts |
| CRAM driver/physical occupancy | Unavailable: no CRAM adapter flags or CXL devices exposed |
| Active PEBS read/write attribution and total CPU overhead <1% | Not established |
| Entropy 4096 B, unchanged 1000 ns upper-mean-confidence-bound gate | FAIL |

Entropy final upper bounds: zero **464.960 ns**, ASCII **4046.514 ns**, random
**3778.157 ns**. The distributions, budget and validator were not changed.
These are VM results; they neither supersede the separate M1 measurement nor
prove physical-host performance. Basic counters and verifier load do not prove
PEBS attachment, sample correctness or collector overhead.

## Bug found by the real probe

Before the fix, binding node zero returned `EINVAL`. An independent libnuma
call reproduced failure with maxnode=1 for node zero and maxnode=2 for node one;
a full-word nodemask limit succeeded. Linux 6.8 `get_nodes()` decrements
`maxnode` before copying the nodemask. The old `highest_node + 1` therefore
excluded the requested bit. The repair uses `highest_node + 2`, retaining
bounds within the allocated bitmap.

A pure regression was observed failing before the repair and covers nodes
0, 1, 63, 64, 65, 127, 128 and 65535. Host core/FFI tests and Clippy passed;
the repaired source then passed the VM core/FFI tests and actual round-trip
migration. The fixture maps only its own private pages, binds before first
touch and never accesses existing workload memory.

Kernel source: https://github.com/torvalds/linux/blob/v6.8/mm/mempolicy.c

## Provisioning details and retained failures

The profile-free guest needed DHCP configured for `enp5s0` inside the VM.
Initial agent unavailability was handled by inspecting boot/console state;
secure boot was not disabled. Registry resolution stalled despite a reachable
index endpoint, so dependencies were vendored from the local locked public
crate cache, transferred to the guest and configured as an offline Cargo source.
No credentials or entire Cargo home were copied. The interrupted initial fetch
is not counted as a semantic test failure. A private-module import error in the
first probe was corrected before observing the actual EINVAL regression.

## Evidence

`docs/evidence/2026-10-09/s5-vm/` contains final per-gate logs/status, the real
pre-fix migration failure, Criterion identity/estimate JSON and a SHA-256 manifest.
Log trailing whitespace is normalized for Git; original guest log hashes are
retained in the manifest, and the original files remain in the VM.
The archived validator reproduces exit 1:

```sh
python3 scripts/entropy_budget.py docs/evidence/2026-10-09/s5-vm/criterion
```

No device was labelled CRAM to manufacture a passing migration result. A
compatible CRAM driver/device and a PEBS-capable execution environment remain
necessary for the outstanding hardware acceptance criteria.

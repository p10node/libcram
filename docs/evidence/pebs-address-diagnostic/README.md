# Independent PEBS address diagnostic

## Result

The all-load address failure reproduces without libcram, Rust, the BPF program,
or its ring/decoder. A newly compiled owned C process repeatedly reads a fresh
16 KiB mapping. `perf record` samples only that process/descendants, pinned to
CPU 12 on s5. No system-wide capture, security change, attachment to another
workload, or live-data mutation was performed. Both fresh remote directories
were removed after collection. Existing remote SDK/probe artifacts were not
removed or overwritten.

Userspace-only (`u`) results at sample period 2,000,003:

| Event | Precise level | Samples | Address in owned mapping |
| --- | ---: | ---: | ---: |
| ALL_LOADS, 0x81d0 | 1 | 1759 | 0 |
| ALL_LOADS, 0x81d0 | 2 | 1735 | 0 |
| ALL_LOADS, 0x81d0 | 3 | 1739 | 0 |
| L1_HIT, 0x01d1 | 2 | 1733 | 1733 |

All four collection commands exited 0. That proves execution, **not all-load
acceptance**. The positive L1-hit control validates mapping bounds and the
address parser. This load-only probe does not renew the separate read/write
cross-contamination or managed collector lifecycle gates. It is not a CPU-cost
measurement and does not prove an all-load substitute.

The first diagnostic omitted the `u` modifier: all four cases, including L1-hit,
had zero owned addresses. Those results are preserved under `default/`, not
silently discarded. The subsequent `user-only/` run matches production's
userspace-only intent and restores the L1-hit positive control. Thus privilege
filters matter for this diagnostic; the default event configuration must not be
mistaken for the production sampling configuration.

## Reference cross-check

Intel's BDX event table explicitly marks ALL_LOADS and L1_HIT with `PEBS=1` and
`Data_LA=1`; ALL_LOADS includes software-prefetch uops. The fresh C loop contains
volatile scalar loads, not explicit prefetch instructions. Selected official
records and the downloaded source hash are retained in `intel-events.json`.

Upstream Linux v6.8 `arch/x86/events/intel/ds.c` gives Broadwell both 0x81d0 and
0xd1 DATALA_LD constraints (lines 988–1008). Lines 1729–1732 copy `pebs->dla` into
the sample address for appropriate sample flags/formats. See
`source-references.json` for URL/hash. This is reference source, not proof that
Ubuntu's running kernel binary is byte-identical to upstream.

These findings rule out the libcram decoder as a *necessary* cause of this
reproduction and rule out simply changing precise level as a repair. They do
not distinguish a CPU, firmware/microcode, kernel or event-interaction cause.
Do not invent a diagnosis, rewrite sampled addresses, or silently substitute a
cache-hit event and call it all-load acceptance.

## Reproduction and evidence

`probe.c` is the compiled workload, with `cc -O2 -Wall -Wextra -Werror` passing.
`recorded-user-runner.py` is the historical collection script, intended to be
copied to `target/pebs_address_diagnostic.py` and paired with a copy of `probe.c`
at `target/pebs-address-probe.c` if intentionally rerun on the owned s5 host.
It creates a unique remote directory, compiles, collects each event serially,
extracts only sample addresses, records child exits and removes that directory.
It is an exploratory collector, not a passing acceptance-gate command.

Each `results.json` records exact commands, exits and owned address bounds.
The `.txt.gz` files contain byte-preserved `perf script -F addr` output. Native
ELF/source hashes, workload logs and current host inventory are retained. The
receipts and decompressed address counts were independently checked against the
recorded bounds. `checksums.json` covers evidence file bytes.

## Remaining gate

C remains OPEN. A qualified owned Linux machine where ALL_LOADS actually yields
correct addresses, or a separately validated fix for this host, is required to
advance that profile's acceptance. L1-hit remains explicitly narrower. Total
active kernel sampling plus collector CPU below 1% of one core also remains
unmeasured; no shared-host throughput proxy was substituted. B and real-CRAM
hardware/long-soak requirements are unchanged. No production source was edited
for this investigation.

# PEBS core/event-interaction diagnostic — C remains OPEN

## New result

The ALL_LOADS data-address failure is reproduced on logical CPUs 12 and 28,
with the same explicitly audited Broadwell-X events. Co-recording L1_HIT or
ALL_STORES, and reversing the L1_HIT/ALL_LOADS group order, does not repair it.
This is a diagnostic, not a production code fix or C acceptance.

All samples in every recorded stream identify instruction `0x401208`, the
owned binary's `mov (%rax),%rcx`. ALL_LOADS nevertheless reports only
`0xfeb80008`, `0xfeb80040`, or `0xfeb80048` as data addresses; none is in its
fresh 16 KiB mapping. L1_HIT reports addresses inside its own mapping.
Thus correct eventing IP does not establish correct data address.

| Case | CPU | ALL_LOADS: owned / samples | L1_HIT: owned / samples |
| --- | ---: | ---: | ---: |
| ALL_LOADS alone | 12 | 0 / 1653 | — |
| L1_HIT alone | 12 | — | 1659 / 1659 |
| ALL_LOADS + ALL_STORES group | 12 | 0 / 1677 | — |
| ALL_LOADS + L1_HIT group | 12 | 0 / 1618 | 1618 / 1618 |
| L1_HIT + ALL_LOADS group | 12 | 0 / 1656 | 1656 / 1656 |
| ALL_LOADS alone | 28 | 0 / 1681 | — |
| L1_HIT alone | 28 | — | 1650 / 1650 |

Every `perf record` and `perf script` command returned zero. ALL_STORES was an
enabled group member, but did not produce samples in this load-only workload;
this is not new store-attribution evidence. Group order is not proof of actual
hardware-counter assignment. The two logical CPUs are not two independent
machines. Results do not rule out every counter, scheduling or event interaction.

## Scope and source identity

- Newly created owned process only, no `perf -a` or attachment to other workloads.
- `precise_ip=2`, userspace-only; event raw encodings ALL_LOADS 0x81d0,
  L1_HIT 0x01d1, ALL_STORES 0x82d0; sample period 2,000,003.
- Same C workload bytes as the preceding independent diagnostic, compiled with
  `cc -O2 -g -fno-pie -no-pie -Wall -Wextra -Werror`. Non-PIE permits direct
  instruction-address comparison with the archived disassembly.
- Host inventory and exact per-case commands/PIDs/mapping bounds are archived.
  Kernel 6.8.0-146-generic; CPU12 reports Xeon E5-2683 v4, family 6/model 79,
  stepping 1, microcode 0xb000040. No sysctl, security, firmware, VM, driver,
  affinity of other processes, or production source was changed.
- Both newly created remote directories were removed after collecting artifacts.
  Existing remote libcram artifacts were neither overwritten nor removed.

## Parser failure and independent correction

The first diagnostic parser mistakenly treated `perf script -F event,ip,addr`
as if fields were printed in argument order. On this host the output is
`event, addr, ip`. Its invalid summaries are retained separately as
`results-initial-invalid.json`; they are **not acceptance evidence**.

The fixture was corrected to match captured output and failed with the old
parser (exit 1, `parser-red.log.gz`). The parser was corrected and the same
raw captures reprocessed, without additional sampling. `parser-green.log.gz`
records the fixture and positive-control checks.

For independent confirmation, the original `.data` files were replayed using
`perf script -F addr`, with no multi-column parsing. Every case's total and
in-mapping count matches the corrected multi-field result. The second SSH
operation only replayed stored captures; it did not collect new samples.
`address-only-receipt.json` and byte-preserved compressed outputs record this.

## Reproduction and limitations

Run the offline evidence checker from the repository root:

```
python3 docs/evidence/pebs-event-interaction/verify.py
```

It checks parser boundary/empty/malformed fixtures, raw-data summaries,
address-only counts, instruction bytes, source consistency and checksums.
Its success means **consistent failing evidence**, not working ALL_LOADS.
It performs no SSH or sampling.

`recorded-runner.py` is the collection script with the corrected parser; the
version at initial collection used the wrong column assignment documented above.
For an explicitly authorized new run, copy it into the canonical repository at
`target/pebs_interaction_probe.py` and invoke `--run --output target/<fresh-name>`.
Without `--run` it only checks parser fixtures. `recorded-replay.py` is a
historical SSH replay driver; do not run/import it merely to inspect syntax.
The raw `remote-artifacts.tar.gz` includes perf data, ELF, C, disassembly and
child exits. Logs are losslessly compressed; no output was edited to pass
whitespace checks. `checksums.json` excludes only itself.

Intel specification update 333811-005US (September 2019) was examined alongside
the prior event-table/kernel references. The inspected PEBS errata do not
establish a direct cause of this failure. In particular, BDF68 describes writes
after VMX transitions; that is not proof it explains these records. Reference
URL/document hash are in `references.json`. A read-only `/proc/iomem` lookup did
not match the repeated bad address; no device or memory-content diagnosis is
claimed, and physical memory was not read.

C still needs a separately validated host/kernel/firmware remedy or a qualified
owned Linux PMU for comparison. No host upgrade, reboot or global setting change
is authorized by this diagnostic. Kernel-plus-collector CPU <1% of one core,
universal load coverage and real CRAM acceptance remain unestablished. Neither
full SDK nor workspace tests were rerun for this evidence-only change.

# Exact entropy update — 2026-10-09

The banked histogram remains exact for arbitrary byte values, unaligned starts,
uniform prefixes, tails and multi-batch inputs. Differential tests include
pre-populated histograms and the 32 KiB/64 KiB boundaries; they passed on M1 and
native Linux x86_64. No distribution, budget, cached result or approximate
entropy was substituted.

Source `crates/libcram-core/src/entropy/mod.rs` SHA-256:
`fb05adbe76e08f068e0acb2d4691070d76cedec332dd0f02c666901390303709`.
Implementation entered canonical history in `fb36d58`; packaging/telemetry
changes do not change that algorithm.

## Strict 4 KiB gate

These are **upper 95% confidence bounds of the mean**, in nanoseconds, from the
retained Criterion estimates (not the displayed fitted slope interval).

| Target | Zero | ASCII | Random | Strict <1000 ns |
| --- | ---: | ---: | ---: | --- |
| Apple M1 Pro / NEON | 163.57 | 1129.74 | 1120.76 | FAIL |
| Owned s5 x86_64 VM / AVX2 | 243.76 | 2970.97 | 2937.34 | FAIL |

The Linux run completed successfully and reported improved performance against
its stored previous baseline; its **acceptance script still exited 1**.
Re-running the unchanged acceptance script over both retained datasets also
exited 1 with ASCII and random failures. M1 improvements are small, not closure
of the original requirement. A successful benchmark process is not a passed
budget gate.

## Evidence and limitations

[Retained datasets, SHA-256 manifests and gate JSON](../evidence/2026-10-09/entropy-update/):

- `m1-evidence.tar.gz`: worker baseline/candidate Criterion artifacts; source and
  experiment detail in [entropy-next.md](entropy-next.md) and the historical
  [handoff](../entropy-handoff.md).
- `linux-criterion.tar.gz`: VM Criterion tree captured after the new 4096-byte
  dispatch run. Only the three `entropy/dispatch_*/4096/new` datasets are new
  timing evidence here; other sizes/old comparison artifacts are not reruns.
- `m1-gate.json` and `linux-gate.json`: actual gate output reproduced from those
  archives, each return code 1.

The bounded optimization cycle did not achieve the requested performance on
either measured machine. Cross-compiling an AVX-512 path is not timing evidence
for an AVX-512 processor. This performance requirement remains explicitly open.

# Redactions

## 2026-11-02: local identifiers

Local identifiers were removed from committed evidence. Measurements, commands
and results are unchanged.

| Removed                                                | Replaced with                           |
| ------------------------------------------------------ | --------------------------------------- |
| Workspace path under the maintainer's macOS home       | `$WORKSPACE`                            |
| Other paths under that home directory                  | `$HOME`                                 |
| Claude Code session scratchpad path                    | `$SCRATCH`                              |
| Physical host name of s5 (text logs)                   | `s5`                                    |
| Physical host name of s5 (`perf.data` hostname header) | `s5`, NUL-padded to the original length |

Archives were rewritten with the same members, modes and timestamps:
`entropy-update/m1-evidence.tar.gz` (macOS provenance xattrs dropped),
`pebs-event-interaction/remote-artifacts.tar.gz` and
`pebs-event-interaction/parser-red.log.gz`. The `perf.data` files still parse;
only the hostname string changed.

Every recorded SHA-256 of a changed file was updated to the redacted content.
[`redactions-2026-11-02.tsv`](redactions-2026-11-02.tsv) lists each changed
file or archive member (`archive!member`) with its hash before and after.

History was rewritten the same day (`git filter-repo`, every branch) with the
same rules applied to every past version of every file, so no commit contains
the removed identifiers. Recorded hashes of past file versions in source
snapshot manifests were updated the same way. The TSV covers the current files
only.

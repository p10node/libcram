# Gate C follow-up (s5, 2026-11-02)

- [source-split](source-split/README.md): PEBS attribution bias measured against
  the read/write policy. Per-data-source events do not fix it; store samples
  are biased as well, and a write-heavy cache-resident region is classified
  read-dominant.
- [burst-budget](burst-budget/README.md): `PERF_EVENT_IOC_REFRESH` holds an
  exact per-interval sample budget under bursts up to 71k samples per 100 ms.

Implemented after these measurements:

- [write-track](write-track/README.md): `telemetry::write_sampler::WriteSampler`
  (userfaultfd asynchronous write-protect, sampled) feeds `Bridge::track_writes`.
  Write-heavy regions are never reported write-free; read-only ones are,
  end to end on hardware (`WRITE_POLICY_PASS` 3/3).
- [burst-budget](burst-budget/README.md): `ProcessPebsCollector` bounds each
  CPU with kernel-enforced limits (≤ 240 samples per 100 ms). After idle, the
  governor alone averaged 1,664–1,724 samples/s and failed two of three
  streaming runs with ring loss.
- `../../PEBS_PROFILE_AUDIT.md`: the procedure for a new CPU model.

Hashes of every binary run on the host are in `binaries.sha256`.

# PEBS attribution bias against the read/write policy (s5, 2026-11-02)

Physical `s5`, Xeon E5-2683 v4, microcode `0xb000040`, Linux 6.8.0-146, owned
process only, pinned to CPU 30, user-only, precise 2. `srcsplit.c` reads the
kernel's perf rings directly (no BPF, no libcram). Raw output: `srcsplit.log`,
three alternating runs of 5·10⁷ steps per configuration.

Each step retires exactly 11 loads and 3 stores (checked in the disassembly and
by the PMU counts: 1.0003–1.0010 and 1.0035–1.0057 of expected; no
multiplexing):

| Region | Kind                         | Loads/step | Stores/step | `reads/4 > writes` |
| ------ | ---------------------------- | ---------: | ----------: | ------------------ |
| A      | 16 KiB, L1-resident          |          8 |           0 | yes                |
| B      | 64 MiB pointer chase (DRAM)  |          1 |           0 | yes                |
| C      | 16 KiB, L1-resident          |          1 |           2 | no                 |
| D      | 64 MiB chase, store after it |          1 |           1 | no                 |

Configurations, each sample weighted by its event's period:

- union: `0x67d1` loads (P=20011) + `0x82d0` stores (P=20323), as libcram today;
- split: `0x41d1` (L1+LFB), `0x06d1` (L2+L3), `0x20d1` (DRAM) loads + stores,
  pairwise coprime periods 20011/20323/20641/20963.

Estimated accesses per step (true value in parentheses), 3 runs:

| Config | A reads (8) | B reads (1) | C reads (1) | C writes (2) | D reads (1) | D writes (1) | C classified as        |
| ------ | ----------- | ----------- | ----------- | ------------ | ----------- | ------------ | ---------------------- |
| union  | 4.22–5.29   | 3.75–4.73   | 0.34–0.39   | 0.004–0.010  | 0.10–0.12   | 2.29–2.87    | read-dominant (wrong)  |
| split  | 2.02–3.59   | 1.40–1.70   | 2.47–4.24   | 0.000        | 0.13–0.18   | 1.88–2.75    | read-dominant (wrong)  |

Findings:

- **Stores are biased too.** Almost every store sample lands on D, the first
  store to retire after the DRAM stall. C's two stores per step are nearly
  invisible, so the write-heavy, cache-resident region C is classified
  read-dominant, i.e. eligible for CRAM, in all six runs.
- **Splitting loads by data source does not remove the bias.** Within the
  L1+LFB group, C's single load (the first L1 hit after the stalls) collects more
  samples than A's eight. The bias follows retirement bursts after any stall,
  not the data source of the sampled load.
- **Four PEBS events lose samples silently**: 50–88% of the expected L1-group
  samples and 63–92% of the store samples arrived, with no LOST record. The
  union pair delivered 77–96%.
- Totals stay exact (PMU counts), only the attribution to addresses is biased.

Conclusion: per-data-source events are rejected. On this CPU, PEBS sample
counts per page cannot decide read/write dominance when pages differ in cache
behaviour. Writes need an unbiased signal (see the follow-up options in
`../README.md`).

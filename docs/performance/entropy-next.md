# Exact entropy: four-bank follow-up

## Outcome: improved, strict 4 KiB gate still FAILS

The 1000 ns budget, all three distributions, input generation, exact count
semantics, entropy finalization, and existing mathematical weight cache are
unchanged. No input memoization, approximation, training, or sampled entropy was
introduced. This is a partial optimization, **not** completion of the latency
requirement and **not** a proof that the requirement is impossible.

Baseline: `f43fb9e7c64e5177c6e4193112c94c63d6e6f0d1`.
Measured on Apple M1 Pro, aarch64-apple-darwin, rustc 1.99.0
(b940084d7, LLVM 23.1.1), Cargo 1.99.0. Evidence captured 2026-10-09.
Only this worktree was modified. No remote runs, sibling writes, manifests,
lockfiles, benchmark distributions, or gate changes were made.

## Algorithm and safety

A shared four-bank u16 histogram replaces the AArch64 eight-bank histogram.
It uses 32-byte chunks, four native-endian word loads, and interleaved independent
counters. Endianness can change bank assignment but cannot change merged counts.
The counter workspace shrinks from 4096 to 2048 bytes. Batches are at most 32768
bytes, so each bank counter is at most 8192 and the four-counter sum is at most
32768. Tails increment the existing usize histogram directly. The merge is
additive and supports arbitrary input lengths. Short inputs avoid bank setup.
All new input loads use safe byte-array conversion; AArch64 retains the existing
register-only identity assembly to prevent LLVM splitting a word into byte loads.

The NEON uniform-prefix scan remains unchanged. AVX2 and AVX512BW now use the
same prefix-only strategy and shared banked histogram rather than repeatedly
checking each mixed vector and accumulating into one dependency-heavy histogram.
ISA detection remains unchanged. The x86 paths compile for Linux but were not
executed here; parent-owned native x86 measurement and differential execution
are required before claiming an x86 improvement. A local x86 Apple executable
could not run (`Errno 86: Bad CPU type in executable`); no Rosetta installation
or remote access was attempted.

## Frozen Criterion receipts

Both baseline and candidate used exactly:

```sh
cargo bench --locked --bench entropy -- 'entropy/dispatch/.*/4096'
python3 scripts/entropy_budget.py --sla strict target/criterion
```

Criterion defaults were retained: 3-second warm-up, 100 samples, estimated
5-second measurement, 95% confidence. Benchmarks ran sequentially, never in
parallel with another benchmark started by this worker. No CPU-specific rustflags
were added. Baseline was run before production edits; candidate source was frozen
and hashed before its single final Criterion run. Hashes were checked afterward.

| Input | Baseline upper mean (ns) | Candidate upper mean (ns) | Strict result |
|---|---:|---:|---|
| zero | 155.50913324287183 | 163.5655474418708 | PASS |
| ascii | 1165.2066071892564 | 1129.7400844157053 | FAIL |
| random | 1127.5274222925211 | 1120.7620133141659 | FAIL |

The unchanged gate exits **1** for both baseline and candidate. Criterion
identifies the ASCII change as improved, with mean-change interval
[-3.4576%, -2.6192%]. Random has a negative estimated change but the interval
[-4.8035%, -0.2847%] crosses Criterion's noise threshold: report it as
**within noise threshold**, not a statistically clean material improvement.
Zero reports no detected change. The broader random candidate interval includes
several high outliers; it is retained rather than discarded or rerun until green.
The optimized implementation is retained for the demonstrated ASCII improvement,
smaller workspace, and stronger small-input diagnostic results, not because the
4 KiB goal has been achieved.

Raw Criterion trees are preserved at `target/entropy-baseline-criterion/` and
`target/entropy-candidate-criterion/`; both retain benchmark metadata, estimates,
samples and comparison data. Logs and gate output are
`target/entropy-{baseline,candidate}-{criterion.log,budget.json}`. These ignored
artifacts must be copied before worktree cleanup. The receipt appendix below
preserves full benchmark metadata and estimates plus hashes even if target is
later removed.

## Differential RED/GREEN and build checks

The new `banked_histogram_collisions_tails_and_batch_boundaries` test was written
before the helper implementation. With an empty helper body the test actually
ran and failed at `pattern=0, offset=0, len=1` (exit 101), rather than failing to
compile. Implementing the helper turned the same test GREEN. It checks additive
histograms initialized to 17, six patterns (uniform 255, lane collisions,
four-byte runs, heavily skewed, every byte value, seeded random), 32 alignments,
short tails and 32768/65536 batch edges through 131073 bytes.

The existing exhaustive test still covers every length through 4097, 64
alignments, larger batches and a single nonuniform byte at every prefix position.
Final entropy remains bit-for-bit equal to scalar entropy; existing independent
Shannon-formula checks remain in place.

Successful commands:

```sh
cargo test --locked -p libcram-core entropy
cargo test --locked --release -p libcram-core entropy
cargo clippy --locked -p libcram-core --all-targets -- -D warnings
cargo +1.89.0 check --locked -p libcram-core
cargo check --locked -p libcram-core --target x86_64-unknown-linux-gnu
```

Debug and release each passed four entropy unit tests and one matching migration
integration test. Rust 1.89 compatibility was checked on AArch64; x86 Linux was
compile-checked on the installed stable toolchain, not run. Logs live under
`target/entropy-{red,green,debug-tests,clippy,msrv,x86-check}.log`.

## Diagnostics (not acceptance statistics)

The existing component profiler was read and rerun unchanged. Candidate medians:

| Input | Histogram (ns) | Finish (ns) | Public total (ns) |
|---|---:|---:|---:|
| zero | 88.68 | 63.19 | 147.79 |
| ascii | 1047.70 | 63.40 | 1106.72 |
| random | 975.57 | 63.63 | 1023.07 |

The histogram remains the bottleneck. Separate component boundaries make these
medians non-additive. An additional sequential paired diagnostic used the exact
benchmark generators, 10000 warm-up calls, nine batches of 100000 calls and median
batch times. It is not a substitute for Criterion confidence bounds:

| Input/length | Baseline median (ns) | Candidate median (ns) |
|---|---:|---:|
| zero/64 | 103.03 | 103.64 |
| ascii/64 | 208.56 | 164.91 |
| random/64 | 205.73 | 164.69 |
| zero/512 | 98.52 | 100.20 |
| ascii/512 | 311.52 | 271.21 |
| random/512 | 307.84 | 261.45 |
| zero/4096 | 151.35 | 147.99 |
| ascii/4096 | 1173.86 | 1117.91 |
| random/4096 | 1137.82 | 1038.15 |

Small mixed-input diagnostic gains are approximately 20–21% at 64 bytes and
13–15% at 512 bytes. They have not been promoted to acceptance claims.

## Rejected exploration

The previous investigation already rejected alternate bank strides, transposed
histograms and eight-bank word unrolling; those were not repeated verbatim.
New experiments varied direct-byte vs intact-word loads, counter width and bank
count, four-bank unrolling, and explicit ARM load/add/store scheduling. They used
fresh standalone copies under target, checked scalar equality for the benchmark
inputs, and retained the existing finalization. Exploration was not full
adversarial verification; only the selected candidate received that verification.

* Direct byte loads: u16/u32/usize, 2/4/8/16 banks. No winner; typical totals
  were 1.3–2.1 microseconds. Several measurements showed background jitter.
* Intact-word loads: u8/u16/u32/usize, 2/4/8 banks (existing u16 eight-bank omitted).
  u8 batches had to shrink to prevent overflow, making repeated initialization
  and merge costs dominate. Two banks helped random but retained skew dependency
  stalls. Four u16 banks balanced setup, collision dependencies and reduction.
* Four-bank unrolling: 32-byte chunks gave exploratory ASCII/random totals
  1129.43/1049.69 ns; 64-byte chunks were not better. Three- and six-bank layouts
  did not win across both distributions.
* Explicit ARM scheduling: four/eight banks, serial vs all-loads-first vs paired
  load/add schedules. None beat the selected Rust four-bank loop. The first
  scratch build used LLVM-reserved x19; it was corrected to x27, and x18 was
  avoided because Apple reserves it. These scratch assembly kernels are rejected
  and are **not** in production.
* Vector collision handling was considered but not claimed as implemented or
  measured. A viable portable vector histogram without scatter/conflict support
  remains an open algorithmic direction, not an established speedup.

The appendix preserves the generators and outputs of these throwaway experiments
so the worktree's ignored files are not the only record. These scripts are
historical experiments against the baseline, not CI tools or production APIs.

## Frozen source hashes

```text
fb05adbe76e08f068e0acb2d4691070d76cedec332dd0f02c666901390303709  crates/libcram-core/src/entropy/mod.rs
00690aaaa7569413d5286a8541dfc3704f95d3a675a1dd579804cfbae18132a7  crates/libcram-core/src/entropy/differential.rs
ddb3052bc8219ebfdf648088920c4ff77cbcdf5200ce4c59c00d7e8544416ac5  benches/entropy.rs
49adc276ad61c264288ed0c92dc392ee07b661163bbfe27b52fdf56c0464535a  scripts/entropy_budget.py
2c7aeddd5047c12183407cbd1d04309cf5863ae95263bd3023f5d94145bb2975  Cargo.lock
```

## Criterion metadata and estimates (preserved receipts)

All values below come from the actual saved files; SHA-256 hashes cover their original bytes.

### baseline: dispatch/ascii/4096

- `target/entropy-baseline-criterion/entropy/dispatch_ascii/4096/new/benchmark.json` SHA-256: `37f577cda8ff4b2c35219a426a6ede3f27a12175e9e6d09201323dddb83dfd0c`
- `target/entropy-baseline-criterion/entropy/dispatch_ascii/4096/new/estimates.json` SHA-256: `d8a7054b8964c6b46dfa2cc7176393593c86fbcf3929dfd59491dafdfb18bfee`
- `target/entropy-baseline-criterion/entropy/dispatch_ascii/4096/new/sample.json` SHA-256: `126203a885b96115e603b39aee0b9842cf9906131e0796d8d0d73a2c6ec63422`

```json
{
  "benchmark": {
    "group_id": "entropy",
    "function_id": "dispatch/ascii",
    "value_str": "4096",
    "throughput": {
      "Bytes": 4096
    },
    "full_id": "entropy/dispatch/ascii/4096",
    "directory_name": "entropy/dispatch_ascii/4096",
    "title": "entropy/dispatch/ascii/4096"
  },
  "estimates": {
    "mean": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 1157.7269470129932,
        "upper_bound": 1165.2066071892564
      },
      "point_estimate": 1161.2541509313396,
      "standard_error": 1.9150261531270791
    },
    "median": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 1153.104752045746,
        "upper_bound": 1159.5790706526514
      },
      "point_estimate": 1155.9720699861805,
      "standard_error": 1.6920590240599034
    },
    "median_abs_dev": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 8.882084591630523,
        "upper_bound": 15.185943030792734
      },
      "point_estimate": 11.721185772210847,
      "standard_error": 1.593974971673227
    },
    "slope": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 1160.8973592710017,
        "upper_bound": 1169.088807466046
      },
      "point_estimate": 1164.8797755508292,
      "standard_error": 2.0923161280357645
    },
    "std_dev": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 13.766104797720962,
        "upper_bound": 24.763012764505184
      },
      "point_estimate": 19.244940044887453,
      "standard_error": 2.8692225553584985
    }
  }
}
```

### baseline: dispatch/random/4096

- `target/entropy-baseline-criterion/entropy/dispatch_random/4096/new/benchmark.json` SHA-256: `9c67d715ad5b24bfd88f24a9ea2fedef30cdb471a3378ef7722524c3b125d75c`
- `target/entropy-baseline-criterion/entropy/dispatch_random/4096/new/estimates.json` SHA-256: `24cb9eee0b4476ff47d354322bcdd05cc728865d3fddc312478592e40d8cf29c`
- `target/entropy-baseline-criterion/entropy/dispatch_random/4096/new/sample.json` SHA-256: `b8b8a6b0706414127d6a356d77f1f72b913c47ad6b3eb1f8f191babf4fd11c24`

```json
{
  "benchmark": {
    "group_id": "entropy",
    "function_id": "dispatch/random",
    "value_str": "4096",
    "throughput": {
      "Bytes": 4096
    },
    "full_id": "entropy/dispatch/random/4096",
    "directory_name": "entropy/dispatch_random/4096",
    "title": "entropy/dispatch/random/4096"
  },
  "estimates": {
    "mean": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 1121.076310907801,
        "upper_bound": 1127.5274222925211
      },
      "point_estimate": 1124.213876658764,
      "standard_error": 1.641266668387384
    },
    "median": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 1116.3536859979101,
        "upper_bound": 1126.3293956043956
      },
      "point_estimate": 1120.5431224654792,
      "standard_error": 2.4448197841682573
    },
    "median_abs_dev": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 11.845081032580643,
        "upper_bound": 19.79534000176097
      },
      "point_estimate": 15.30854069781028,
      "standard_error": 2.0268093064571477
    },
    "slope": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 1119.959302351692,
        "upper_bound": 1127.3665130350564
      },
      "point_estimate": 1123.409222987224,
      "standard_error": 1.8914103072950756
    },
    "std_dev": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 13.49887584471981,
        "upper_bound": 19.34343826687954
      },
      "point_estimate": 16.4489602130196,
      "standard_error": 1.5022657917799271
    }
  }
}
```

### baseline: dispatch/zero/4096

- `target/entropy-baseline-criterion/entropy/dispatch_zero/4096/new/benchmark.json` SHA-256: `7de32e8a871bed8cac87f73107b15cb193f280300c9a480bc16ad92830c76b38`
- `target/entropy-baseline-criterion/entropy/dispatch_zero/4096/new/estimates.json` SHA-256: `caa8226228e9fbc6b95dd04f64018d2436a602ee581394de13d6046a9971e287`
- `target/entropy-baseline-criterion/entropy/dispatch_zero/4096/new/sample.json` SHA-256: `5bf669bca51dc9a823db3b82d5736510f798ef4ca245e0aa7130668839295675`

```json
{
  "benchmark": {
    "group_id": "entropy",
    "function_id": "dispatch/zero",
    "value_str": "4096",
    "throughput": {
      "Bytes": 4096
    },
    "full_id": "entropy/dispatch/zero/4096",
    "directory_name": "entropy/dispatch_zero/4096",
    "title": "entropy/dispatch/zero/4096"
  },
  "estimates": {
    "mean": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 154.48624515210847,
        "upper_bound": 155.50913324287183
      },
      "point_estimate": 154.98417583793253,
      "standard_error": 0.25858625633668136
    },
    "median": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 153.71718078681212,
        "upper_bound": 155.1937914059195
      },
      "point_estimate": 154.69980079935135,
      "standard_error": 0.3894168905739122
    },
    "median_abs_dev": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 1.8425405387938356,
        "upper_bound": 3.1377737534678563
      },
      "point_estimate": 2.676107936150376,
      "standard_error": 0.3296975165157202
    },
    "slope": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 153.4055105664472,
        "upper_bound": 154.67514272910404
      },
      "point_estimate": 153.96843877470903,
      "standard_error": 0.32418665436431493
    },
    "std_dev": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 2.0947865570814197,
        "upper_bound": 3.103989102880294
      },
      "point_estimate": 2.6055453489647182,
      "standard_error": 0.26152365451556875
    }
  }
}
```

### candidate: dispatch/ascii/4096

- `target/entropy-candidate-criterion/entropy/dispatch_ascii/4096/new/benchmark.json` SHA-256: `37f577cda8ff4b2c35219a426a6ede3f27a12175e9e6d09201323dddb83dfd0c`
- `target/entropy-candidate-criterion/entropy/dispatch_ascii/4096/new/estimates.json` SHA-256: `16c7d36192aadec4e5b78affd4722a986421e3f47b30220ed059989f2dbeaa9b`
- `target/entropy-candidate-criterion/entropy/dispatch_ascii/4096/new/sample.json` SHA-256: `af1bd2376c47d5005af1a439e51c47414d6e081d2449a511d13fda4368c95631`

```json
{
  "benchmark": {
    "group_id": "entropy",
    "function_id": "dispatch/ascii",
    "value_str": "4096",
    "throughput": {
      "Bytes": 4096
    },
    "full_id": "entropy/dispatch/ascii/4096",
    "directory_name": "entropy/dispatch_ascii/4096",
    "title": "entropy/dispatch/ascii/4096"
  },
  "estimates": {
    "mean": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 1122.8231029749202,
        "upper_bound": 1129.7400844157053
      },
      "point_estimate": 1126.1181048376668,
      "standard_error": 1.766168230528666
    },
    "median": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 1118.3668940870696,
        "upper_bound": 1125.2947368421053
      },
      "point_estimate": 1121.4049678067222,
      "standard_error": 1.6327389513256696
    },
    "median_abs_dev": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 7.4605994747601825,
        "upper_bound": 16.17454903470663
      },
      "point_estimate": 10.88199514828041,
      "standard_error": 2.0724670777750163
    },
    "slope": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 1125.9930831276126,
        "upper_bound": 1136.0001409002723
      },
      "point_estimate": 1130.7175790942802,
      "standard_error": 2.563089268478705
    },
    "std_dev": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 12.794784199360949,
        "upper_bound": 22.1931456413439
      },
      "point_estimate": 17.674729915835375,
      "standard_error": 2.411675470994241
    }
  }
}
```

### candidate: dispatch/random/4096

- `target/entropy-candidate-criterion/entropy/dispatch_random/4096/new/benchmark.json` SHA-256: `9c67d715ad5b24bfd88f24a9ea2fedef30cdb471a3378ef7722524c3b125d75c`
- `target/entropy-candidate-criterion/entropy/dispatch_random/4096/new/estimates.json` SHA-256: `6966f1eb1156be5ec6d45387b564a93ccf6134fc2c1fbe541c5621185128f2aa`
- `target/entropy-candidate-criterion/entropy/dispatch_random/4096/new/sample.json` SHA-256: `b01f3b08f65cefc42f9f2ef1daeb367c1732018ad4899c16849b7da131013945`

```json
{
  "benchmark": {
    "group_id": "entropy",
    "function_id": "dispatch/random",
    "value_str": "4096",
    "throughput": {
      "Bytes": 4096
    },
    "full_id": "entropy/dispatch/random/4096",
    "directory_name": "entropy/dispatch_random/4096",
    "title": "entropy/dispatch/random/4096"
  },
  "estimates": {
    "mean": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 1069.709208696578,
        "upper_bound": 1120.7620133141659
      },
      "point_estimate": 1091.616389770213,
      "standard_error": 13.183889334177271
    },
    "median": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 1042.6459808495324,
        "upper_bound": 1055.509174109442
      },
      "point_estimate": 1047.822296822094,
      "standard_error": 3.1449199018543044
    },
    "median_abs_dev": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 11.739960551345431,
        "upper_bound": 28.079585413360192
      },
      "point_estimate": 19.159927313618052,
      "standard_error": 4.208775305239938
    },
    "slope": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 1062.491183990023,
        "upper_bound": 1104.0633150819585
      },
      "point_estimate": 1081.543801797978,
      "standard_error": 10.692852997932754
    },
    "std_dev": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 59.23369031138072,
        "upper_bound": 204.01205403384267
      },
      "point_estimate": 132.6823904503169,
      "standard_error": 43.00362469225517
    }
  }
}
```

### candidate: dispatch/zero/4096

- `target/entropy-candidate-criterion/entropy/dispatch_zero/4096/new/benchmark.json` SHA-256: `7de32e8a871bed8cac87f73107b15cb193f280300c9a480bc16ad92830c76b38`
- `target/entropy-candidate-criterion/entropy/dispatch_zero/4096/new/estimates.json` SHA-256: `9950925c6f51eea8ff1eb80cff99b1ad06010aa1f0aaa82739459dfb449f03ca`
- `target/entropy-candidate-criterion/entropy/dispatch_zero/4096/new/sample.json` SHA-256: `f2a404576b9d54738448d7b29c37ba94db2b5239f47509ceff5664b739b93e31`

```json
{
  "benchmark": {
    "group_id": "entropy",
    "function_id": "dispatch/zero",
    "value_str": "4096",
    "throughput": {
      "Bytes": 4096
    },
    "full_id": "entropy/dispatch/zero/4096",
    "directory_name": "entropy/dispatch_zero/4096",
    "title": "entropy/dispatch/zero/4096"
  },
  "estimates": {
    "mean": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 154.47235591062687,
        "upper_bound": 163.5655474418708
      },
      "point_estimate": 157.6299100311422,
      "standard_error": 2.8144755448533862
    },
    "median": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 153.9807311090548,
        "upper_bound": 154.67730461322736
      },
      "point_estimate": 154.27808406952542,
      "standard_error": 0.19228893343801243
    },
    "median_abs_dev": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 1.2501786600684657,
        "upper_bound": 2.091326735472295
      },
      "point_estimate": 1.674547925277863,
      "standard_error": 0.21620042520908955
    },
    "slope": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 154.11093405275392,
        "upper_bound": 159.52517676066572
      },
      "point_estimate": 155.87715098823477,
      "standard_error": 1.558667684720292
    },
    "std_dev": {
      "confidence_interval": {
        "confidence_level": 0.95,
        "lower_bound": 2.0581305453279546,
        "upper_bound": 47.71968436071679
      },
      "point_estimate": 27.885472512721744,
      "standard_error": 16.39780098195339
    }
  }
}
```

## Preserved exploration source and output

These sources ran before production edits. Reproduce against a separate baseline
snapshot at the recorded base commit: the textual substitution anchors intentionally
refer to the old eight-bank code. Extract these code blocks into the named target
files in that snapshot; do not rewrite the integrated candidate to run experiments.
All generated sources and binaries remain in the owned ignored target directory.

### target/entropy-explore.py

```python
from pathlib import Path
import subprocess, importlib.util
root=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('cost',root/'docs/performance/entropy-costs.py'); cost=importlib.util.module_from_spec(spec);spec.loader.exec_module(cost)
src=(root/'crates/libcram-core/src/entropy/mod.rs').read_text()
start=src.index('    // Eight independent counters')
end=src.index('\n}\n#[cfg(target_arch = "x86_64")]',start)
work=root/'target/entropy-experiments';work.mkdir(exist_ok=True)
probe=cost.PROBE.replace('0..100000','0..30000').replace('/ 100000.0','/ 30000.0').replace('0..9','0..5').replace('samples[4]','samples[2]')
(work/'probe.rs').write_text(probe)
variants={'baseline':src}
for typ in ['u16','u32','usize']:
 for banks in [2,4,8,16]:
  names=[f'h{i}' for i in range(banks)]
  body=f'    for batch in data.chunks(32768) {{\n        let mut counters = [[0{typ}; 256]; {banks}];\n        let [{", ".join(names)}] = &mut counters;\n        let (chunks, remainder) = batch.as_chunks::<{banks}>();\n        for chunk in chunks {{\n'
  body+=''.join(f'            h{i}[chunk[{i}] as usize] += 1;\n' for i in range(banks))
  body+='        }\n        for i in 0..256 {\n            hist[i] += '+ ' + '.join(f'h{j}[i] as usize' for j in range(banks))+';\n        }\n        scalar_hist(remainder, hist);\n    }'
  variants[f'direct-{typ}-{banks}']=src[:start]+body+src[end:]
for name,source in variants.items():
 (work/'mod.rs').write_text(source+cost.WRAPPERS)
 subprocess.run(['rustc','--edition=2021','-O',str(work/'probe.rs'),'-o',str(work/'probe')],check=True,capture_output=True)
 out=subprocess.check_output([str(work/'probe')],text=True)
 (work/(name+'.log')).write_text(out)
 (work/(name+'.rs')).write_text(source)
 print(name+'\n'+out,flush=True)
```

```text
baseline
zero/histogram: median 78.52 ns
zero/finish: median 65.70 ns
zero/total: median 152.68 ns
ascii/histogram: median 1120.02 ns
ascii/finish: median 65.43 ns
ascii/total: median 1196.60 ns
random/histogram: median 1086.57 ns
random/finish: median 65.24 ns
random/total: median 1159.39 ns

direct-u16-2
zero/histogram: median 77.96 ns
zero/finish: median 65.38 ns
zero/total: median 151.08 ns
ascii/histogram: median 1361.46 ns
ascii/finish: median 65.83 ns
ascii/total: median 1478.59 ns
random/histogram: median 1255.33 ns
random/finish: median 65.74 ns
random/total: median 1310.78 ns

direct-u16-4
zero/histogram: median 112.39 ns
zero/finish: median 95.91 ns
zero/total: median 229.90 ns
ascii/histogram: median 1860.61 ns
ascii/finish: median 94.97 ns
ascii/total: median 2098.15 ns
random/histogram: median 1988.36 ns
random/finish: median 103.76 ns
random/total: median 1583.25 ns

direct-u16-8
zero/histogram: median 94.00 ns
zero/finish: median 80.91 ns
zero/total: median 178.67 ns
ascii/histogram: median 2084.82 ns
ascii/finish: median 92.26 ns
ascii/total: median 1891.75 ns
random/histogram: median 1373.39 ns
random/finish: median 67.38 ns
random/total: median 1400.71 ns

direct-u16-16
zero/histogram: median 75.01 ns
zero/finish: median 63.16 ns
zero/total: median 155.09 ns
ascii/histogram: median 1535.40 ns
ascii/finish: median 66.48 ns
ascii/total: median 1659.32 ns
random/histogram: median 2392.74 ns
random/finish: median 98.86 ns
random/total: median 2484.98 ns

direct-u32-2
zero/histogram: median 79.38 ns
zero/finish: median 157.48 ns
zero/total: median 186.75 ns
ascii/histogram: median 2480.12 ns
ascii/finish: median 70.33 ns
ascii/total: median 1744.12 ns
random/histogram: median 1692.55 ns
random/finish: median 95.59 ns
random/total: median 1819.76 ns

direct-u32-4
zero/histogram: median 76.89 ns
zero/finish: median 66.14 ns
zero/total: median 156.83 ns
ascii/histogram: median 1353.55 ns
ascii/finish: median 65.09 ns
ascii/total: median 1419.73 ns
random/histogram: median 1297.54 ns
random/finish: median 65.34 ns
random/total: median 1384.17 ns

direct-u32-8
zero/histogram: median 77.18 ns
zero/finish: median 65.84 ns
zero/total: median 153.11 ns
ascii/histogram: median 1386.71 ns
ascii/finish: median 65.54 ns
ascii/total: median 1502.31 ns
random/histogram: median 1416.35 ns
random/finish: median 65.17 ns
random/total: median 1471.68 ns

direct-u32-16
zero/histogram: median 77.62 ns
zero/finish: median 66.05 ns
zero/total: median 152.64 ns
ascii/histogram: median 1564.29 ns
ascii/finish: median 67.06 ns
ascii/total: median 1655.38 ns
random/histogram: median 1573.45 ns
random/finish: median 64.81 ns
random/total: median 1633.67 ns

direct-usize-2
zero/histogram: median 77.45 ns
zero/finish: median 65.31 ns
zero/total: median 150.87 ns
ascii/histogram: median 1474.27 ns
ascii/finish: median 64.77 ns
ascii/total: median 1545.14 ns
random/histogram: median 1302.88 ns
random/finish: median 63.45 ns
random/total: median 1365.80 ns

direct-usize-4
zero/histogram: median 74.07 ns
zero/finish: median 63.92 ns
zero/total: median 150.15 ns
ascii/histogram: median 1387.43 ns
ascii/finish: median 63.90 ns
ascii/total: median 1471.45 ns
random/histogram: median 1358.06 ns
random/finish: median 63.56 ns
random/total: median 1437.19 ns

direct-usize-8
zero/histogram: median 80.10 ns
zero/finish: median 65.30 ns
zero/total: median 152.76 ns
ascii/histogram: median 1531.51 ns
ascii/finish: median 64.17 ns
ascii/total: median 1598.52 ns
random/histogram: median 1517.23 ns
random/finish: median 63.86 ns
random/total: median 1612.29 ns

direct-usize-16
zero/histogram: median 87.51 ns
zero/finish: median 65.18 ns
zero/total: median 152.71 ns
ascii/histogram: median 2032.45 ns
ascii/finish: median 65.58 ns
ascii/total: median 2092.19 ns
random/histogram: median 1960.60 ns
random/finish: median 62.09 ns
random/total: median 2025.81 ns

```

### target/entropy-word-explore.py

```python
exec((__import__('pathlib').Path(__file__).resolve().parent/'entropy-explore.py').read_text().split("variants={'baseline':src}")[0])
variants={}
for typ in ['u8','u16','u32','usize']:
 for banks in [2,4,8]:
  if typ=='u16' and banks==8: continue
  names=[f'h{i}' for i in range(banks)]
  batch= 248*banks if typ=='u8' else 32768
  body=f'    for batch in data.chunks({batch}) {{\n        let mut counters = [[0{typ}; 256]; {banks}];\n        let [{", ".join(names)}] = &mut counters;\n        let (chunks, remainder) = batch.as_chunks::<8>();\n        for chunk in chunks {{\n            let mut word = unsafe {{ chunk.as_ptr().cast::<u64>().read_unaligned() }};\n            unsafe {{ std::arch::asm!("/* {{word}} */", word=inout(reg) word, options(pure,nomem,nostack,preserves_flags)); }}\n'
  body+=''.join(f'            h{i % banks}[((word >> {i*8}) & 255) as usize] += 1;\n' for i in range(8))
  body+='        }\n        for i in 0..256 {\n            hist[i] += '+ ' + '.join(f'h{j}[i] as usize' for j in range(banks))+';\n        }\n        scalar_hist(remainder, hist);\n    }'
  variants[f'word-{typ}-{banks}']=src[:start]+body+src[end:]
for name,source in variants.items():
 (work/'mod.rs').write_text(source+cost.WRAPPERS)
 subprocess.run(['rustc','--edition=2021','-O',str(work/'probe.rs'),'-o',str(work/'probe')],check=True,capture_output=True)
 out=subprocess.check_output([str(work/'probe')],text=True)
 (work/(name+'.log')).write_text(out);(work/(name+'.rs')).write_text(source)
 print(name+'\n'+out,flush=True)
```

```text
word-u8-2
zero/histogram: median 76.11 ns
zero/finish: median 67.18 ns
zero/total: median 150.64 ns
ascii/histogram: median 1444.29 ns
ascii/finish: median 65.22 ns
ascii/total: median 1519.00 ns
random/histogram: median 1304.56 ns
random/finish: median 62.71 ns
random/total: median 1369.39 ns

word-u8-4
zero/histogram: median 76.14 ns
zero/finish: median 64.91 ns
zero/total: median 153.18 ns
ascii/histogram: median 1385.39 ns
ascii/finish: median 62.74 ns
ascii/total: median 1442.59 ns
random/histogram: median 1314.24 ns
random/finish: median 62.42 ns
random/total: median 1381.33 ns

word-u8-8
zero/histogram: median 76.22 ns
zero/finish: median 62.92 ns
zero/total: median 150.18 ns
ascii/histogram: median 1515.49 ns
ascii/finish: median 65.60 ns
ascii/total: median 1644.93 ns
random/histogram: median 1483.33 ns
random/finish: median 67.21 ns
random/total: median 1550.07 ns

word-u16-2
zero/histogram: median 76.87 ns
zero/finish: median 64.43 ns
zero/total: median 144.78 ns
ascii/histogram: median 1157.75 ns
ascii/finish: median 62.92 ns
ascii/total: median 1224.73 ns
random/histogram: median 980.53 ns
random/finish: median 61.46 ns
random/total: median 1048.24 ns

word-u16-4
zero/histogram: median 123.55 ns
zero/finish: median 82.54 ns
zero/total: median 171.11 ns
ascii/histogram: median 1079.99 ns
ascii/finish: median 64.93 ns
ascii/total: median 1141.82 ns
random/histogram: median 1048.31 ns
random/finish: median 64.01 ns
random/total: median 1068.46 ns

word-u32-2
zero/histogram: median 112.27 ns
zero/finish: median 81.43 ns
zero/total: median 160.87 ns
ascii/histogram: median 1170.71 ns
ascii/finish: median 64.96 ns
ascii/total: median 1239.34 ns
random/histogram: median 987.63 ns
random/finish: median 62.78 ns
random/total: median 1056.70 ns

word-u32-4
zero/histogram: median 78.08 ns
zero/finish: median 65.56 ns
zero/total: median 146.33 ns
ascii/histogram: median 1090.46 ns
ascii/finish: median 65.95 ns
ascii/total: median 1186.55 ns
random/histogram: median 1018.68 ns
random/finish: median 61.91 ns
random/total: median 1090.65 ns

word-u32-8
zero/histogram: median 81.41 ns
zero/finish: median 62.29 ns
zero/total: median 149.93 ns
ascii/histogram: median 1144.13 ns
ascii/finish: median 62.63 ns
ascii/total: median 1224.11 ns
random/histogram: median 1118.04 ns
random/finish: median 61.84 ns
random/total: median 1172.28 ns

word-usize-2
zero/histogram: median 141.32 ns
zero/finish: median 90.28 ns
zero/total: median 174.36 ns
ascii/histogram: median 1204.39 ns
ascii/finish: median 61.92 ns
ascii/total: median 1277.76 ns
random/histogram: median 1033.88 ns
random/finish: median 63.44 ns
random/total: median 1143.32 ns

word-usize-4
zero/histogram: median 75.32 ns
zero/finish: median 64.34 ns
zero/total: median 152.77 ns
ascii/histogram: median 1209.90 ns
ascii/finish: median 66.06 ns
ascii/total: median 1262.47 ns
random/histogram: median 1121.98 ns
random/finish: median 62.15 ns
random/total: median 1218.40 ns

word-usize-8
zero/histogram: median 77.43 ns
zero/finish: median 67.49 ns
zero/total: median 149.38 ns
ascii/histogram: median 1333.05 ns
ascii/finish: median 62.88 ns
ascii/total: median 1368.73 ns
random/histogram: median 1248.28 ns
random/finish: median 62.15 ns
random/total: median 1316.47 ns

```

### target/entropy-unroll-explore.py

```python
exec((__import__('pathlib').Path(__file__).resolve().parent/'entropy-explore.py').read_text().split("variants={'baseline':src}")[0])
variants={}
for banks,wordsize,unroll in [(3,8,3),(4,8,2),(4,8,4),(4,8,8),(4,4,1),(4,4,4),(4,4,8),(6,8,3),(8,4,2)]:
 names=[f'h{i}' for i in range(banks)]; chunk=wordsize*unroll
 body=f'    for batch in data.chunks(32768) {{\n        let mut counters = [[0u16; 256]; {banks}];\n        let [{", ".join(names)}] = &mut counters;\n        let (chunks, remainder) = batch.as_chunks::<{chunk}>();\n        for chunk in chunks {{\n'
 for w in range(unroll):
  body+=f'            let mut word = unsafe {{ chunk.as_ptr().add({w*wordsize}).cast::<u{wordsize*8}>().read_unaligned() }};\n            unsafe {{ std::arch::asm!("/* {{word}} */", word=inout(reg) word, options(pure,nomem,nostack,preserves_flags)); }}\n'
  body+=''.join(f'            h{(w*wordsize+i) % banks}[((word >> {i*8}) & 255) as usize] += 1;\n' for i in range(wordsize))
 body+='        }\n        for i in 0..256 {\n            hist[i] += '+ ' + '.join(f'h{j}[i] as usize' for j in range(banks))+';\n        }\n        scalar_hist(remainder, hist);\n    }'
 variants[f'unroll-{banks}-{wordsize}-{unroll}']=src[:start]+body+src[end:]
for name,source in variants.items():
 (work/'mod.rs').write_text(source+cost.WRAPPERS)
 result=subprocess.run(['rustc','--edition=2021','-O',str(work/'probe.rs'),'-o',str(work/'probe')],capture_output=True,text=True)
 if result.returncode:print(name,result.stderr,flush=True);continue
 out=subprocess.check_output([str(work/'probe')],text=True)
 (work/(name+'.log')).write_text(out);(work/(name+'.rs')).write_text(source)
 print(name+'\n'+out,flush=True)
```

```text
unroll-3-8-3
zero/histogram: median 81.48 ns
zero/finish: median 73.49 ns
zero/total: median 165.09 ns
ascii/histogram: median 1173.78 ns
ascii/finish: median 64.30 ns
ascii/total: median 1238.71 ns
random/histogram: median 981.89 ns
random/finish: median 62.80 ns
random/total: median 1063.96 ns

unroll-4-8-2
zero/histogram: median 74.99 ns
zero/finish: median 66.51 ns
zero/total: median 151.45 ns
ascii/histogram: median 1098.25 ns
ascii/finish: median 66.03 ns
ascii/total: median 1146.75 ns
random/histogram: median 1004.42 ns
random/finish: median 65.90 ns
random/total: median 1069.00 ns

unroll-4-8-4
zero/histogram: median 136.21 ns
zero/finish: median 91.82 ns
zero/total: median 169.27 ns
ascii/histogram: median 1058.64 ns
ascii/finish: median 63.17 ns
ascii/total: median 1129.43 ns
random/histogram: median 982.36 ns
random/finish: median 65.21 ns
random/total: median 1049.69 ns

unroll-4-8-8
zero/histogram: median 143.24 ns
zero/finish: median 92.28 ns
zero/total: median 174.09 ns
ascii/histogram: median 1048.13 ns
ascii/finish: median 66.49 ns
ascii/total: median 1136.76 ns
random/histogram: median 990.31 ns
random/finish: median 63.48 ns
random/total: median 1048.75 ns

unroll-4-4-1
zero/histogram: median 76.58 ns
zero/finish: median 64.65 ns
zero/total: median 155.72 ns
ascii/histogram: median 1136.63 ns
ascii/finish: median 61.77 ns
ascii/total: median 1184.37 ns
random/histogram: median 1032.66 ns
random/finish: median 62.63 ns
random/total: median 1097.63 ns

unroll-4-4-4
zero/histogram: median 90.40 ns
zero/finish: median 70.07 ns
zero/total: median 154.64 ns
ascii/histogram: median 1088.00 ns
ascii/finish: median 62.89 ns
ascii/total: median 1135.98 ns
random/histogram: median 999.65 ns
random/finish: median 62.33 ns
random/total: median 1067.86 ns

unroll-4-4-8
zero/histogram: median 122.75 ns
zero/finish: median 83.07 ns
zero/total: median 170.39 ns
ascii/histogram: median 1117.20 ns
ascii/finish: median 63.54 ns
ascii/total: median 1175.58 ns
random/histogram: median 1040.98 ns
random/finish: median 66.32 ns
random/total: median 1105.47 ns

unroll-6-8-3
zero/histogram: median 101.17 ns
zero/finish: median 75.95 ns
zero/total: median 156.14 ns
ascii/histogram: median 1163.98 ns
ascii/finish: median 64.58 ns
ascii/total: median 1235.37 ns
random/histogram: median 1035.82 ns
random/finish: median 64.21 ns
random/total: median 1098.98 ns

unroll-8-4-2
zero/histogram: median 121.37 ns
zero/finish: median 83.22 ns
zero/total: median 167.05 ns
ascii/histogram: median 1137.31 ns
ascii/finish: median 64.06 ns
ascii/total: median 1226.25 ns
random/histogram: median 1102.05 ns
random/finish: median 65.17 ns
random/total: median 1147.67 ns

```

### target/entropy-asm-explore.py

```python
exec((__import__('pathlib').Path(__file__).resolve().parent/'entropy-explore.py').read_text().split("variants={'baseline':src}")[0])
variants={}
for banks in [4,8]:
 for schedule in ['serial','loads','loadadds']:
  body=f'    for batch in data.chunks(32768) {{\n        let mut counters = [[0u16; 256]; {banks}];\n        let (chunks, remainder) = batch.as_chunks::<{banks}>();\n        let mut ptr = chunks.as_ptr().cast::<u8>();\n        let end = unsafe {{ ptr.add(chunks.len() * {banks}) }};\n        if ptr != end {{\n        unsafe {{ std::arch::asm!(\n'
  instr=['2:',f'ldr '+('w9' if banks==4 else 'x9')+', [{ptr}], #'+str(banks)]
  for i in range(banks): instr+=[f'ubfx x{10+i}, x9, #{8*i}, #8']
  if schedule=='serial':
   for i in range(banks): instr += [f'ldrh w{18+i if i==0 else 18+i}, [{{h{i}}}, x{10+i}, lsl #1]',f'add w{18+i}, w{18+i}, #1',f'strh w{18+i}, [{{h{i}}}, x{10+i}, lsl #1]']
  else:
   for i in range(banks):
    instr += [f'ldrh w{18+i}, [{{h{i}}}, x{10+i}, lsl #1]']
    if schedule=='loadadds':instr += [f'add w{18+i}, w{18+i}, #1']
   if schedule=='loads':instr += [f'add w{18+i}, w{18+i}, #1' for i in range(banks)]
   instr += [f'strh w{18+i}, [{{h{i}}}, x{10+i}, lsl #1]' for i in range(banks)]
  # x18 reserved on Apple; use x26 instead
  instr=[x.replace('w18','w26') for x in instr]+['cmp {ptr}, {end}','b.ne 2b']
  body+=''.join('            "'+x+'",\n' for x in instr)
  body+='            ptr = inout(reg) ptr => _, end = in(reg) end,\n'
  body+=''.join(f'            h{i} = in(reg) counters[{i}].as_mut_ptr(),\n' for i in range(banks))
  body+=''.join(f'            out("x{i}") _,\n' for i in list(range(9,10+banks))+[26]+list(range(19,18+banks)))
  body+='            options(nostack),\n        ); } }\n'
  body+='        for i in 0..256 {\n            hist[i] += '+ ' + '.join(f'counters[{j}][i] as usize' for j in range(banks))+';\n        }\n        scalar_hist(remainder, hist);\n    }'
  variants[f'asm-{banks}-{schedule}']=src[:start]+body+src[end:]
for name,source in variants.items():
 source=source.replace('w19','w27').replace('"x19"','"x27"')
 (work/'mod.rs').write_text(source+cost.WRAPPERS)
 result=subprocess.run(['rustc','--edition=2021','-O',str(work/'probe.rs'),'-o',str(work/'probe')],capture_output=True,text=True)
 if result.returncode:print(name,result.stderr,flush=True);continue
 out=subprocess.check_output([str(work/'probe')],text=True)
 (work/(name+'.log')).write_text(out);(work/(name+'.rs')).write_text(source)
 print(name+'\n'+out,flush=True)
```

```text
asm-4-serial
zero/histogram: median 81.02 ns
zero/finish: median 63.60 ns
zero/total: median 150.87 ns
ascii/histogram: median 1129.16 ns
ascii/finish: median 62.30 ns
ascii/total: median 1211.89 ns
random/histogram: median 1053.12 ns
random/finish: median 64.89 ns
random/total: median 1113.61 ns

asm-4-loads
zero/histogram: median 77.55 ns
zero/finish: median 65.32 ns
zero/total: median 149.09 ns
ascii/histogram: median 1165.42 ns
ascii/finish: median 90.52 ns
ascii/total: median 1231.00 ns
random/histogram: median 1125.08 ns
random/finish: median 62.19 ns
random/total: median 1105.19 ns

asm-4-loadadds
zero/histogram: median 76.47 ns
zero/finish: median 62.92 ns
zero/total: median 152.20 ns
ascii/histogram: median 1110.50 ns
ascii/finish: median 62.11 ns
ascii/total: median 1188.70 ns
random/histogram: median 1046.93 ns
random/finish: median 62.40 ns
random/total: median 1102.47 ns

asm-8-serial
zero/histogram: median 134.89 ns
zero/finish: median 90.64 ns
zero/total: median 175.66 ns
ascii/histogram: median 1141.34 ns
ascii/finish: median 62.07 ns
ascii/total: median 1210.89 ns
random/histogram: median 1095.04 ns
random/finish: median 61.54 ns
random/total: median 1157.38 ns

asm-8-loads
zero/histogram: median 75.86 ns
zero/finish: median 62.43 ns
zero/total: median 152.40 ns
ascii/histogram: median 1166.27 ns
ascii/finish: median 64.82 ns
ascii/total: median 1228.67 ns
random/histogram: median 1076.82 ns
random/finish: median 61.78 ns
random/total: median 1135.84 ns

asm-8-loadadds
zero/histogram: median 76.61 ns
zero/finish: median 65.65 ns
zero/total: median 163.58 ns
ascii/histogram: median 1152.07 ns
ascii/finish: median 62.12 ns
ascii/total: median 1202.76 ns
random/histogram: median 1063.58 ns
random/finish: median 61.83 ns
random/total: median 1149.56 ns

```

### target/entropy-paired.py

```python
from pathlib import Path
import subprocess
root=Path(__file__).resolve().parents[1]
work=root/'target/entropy-paired';work.mkdir(exist_ok=True)
probe=r'''
#[path="mod.rs"] mod entropy;
use std::{hint::black_box, time::Instant};
fn main() {
 #[cfg(target_arch="x86_64")]
 println!("avx2={} avx512bw={}",std::is_x86_feature_detected!("avx2"),std::is_x86_feature_detected!("avx512bw"));
 for n in [64,512,4096] {
  let mut state=0x9e3779b97f4a7c15_u64;
  let random:Vec<u8>=(0..n).map(|_|{state^=state<<13;state^=state>>7;state^=state<<17;state as u8}).collect();
  let ascii:Vec<u8>=b"libcram memory tier orchestration ".iter().copied().cycle().take(n).collect();
  for (label,bytes) in [("zero",vec![0;n]),("ascii",ascii),("random",random)] {
   assert_eq!(entropy::shannon_entropy(&bytes),entropy::scalar_entropy(&bytes));
   for _ in 0..10000 {black_box(entropy::shannon_entropy(black_box(&bytes)));}
   let mut samples=Vec::new();
   for _ in 0..9 {
    let start=Instant::now();
    for _ in 0..100000 {black_box(entropy::shannon_entropy(black_box(&bytes)));}
    samples.push(start.elapsed().as_nanos() as f64 / 100000.0);
   }
   samples.sort_by(f64::total_cmp);
   println!("{label}/{n} median {:.2} ns", samples[4]);
  }
 }
}
'''
(work/'probe.rs').write_text(probe)
sources={'baseline':(root/'target/entropy-experiments/baseline.rs').read_text(),'candidate':(root/'crates/libcram-core/src/entropy/mod.rs').read_text()}
for arch in ['aarch64-apple-darwin','x86_64-apple-darwin']:
 for name,src in sources.items():
  (work/'mod.rs').write_text(src)
  binary=work/(name+'-'+arch)
  subprocess.run(['rustc','--edition=2021','-O','--target',arch,str(work/'probe.rs'),'-o',str(binary)],check=True)
  print(name+' '+arch,flush=True)
  subprocess.run([str(binary)],check=True)
```

```text
baseline aarch64-apple-darwin
zero/64 median 103.03 ns
ascii/64 median 208.56 ns
random/64 median 205.73 ns
zero/512 median 98.52 ns
ascii/512 median 311.52 ns
random/512 median 307.84 ns
zero/4096 median 151.35 ns
ascii/4096 median 1173.86 ns
random/4096 median 1137.82 ns
candidate aarch64-apple-darwin
zero/64 median 103.64 ns
ascii/64 median 164.91 ns
random/64 median 164.69 ns
zero/512 median 100.20 ns
ascii/512 median 271.21 ns
random/512 median 261.45 ns
zero/4096 median 147.99 ns
ascii/4096 median 1117.91 ns
random/4096 median 1038.15 ns
baseline x86_64-apple-darwin
Traceback (most recent call last):
  File "$WORKSPACE/libcram-entropy/target/entropy-paired.py", line 38, in <module>
    subprocess.run([str(binary)],check=True)
    ~~~~~~~~~~~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Library/Frameworks/Python.framework/Versions/3.14/lib/python3.14/subprocess.py", line 555, in run
    with Popen(*popenargs, **kwargs) as process:
         ~~~~~^^^^^^^^^^^^^^^^^^^^^^
  File "/Library/Frameworks/Python.framework/Versions/3.14/lib/python3.14/subprocess.py", line 1039, in __init__
    self._execute_child(args, executable, preexec_fn, close_fds,
    ~~~~~~~~~~~~~~~~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
                        pass_fds, cwd, env,
                        ^^^^^^^^^^^^^^^^^^^
    ...<5 lines>...
                        gid, gids, uid, umask,
                        ^^^^^^^^^^^^^^^^^^^^^^
                        start_new_session, process_group)
                        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Library/Frameworks/Python.framework/Versions/3.14/lib/python3.14/subprocess.py", line 1990, in _execute_child
    raise child_exception_type(errno_num, err_msg, err_filename)
OSError: [Errno 86] Bad CPU type in executable: '$WORKSPACE/libcram-entropy/target/entropy-paired/baseline-x86_64-apple-darwin'
```

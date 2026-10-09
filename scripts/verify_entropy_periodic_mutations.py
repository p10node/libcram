"""Negative controls for exact periodic entropy; mutate disposable copies only."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path("target/periodic-mutations"))
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    source = root / "crates/libcram-core/src/entropy"
    files = ["mod.rs", "differential.rs", "periodic.rs"]
    original = {name: (source / name).read_bytes() for name in files}
    args.output.mkdir(parents=True, exist_ok=True)
    controls = [
        ("baseline", None, None, None),
        ("partial-proof", "periodic.rs",
         "data[period..] == data[..data.len() - period]",
         "data[period..128] == data[..128 - period]"),
        ("lost-tail", "mod.rs", "data = &data[repeats * period..];", "data = &[];"),
        ("wrong-weight", "mod.rs", "hist[byte as usize] += repeats;", "hist[byte as usize] += 1;"),
        ("restored", None, None, None),
    ]
    results = []
    try:
        with tempfile.TemporaryDirectory(prefix="libcram-periodic-") as scratch:
            scratch = Path(scratch)
            for name, target, before, after in controls:
                for file, contents in original.items():
                    (scratch / file).write_bytes(contents)
                if target:
                    text = (scratch / target).read_text()
                    if text.count(before) != 1:
                        raise RuntimeError(f"non-unique mutation: {name}")
                    (scratch / target).write_text(text.replace(before, after))
                (scratch / "driver.rs").write_text('#[path="mod.rs"] mod entropy;\n')
                build = subprocess.run(
                    ["rustc", "--edition=2021", "--test", "-O", str(scratch / "driver.rs"), "-o", str(scratch / "tests")],
                    capture_output=True, timeout=60,
                )
                (args.output / f"{name}-compile.log").write_bytes(build.stdout + build.stderr)
                build.check_returncode()  # Compilation errors are not behavioral RED.
                exits = []
                for test in ["short_period_requires_complete_byte_equality", "repeated_blocks_validate_every_byte_and_preserve_additive_tails"]:
                    run = subprocess.run([str(scratch / "tests"), "--exact", f"entropy::periodic::tests::{test}" if test.startswith("short_") else f"entropy::differential::{test}"], capture_output=True, timeout=30)
                    output = run.stdout + run.stderr
                    (args.output / f"{name}-{test}.log").write_bytes(output)
                    if b"running 1 test" not in output:
                        raise RuntimeError(f"test did not run: {name}/{test}")
                    exits.append(run.returncode)
                if (not target and exits != [0, 0]) or (target and 101 not in exits):
                    raise RuntimeError(f"unexpected verdict: {name} {exits}")
                results.append({"control": name, "test_exits": exits})
    finally:
        if any((source / name).read_bytes() != data for name, data in original.items()):
            raise RuntimeError("canonical source changed during verification")
    report = {"results": results, "source_sha256": {n: hashlib.sha256(b).hexdigest() for n, b in original.items()}}
    (args.output / "receipt.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report))


if __name__ == "__main__":
    main()

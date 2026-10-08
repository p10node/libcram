"""Validate Criterion measurements; missing evidence is a failure."""
import argparse
import json
import math
from pathlib import Path


def failures(estimates, budget_ns=1000.0):
    problems = []
    for distribution in ("zero", "ascii", "random"):
        value = estimates.get(distribution)
        if value is None:
            problems.append(f"{distribution}: missing 4KB dispatch measurement")
        elif not math.isfinite(value) or value < 0 or value >= budget_ns:
            problems.append(f"{distribution}: upper mean confidence bound {value} ns is not < {budget_ns} ns")
    return problems


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("criterion_dir", type=Path, nargs="?", default=Path("target/criterion"))
    args = parser.parse_args()
    estimates = {}
    for path in args.criterion_dir.rglob("new/estimates.json"):
        benchmark = path.parent / "benchmark.json"
        if not benchmark.exists():
            continue
        metadata = json.loads(benchmark.read_text())
        if metadata.get("group_id") != "entropy" or metadata.get("value_str") != "4096":
            continue
        function = metadata.get("function_id", "")
        if not function.startswith("dispatch/"):
            continue
        distribution = function.removeprefix("dispatch/")
        value = json.loads(path.read_text())["mean"]["confidence_interval"]["upper_bound"]
        estimates[distribution] = float(value)
    problems = failures(estimates)
    print(json.dumps({"upper_mean_ns": estimates, "budget_ns": 1000.0, "failures": problems}, indent=2))
    return int(bool(problems))


if __name__ == "__main__":
    raise SystemExit(main())

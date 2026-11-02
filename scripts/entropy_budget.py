"""Validate Criterion 4 KiB entropy measurements; missing evidence is a failure.

Two operations can be gated (`--target`):

* value: `entropy/dispatch/<distribution>/4096`, the exact Shannon value.
* verdict: `entropy/verdict/<distribution>/4096`, the exact policy verdict
  `entropy < 4.5` (`entropy::entropy_below`) that the migration policy uses.

Two SLAs are evaluated from the same Criterion tree, and both are always
reported for both targets:

* strict: every distribution's upper mean confidence bound is < 1000 ns.
* calibrated: the budget is
  max(1000 ns, FLOOR_RATIO * lower mean bound of `floor/scatter_store/4096`).
  The floor is 4096 scattered u16 stores measured on the same machine in the
  same run, i.e. the memory work any exact memory-counter histogram must do.

`--target` and `--sla` select which result determines the exit status.
"""
import argparse
import json
import math
from pathlib import Path

DISTRIBUTIONS = ("zero", "ascii", "random")
STRICT_BUDGET_NS = 1000.0
FLOOR_RATIO = 1.5
FLOOR = "floor/scatter_store"
TARGETS = {"value": "dispatch/", "verdict": "verdict/"}


def _finite_positive(value):
    return value is not None and math.isfinite(value) and value > 0


def failures(estimates, budget_ns=STRICT_BUDGET_NS, label="4KB dispatch"):
    problems = []
    for distribution in DISTRIBUTIONS:
        value = estimates.get(distribution)
        if value is None:
            problems.append(f"{distribution}: missing {label} measurement")
        elif not math.isfinite(value) or value < 0 or value >= budget_ns:
            problems.append(f"{distribution}: upper mean confidence bound {value} ns is not < {budget_ns} ns")
    return problems


def calibrated_budget(floor_lower_ns):
    """Return the calibrated budget, or None when the floor is not valid evidence."""
    if not _finite_positive(floor_lower_ns):
        return None
    return max(STRICT_BUDGET_NS, FLOOR_RATIO * floor_lower_ns)


def calibrated_failures(estimates, floor_lower_ns, label="4KB dispatch"):
    budget = calibrated_budget(floor_lower_ns)
    if budget is None:
        return [f"{FLOOR}: missing or invalid 4KB floor measurement ({floor_lower_ns})"]
    return failures(estimates, budget, label)


def read_criterion(criterion_dir):
    """Return ({target: {distribution: upper mean ns}}, floor lower mean ns)."""
    estimates = {target: {} for target in TARGETS}
    floor_lower = None
    for path in criterion_dir.rglob("new/estimates.json"):
        benchmark = path.parent / "benchmark.json"
        if not benchmark.exists():
            continue
        metadata = json.loads(benchmark.read_text())
        if metadata.get("group_id") != "entropy" or metadata.get("value_str") != "4096":
            continue
        function = metadata.get("function_id", "")
        interval = json.loads(path.read_text())["mean"]["confidence_interval"]
        if function == FLOOR:
            floor_lower = float(interval["lower_bound"])
            continue
        for target, prefix in TARGETS.items():
            if function.startswith(prefix):
                estimates[target][function.removeprefix(prefix)] = float(interval["upper_bound"])
    return {target: dict(sorted(values.items())) for target, values in estimates.items()}, floor_lower


def report(estimates, floor_lower, label):
    return {
        "strict": {"budget_ns": STRICT_BUDGET_NS, "failures": failures(estimates, label=label)},
        "calibrated": {"floor_ratio": FLOOR_RATIO, "budget_ns": calibrated_budget(floor_lower),
                       "ratio_to_floor": {k: v / floor_lower for k, v in estimates.items()}
                       if _finite_positive(floor_lower) else None,
                       "failures": calibrated_failures(estimates, floor_lower, label)},
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("criterion_dir", type=Path, nargs="?", default=Path("target/criterion"))
    parser.add_argument("--sla", choices=("calibrated", "strict"), default="calibrated",
                        help="which SLA determines the exit status (both are reported)")
    parser.add_argument("--target", choices=tuple(TARGETS), default="value",
                        help="which operation determines the exit status (value is always reported)")
    args = parser.parse_args()
    estimates, floor_lower = read_criterion(args.criterion_dir)
    value = report(estimates["value"], floor_lower, "4KB dispatch")
    output = {
        "upper_mean_ns": estimates["value"],
        "floor_scatter_store_lower_ns": floor_lower,
        "sla": args.sla,
        **value,
    }
    if args.target == "verdict" or estimates["verdict"]:
        output["target"] = args.target
        output["verdict"] = {"upper_mean_ns": estimates["verdict"],
                             **report(estimates["verdict"], floor_lower, "4KB verdict")}
    selected = output if args.target == "value" else output["verdict"]
    output["failures"] = selected[args.sla]["failures"]
    print(json.dumps(output, indent=2))
    return int(bool(output["failures"]))


if __name__ == "__main__":
    raise SystemExit(main())

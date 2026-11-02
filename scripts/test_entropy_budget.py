import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from entropy_budget import calibrated_budget, calibrated_failures, failures

SCRIPT = str(Path(__file__).with_name("entropy_budget.py"))


def write_fixture(root, function_id, interval):
    folder = root / function_id.replace("/", "_") / "new"
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "benchmark.json").write_text(json.dumps({
        "group_id": "entropy", "value_str": "4096", "function_id": function_id}))
    (folder / "estimates.json").write_text(json.dumps({
        "mean": {"confidence_interval": interval}}))


class BudgetTests(unittest.TestCase):
    def test_missing_measurements_fail_closed(self):
        self.assertTrue(failures({}))
        self.assertTrue(calibrated_failures({}, 800.0))

    def test_upper_interval_not_only_mean_must_fit(self):
        good = {key: 900.0 for key in ("zero", "ascii", "random")}
        self.assertEqual(failures(good), [])
        for invalid in (1000.0, 1200.0, float("nan"), float("inf"), -1):
            with self.subTest(value=invalid):
                self.assertTrue(failures(dict(good, random=invalid)))

    def test_calibrated_budget_scales_with_floor_but_never_below_strict(self):
        self.assertEqual(calibrated_budget(100.0), 1000.0)
        self.assertEqual(calibrated_budget(800.0), 1200.0)
        self.assertEqual(calibrated_budget(2000.0), 3000.0)

    def test_calibrated_floor_must_be_valid_evidence(self):
        good = {key: 900.0 for key in ("zero", "ascii", "random")}
        for floor in (None, 0.0, -5.0, float("nan"), float("inf")):
            with self.subTest(floor=floor):
                self.assertIsNone(calibrated_budget(floor))
                self.assertTrue(calibrated_failures(good, floor))

    def test_calibrated_budget_is_an_exclusive_upper_bound(self):
        measured = {"zero": 150.0, "ascii": 250.0, "random": 1100.0}
        self.assertEqual(calibrated_failures(measured, 820.0), [])
        self.assertTrue(failures(measured))
        self.assertTrue(calibrated_failures(dict(measured, random=1230.0), 820.0))
        self.assertTrue(calibrated_failures(dict(measured, random=float("nan")), 820.0))

    def test_cli_reads_owned_criterion_fixtures(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for distribution in ("zero", "ascii", "random"):
                write_fixture(root, f"dispatch/{distribution}",
                              {"lower_bound": 700, "upper_bound": 800})
            strict = [sys.executable, SCRIPT, "--sla", "strict", directory]
            calibrated = [sys.executable, SCRIPT, directory]
            success = subprocess.run(strict, capture_output=True, text=True)
            self.assertEqual(success.returncode, 0, success.stderr)
            self.assertEqual(json.loads(success.stdout)["upper_mean_ns"]["random"], 800)
            missing_floor = subprocess.run(calibrated, capture_output=True, text=True)
            self.assertEqual(missing_floor.returncode, 1)
            self.assertIn("floor", json.loads(missing_floor.stdout)["failures"][0])

            write_fixture(root, "floor/scatter_store", {"lower_bound": 820, "upper_bound": 900})
            write_fixture(root, "dispatch/random", {"lower_bound": 1050, "upper_bound": 1100})
            report = subprocess.run(calibrated, capture_output=True, text=True)
            self.assertEqual(report.returncode, 0, report.stdout)
            output = json.loads(report.stdout)
            self.assertEqual(output["floor_scatter_store_lower_ns"], 820)
            self.assertEqual(output["calibrated"]["budget_ns"], 1230)
            self.assertIn("random", output["strict"]["failures"][0])
            self.assertEqual(subprocess.run(strict, capture_output=True).returncode, 1)

            write_fixture(root, "dispatch/random", {"lower_bound": 1200, "upper_bound": 1400})
            failure = subprocess.run(calibrated, capture_output=True, text=True)
            self.assertEqual(failure.returncode, 1)
            self.assertIn("random", json.loads(failure.stdout)["failures"][0])
            self.assertNotIn("verdict", json.loads(failure.stdout))

    def test_verdict_target_is_gated_separately_and_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for distribution in ("zero", "ascii", "random"):
                write_fixture(root, f"dispatch/{distribution}",
                              {"lower_bound": 2800, "upper_bound": 2900})
            write_fixture(root, "floor/scatter_store", {"lower_bound": 1700, "upper_bound": 1750})
            verdict = [sys.executable, SCRIPT, "--target", "verdict", directory]
            missing = subprocess.run(verdict, capture_output=True, text=True)
            self.assertEqual(missing.returncode, 1)
            self.assertIn("verdict measurement", json.loads(missing.stdout)["failures"][0])

            for distribution, upper in (("zero", 250), ("ascii", 290), ("random", 1580)):
                write_fixture(root, f"verdict/{distribution}",
                              {"lower_bound": upper - 10, "upper_bound": upper})
            passed = subprocess.run(verdict, capture_output=True, text=True)
            self.assertEqual(passed.returncode, 0, passed.stdout)
            output = json.loads(passed.stdout)
            self.assertEqual(output["target"], "verdict")
            self.assertEqual(output["verdict"]["upper_mean_ns"]["random"], 1580)
            self.assertEqual(output["verdict"]["calibrated"]["budget_ns"], 2550)
            self.assertIn("random", output["verdict"]["strict"]["failures"][0])
            # The value target still fails on the same tree and is still reported.
            self.assertTrue(output["calibrated"]["failures"])
            self.assertEqual(subprocess.run([*verdict[:2], "--target", "verdict", "--sla", "strict",
                                             directory], capture_output=True).returncode, 1)
            self.assertEqual(subprocess.run([sys.executable, SCRIPT, directory],
                                            capture_output=True).returncode, 1)


if __name__ == "__main__":
    unittest.main()

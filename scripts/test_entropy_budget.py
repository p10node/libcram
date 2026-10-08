import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from entropy_budget import failures

class BudgetTests(unittest.TestCase):
    def test_missing_measurements_fail_closed(self):
        self.assertTrue(failures({}))

    def test_upper_interval_not_only_mean_must_fit(self):
        good = {key: 900.0 for key in ("zero", "ascii", "random")}
        self.assertEqual(failures(good), [])
        for invalid in (1000.0, 1200.0, float("nan"), float("inf"), -1):
            with self.subTest(value=invalid):
                self.assertTrue(failures(dict(good, random=invalid)))

    def test_cli_reads_owned_criterion_fixtures(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for distribution in ("zero", "ascii", "random"):
                folder = root / distribution / "new"
                folder.mkdir(parents=True)
                (folder / "benchmark.json").write_text(json.dumps({
                    "group_id": "entropy", "value_str": "4096",
                    "function_id": f"dispatch/{distribution}"}))
                (folder / "estimates.json").write_text(json.dumps({
                    "mean": {"confidence_interval": {"upper_bound": 800}}}))
            command = [sys.executable, str(Path(__file__).with_name("entropy_budget.py")), directory]
            success = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(success.returncode, 0, success.stderr)
            self.assertEqual(json.loads(success.stdout)["upper_mean_ns"]["random"], 800)
            (root / "random/new/estimates.json").write_text(json.dumps({
                "mean": {"confidence_interval": {"upper_bound": 1400}}}))
            failure = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(failure.returncode, 1)
            self.assertIn("random", json.loads(failure.stdout)["failures"][0])

if __name__ == "__main__":
    unittest.main()

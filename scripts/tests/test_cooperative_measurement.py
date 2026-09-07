"""Regression coverage for measurement reports emitted by Rust libtest."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location(
    "measurement", Path(__file__).parents[1] / "measure-cooperative-workflows.py"
)
measurement = importlib.util.module_from_spec(spec)
spec.loader.exec_module(measurement)


class ReportParsing(unittest.TestCase):
    def test_accepts_libtest_prefix_and_standalone_report(self):
        for prefix in ("", "test cooperative_measurement::cooperative_revision_measurement ... "):
            log = 'running 1 test\n' + prefix + measurement.PREFIX + '{"prepared_samples":1000}\nok\n'
            self.assertEqual(measurement.parse_measurement(log), {"prepared_samples": 1000})

    def test_rejects_missing_or_duplicate_reports(self):
        for log in ("test failed", (measurement.PREFIX + '{}\n') * 2):
            with self.assertRaises(RuntimeError):
                measurement.parse_measurement(log)


if __name__ == '__main__':
    unittest.main()

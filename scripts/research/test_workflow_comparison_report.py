"""Synthetic report fixtures; these are validator tests, never measurements."""
import copy
import unittest

import workflow_comparison_report as report


def metric(scale=1):
    return {"samples_us": [1.0 * scale, 2.0 * scale, 3.0 * scale],
            "summary": {"samples": 3, "min_us": 1.0 * scale, "max_us": 3.0 * scale,
                        "p50_us": 2.0 * scale, "p95_us": 3.0 * scale}}


def fixture():
    run = dict(source_revision="synthetic", benchmark_executable_sha256="synthetic", format_version=1, quantile_method="ceil((n-1)*p)", profile="release",
               os="test-os", arch="test-arch", wasmtime="test-version", workers=4,
               epoch_tick_ms=100, disk_cache=False, preparation_method="synthetic",
               runtime="synthetic", child_limits={}, store_limits={}, warmups=5,
               smoke=False, dependency_hashes=[], missing_metrics=[],
               first_backend="legacy", reports=[])
    for name, count in report.COUNTS.items():
        for backend in report.BACKENDS:
            isolated = backend != "legacy" and count > 0
            run["reports"].append(dict(
                name=name, backend=backend, graph={"synthetic": name}, child_graph=None,
                graph_sha256="fixture", input_sha256="fixture", input_bytes=2,
                random_values=count, track_events=False,
                sizes={"workflow_wasm_bytes": 100, "workflow_wasm_gzip_bytes": 50,
                       "serialized_native_package_bytes": 500, "root_component_bytes": 60 if isolated else 100,
                       "unique_child_bytes": 32 if isolated else 0,
                       "package_index_and_framing_bytes": 8 if isolated else 0,
                       "unique_children": int(isolated), "bindings": int(isolated)},
                isolation={"agent_boundary": isolated, "embed_boundary": False,
                           "verified_starts_per_instrumented_run": count if isolated else 0,
                           "verified_replay_starts": 0 if name in report.REPLAY else None,
                           "control_reason": "control" if not count else None,
                           "context_contract": "live-adapter-call:1" if isolated else "none"},
                checkpoint_count=1 if name in report.REPLAY else 0,
                metrics={key: None if key == "durable_replay" and name not in report.REPLAY
                         else metric(1 if backend == "legacy" else 2) for key in report.METRICS}))
    return dict(date="test-date", source_revision="synthetic", machine="synthetic", runs=[run])


class ComparisonTests(unittest.TestCase):
    def test_valid_report_renders_absolute_and_percentage_values(self):
        text = report.render(fixture())
        self.assertIn("100.00%", text)
        self.assertIn("Agent-only service and parent-step spans remain pending", text)
        self.assertIn("not production qualification", text)

    def test_alternating_order_is_allowed_but_changed_configuration_is_rejected(self):
        data = fixture()
        second = copy.deepcopy(data["runs"][0])
        second["first_backend"] = report.BACKENDS[1]
        second["reports"] = [row for i in range(0, len(second["reports"]), 2)
                             for row in reversed(second["reports"][i:i+2])]
        data["runs"].append(second)
        report.validate(data)
        second["workers"] = 8
        with self.assertRaisesRegex(ValueError, "configuration changed"):
            report.validate(data)

    def test_changed_workloads_and_duplicate_rows_are_rejected(self):
        data = fixture()
        data["runs"][0]["reports"][1]["input_sha256"] = "changed"
        with self.assertRaisesRegex(ValueError, "workload changed"):
            report.validate(data)
        data = fixture()
        data["runs"][0]["reports"].append(data["runs"][0]["reports"][0])
        with self.assertRaisesRegex(ValueError, "duplicate"):
            report.validate(data)

    def test_false_isolation_and_partial_package_sizes_are_rejected(self):
        for mutate, error in [
            (lambda row: row["isolation"].update(verified_starts_per_instrumented_run=0), "actual isolated-call"),
            (lambda row: row["isolation"].update(embed_boundary=True), "isolation claim"),
            (lambda row: row["sizes"].update(root_component_bytes=1), "package size"),
            (lambda row: row["sizes"].update(unique_children=100), "deduplication"),
        ]:
            with self.subTest(error=error):
                data = fixture()
                mutate(data["runs"][0]["reports"][1])
                with self.assertRaisesRegex(ValueError, error):
                    report.validate(data)

    def test_incorrect_summaries_and_invalid_samples_are_rejected(self):
        for value in [float("nan"), -1, True]:
            with self.subTest(value=value):
                data = fixture()
                data["runs"][0]["reports"][0]["metrics"]["worker_precompile"]["samples_us"][0] = value
                with self.assertRaisesRegex(ValueError, "invalid elapsed-time"):
                    report.validate(data)
        data = fixture()
        data["runs"][0]["reports"][0]["metrics"]["prepared_full_run"]["summary"]["p50_us"] = 99
        with self.assertRaisesRegex(ValueError, "summary disagrees"):
            report.validate(data)

    def test_debug_cache_and_smoke_reports_cannot_be_presented_as_measurements(self):
        for key, value in [("profile", "debug"), ("disk_cache", True), ("smoke", True)]:
            data = fixture()
            data["runs"][0][key] = value
            with self.assertRaisesRegex(ValueError, "measurement requires"):
                report.validate(data)

    def test_replay_requires_zero_new_invocations(self):
        data = fixture()
        data["runs"][0]["reports"][1]["isolation"]["verified_replay_starts"] = 1
        with self.assertRaisesRegex(ValueError, "replay lacks evidence"):
            report.validate(data)

    def test_mismatched_source_revision_and_backend_order_are_rejected(self):
        data = fixture()
        data["source_revision"] = "wrong"
        with self.assertRaisesRegex(ValueError, "source revision"):
            report.validate(data)
        data = fixture()
        data["runs"][0]["first_backend"] = report.BACKENDS[1]
        with self.assertRaisesRegex(ValueError, "backend order"):
            report.validate(data)

    def test_zero_baseline_has_no_percentage_delta(self):
        self.assertEqual(report.delta([0], [10]), ("10.000", "N/A"))
        self.assertEqual(report.delta([2], [3]), ("1.000", "50.00%"))


if __name__ == "__main__":
    unittest.main()

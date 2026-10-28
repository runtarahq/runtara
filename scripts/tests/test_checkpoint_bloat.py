"""Coverage for the checkpoint bloat harness's workload and report shaping."""
import importlib.util
from pathlib import Path
import unittest
import zlib

spec = importlib.util.spec_from_file_location(
    "bloat", Path(__file__).parents[1] / "measure-checkpoint-bloat.py"
)
bloat = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bloat)


class Workload(unittest.TestCase):
    def test_graph_is_agent_split_agent_with_one_agent_per_item(self):
        graph = bloat.build_graph()
        steps = graph["steps"]
        self.assertEqual(graph["entryPoint"], "a")
        self.assertEqual(
            [(e["fromStep"], e["toStep"]) for e in graph["executionPlan"]],
            [("a", "split"), ("split", "b"), ("b", "finish")],
        )
        for step in ("a", "b"):
            self.assertEqual(steps[step]["stepType"], "Agent")
        self.assertEqual(steps["split"]["config"]["value"], bloat.ref("data.items"))
        body = steps["split"]["subgraph"]["steps"]
        self.assertEqual(body["item"]["stepType"], "Agent")
        self.assertEqual(body["item"]["capabilityId"], "return-input")

    def test_items_carry_incompressible_padding_of_the_requested_size(self):
        items = bloat.build_items(3, 3000, seed=1)
        self.assertEqual([i["i"] for i in items], [0, 1, 2])
        pad = items[0]["pad"].encode()
        self.assertAlmostEqual(len(pad), 3000, delta=4)
        # Random bytes, base64-encoded: compression saves at most a quarter.
        self.assertGreater(len(zlib.compress(pad)), len(pad) * 0.7)
        self.assertEqual(items, bloat.build_items(3, 3000, seed=1))
        self.assertEqual(bloat.build_items(2, 0, seed=1), [{"i": 0, "pad": ""}, {"i": 1, "pad": ""}])


class Parameters(unittest.TestCase):
    def test_parses_heap_and_toast_parameters(self):
        self.assertEqual(
            bloat.parse_parameters("autovacuum_vacuum_scale_factor=0.02, toast.autovacuum_enabled=false"),
            {"autovacuum_vacuum_scale_factor": "0.02", "toast.autovacuum_enabled": "false"},
        )
        self.assertEqual(bloat.parse_parameters(""), {})

    def test_rejects_parameters_that_are_not_reset_between_modes(self):
        for text in ("fillfactor=90", "autovacuum_vacuum_scale_factor", "autovacuum_enabled=false); DROP TABLE x; --"):
            with self.assertRaises(ValueError):
                bloat.parse_parameters(text)

    def test_mode_parameters(self):
        tuned = {"autovacuum_vacuum_scale_factor": "0.02"}
        self.assertEqual(bloat.mode_parameters("seeded", tuned), {})
        self.assertEqual(bloat.mode_parameters("tuned", tuned), tuned)
        self.assertEqual(
            bloat.mode_parameters("paused", tuned),
            {"autovacuum_enabled": "false", "toast.autovacuum_enabled": "false"},
        )


class Summary(unittest.TestCase):
    @staticmethod
    def stats(dead, total, autovacuums=0):
        # A quarter heap, a quarter indexes, half TOAST.
        return {"n_dead_tup": dead, "n_live_tup": 10, "total_bytes": total, "autovacuum_count": autovacuums,
                "toast_autovacuum_count": None, "heap_bytes": total // 4, "index_bytes": total // 4,
                "toast_bytes": total // 2}

    def test_summarizes_runs_samples_and_wal(self):
        mib = 1048576
        wal = {
            "total_bytes": 4 * mib,
            "checkpoints_bytes": 2 * mib,
            "breakdown": [{"kind": "heap", "record": "Heap/DELETE", "bytes": mib}],
            "by_relation": [{"relation": "checkpoints", "bytes": 2 * mib}],
        }
        runs = [
            {"run": 0, "status": "completed", "duration_s": 2.0, "after": self.stats(100, 10 * mib, 0), "wal": wal},
            {"run": 1, "status": "failed", "duration_s": 4.0, "after": self.stats(50, 11 * mib, 1), "wal": wal},
        ]
        summary = bloat.summarize(self.stats(0, 8 * mib), runs, [self.stats(300, 0)], self.stats(0, 11 * mib, 2))
        self.assertEqual(summary["failed_runs"], [1])
        self.assertEqual(summary["mean_duration_s"], 3.0)
        self.assertEqual(summary["autovacuums"], 2)
        self.assertEqual(summary["toast_autovacuums"], 0)
        self.assertEqual(summary["peak_dead_tuples"], 300)
        self.assertEqual(summary["dead_tuples_end_of_runs"], 50)
        self.assertEqual(summary["total_mib_after_each_run"], [10.0, 11.0])
        self.assertEqual(summary["growth_last_10_runs_pct"], 10.0)
        self.assertEqual(summary["growth_last_10_runs_mib"], {"heap": 0.25, "indexes": 0.25, "toast": 0.5})
        self.assertEqual(summary["wal_mib_total"], 8.0)
        self.assertEqual(summary["wal_mib_per_run"], 4.0)
        self.assertEqual(summary["wal_mib_checkpoints_by_record"], {"heap Heap/DELETE": 2.0})
        self.assertEqual(summary["wal_mib_by_relation"], {"checkpoints": 4.0})


if __name__ == "__main__":
    unittest.main()

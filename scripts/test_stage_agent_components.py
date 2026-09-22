"""Regression tests for release staging from reused Cargo output directories."""

import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("stage_components", Path(__file__).with_name("stage-agent-components.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class ComponentStagingTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "Cargo.toml").write_text('[workspace]\nresolver = "3"\nmembers = ["crates/agents/runtara-agent-http"]\n')
        agent = self.root / "crates/agents/runtara-agent-http"
        (agent / "src").mkdir(parents=True)
        (agent / "Cargo.toml").write_text('[package]\nname = "runtara-agent-http"\nversion = "0.0.0"\nedition = "2024"\n')
        (agent / "src/lib.rs").write_text("")
        self.source = self.root / "source"
        self.destination = self.root / "staged"
        self.source.mkdir()
        for name in module.component_names(self.root) + ["runtara_agent_sftp"]:
            for extension in ("wasm", "meta.json"):
                (self.source / f"{name}.{extension}").write_text(name)

    def test_stale_removed_agent_is_excluded_and_source_is_preserved(self):
        module.stage(self.root, self.source, self.destination)
        self.assertEqual(len(list(self.destination.iterdir())), 6)
        self.assertFalse((self.destination / "runtara_agent_sftp.wasm").exists())
        self.assertTrue((self.source / "runtara_agent_sftp.wasm").exists())

    def test_missing_sidecar_fails_before_any_copy(self):
        (self.source / "runtara_agent_http.meta.json").unlink()
        with self.assertRaisesRegex(ValueError, "Missing component artifacts"):
            module.stage(self.root, self.source, self.destination)
        self.assertFalse(self.destination.exists())

    def test_existing_destination_cannot_reintroduce_removed_agent(self):
        self.destination.mkdir()
        stale = self.destination / "runtara_agent_sftp.wasm"
        stale.write_text("old")
        with self.assertRaisesRegex(ValueError, "fresh staging directory"):
            module.stage(self.root, self.source, self.destination)
        self.assertEqual(stale.read_text(), "old")


if __name__ == "__main__":
    unittest.main()

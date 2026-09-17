#!/usr/bin/env python3
"""Stage only workspace-declared components, excluding stale Cargo outputs."""

import argparse
from pathlib import Path
import json
import shutil
import subprocess


def component_names(workspace: Path) -> list[str]:
    # Cargo resolves workspace membership; this also works with Python 3.9/3.10
    # shipped by the release builders, without requiring a TOML dependency.
    metadata = json.loads(subprocess.check_output([
        "cargo", "metadata", "--no-deps", "--format-version", "1",
        "--manifest-path", str(workspace / "Cargo.toml"),
    ], text=True))
    members = set(metadata["workspace_members"])
    agents_dir = (workspace / "crates" / "agents").resolve()
    agents = [
        package["name"].replace("-", "_")
        for package in metadata["packages"]
        if package["id"] in members
        and Path(package["manifest_path"]).resolve().parent.parent == agents_dir
        and package["name"].startswith("runtara-agent-")
    ]
    if not agents:
        raise ValueError("workspace declares no agent components")
    return agents + ["runtara_workflow_stdlib", "runtara_workflow_runtime"]


def stage(workspace: Path, source: Path, destination: Path) -> None:
    expected = {
        f"{name}.{extension}"
        for name in component_names(workspace)
        for extension in ("wasm", "meta.json")
    }
    missing = sorted(name for name in expected if not (source / name).is_file())
    if missing:
        raise ValueError(f"Missing component artifacts: {', '.join(missing)}")
    # Refuse to mix releases or remove someone else's files. Callers must use a
    # fresh staging directory when the declared component set changes.
    if destination.exists():
        unexpected = sorted(p.name for p in destination.iterdir() if p.name not in expected)
        if unexpected:
            raise ValueError(f"Use a fresh staging directory; unexpected artifacts: {', '.join(unexpected)}")
    destination.mkdir(parents=True, exist_ok=True)
    for name in sorted(expected):
        shutil.copy2(source / name, destination / name)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    try:
        stage(Path(__file__).resolve().parent.parent, args.source, args.destination)
    except ValueError as error:
        parser.exit(1, f"{error}\n")

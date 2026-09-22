#!/usr/bin/env python3
"""Measure two worktrees with identical test code and separate Cargo outputs.

Only the baseline test module and its registration are overlaid. Production
sources, Cargo configuration and historical benchmark reports are unchanged.
Builds finish before the three alternating measurement pairs start.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import difflib
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import time
import tomllib

HARNESS = Path("crates/runtara-workflows/tests/cooperative_measurement/mod.rs")
PARENT = Path("crates/runtara-workflows/tests/direct_wasm_execute.rs")
PREFIX = "COOPERATIVE_MEASUREMENT_JSON="


def digest(data):
    return hashlib.sha256(data).hexdigest()


def command(args, cwd, env=None):
    return subprocess.check_output(args, cwd=cwd, env=env, text=True, stderr=subprocess.STDOUT).strip()


def source_manifest(root):
    lock = (root / "Cargo.lock").read_bytes()
    packages = tomllib.loads(lock.decode())["package"]
    return {
        "root": str(root),
        "revision": command(["git", "rev-parse", "HEAD"], root),
        "cargo_lock_sha256": digest(lock),
        "stack": {name: sorted({p["version"] for p in packages if p["name"] == name})
                  for name in ["wasmtime", "wit-bindgen", "wasm-encoder", "wasmparser", "wac-graph"]},
        "harness_sha256": digest((root / HARNESS).read_bytes()),
        "parent_test_sha256": digest((root / PARENT).read_bytes()),
        "changes": command(["git", "status", "--porcelain"], root).splitlines(),
    }


def host_source(root):
    text = (root / PARENT).read_text()
    return text[text.index("struct CapturingRuntimeHost {"):text.index("/// CLI path: spawn")]


def overlay(baseline, candidate):
    original = subprocess.check_output(["git", "show", f"HEAD:{PARENT}"], cwd=baseline)
    expected = original + b"\nmod cooperative_measurement;\n"
    existing = (baseline / PARENT).read_bytes()
    if existing not in (original, expected):
        raise RuntimeError("baseline test file has unrelated edits")
    shared = (candidate / HARNESS).read_bytes()
    if (baseline / HARNESS).exists() and (baseline / HARNESS).read_bytes() != shared:
        raise RuntimeError("baseline has a different measurement overlay; preserve/review it first")
    changed = set(command(["git", "diff", "--name-only", "HEAD"], baseline).splitlines())
    changed |= set(command(["git", "ls-files", "--others", "--exclude-standard"], baseline).splitlines())
    if changed - {str(HARNESS), str(PARENT)}:
        raise RuntimeError("baseline has unrelated working-tree changes")
    (baseline / HARNESS).parent.mkdir(parents=True, exist_ok=True)
    (baseline / HARNESS).write_bytes(shared)
    (baseline / PARENT).write_bytes(expected)


def environment(target):
    env = os.environ.copy()
    # These are measurement inputs, not product configuration alternatives.
    for name in list(env):
        if name in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS") or name.startswith("CARGO_PROFILE_"):
            if env[name]:
                raise RuntimeError(f"remove compiler override {name} for this measurement")
    for name in ("RUNTARA_DIRECT_OMIT_RUNTIME", "RUNTARA_DIRECT_RUNTIME_BINDING", "RUNTARA_DIRECT_WORKFLOW_ABI"):
        env.pop(name, None)
    env.update(RUSTC_WRAPPER="", SQLX_OFFLINE="true", CARGO_BUILD_JOBS="4",
               CARGO_TARGET_DIR=str(target),
               RUNTARA_AGENT_COMPONENTS_DIR=str(target / "wasm32-wasip2/release"))
    return env


def build(root, target, label, output):
    env = environment(target)
    print(f"Building {label} normal components", flush=True)
    with (output / f"{label}-components.log").open("w") as log:
        subprocess.run(["scripts/build-agent-components.sh"], cwd=root, env=env,
                       stdout=log, stderr=subprocess.STDOUT, check=True)
    print(f"Building {label} release measurement executable", flush=True)
    args = ["cargo", "test", "--release", "-p", "runtara-workflows", "--features",
            "direct-wasm-integration-tests", "--test", "direct_wasm_execute", "--no-run", "--message-format=json"]
    with (output / f"{label}-build.jsonl").open("w") as log, (output / f"{label}-build.log").open("w") as err:
        subprocess.run(args, cwd=root, env=env, stdout=log, stderr=err, check=True)
    binaries = []
    for line in (output / f"{label}-build.jsonl").read_text().splitlines():
        item = json.loads(line)
        if item.get("reason") == "compiler-artifact" and item.get("executable") and item["target"]["name"] == "direct_wasm_execute":
            binaries.append(Path(item["executable"]))
    if len(binaries) != 1:
        raise RuntimeError(f"expected one {label} executable: {binaries}")
    return binaries[0]


def component_inputs(target):
    directory = target / "wasm32-wasip2/release"
    names = [f"{name}.{suffix}" for name in ("runtara_agent_utils", "runtara_workflow_stdlib", "runtara_workflow_runtime") for suffix in ("wasm", "meta.json")]
    return {name: digest((directory / name).read_bytes()) for name in names}


def hardware(root):
    result = {"logical_cpus": os.cpu_count(), "cpu_model": platform.processor(), "physical_cpus": None, "memory_bytes": None}
    if platform.system() == "Darwin":
        values = command(["sysctl", "-n", "machdep.cpu.brand_string", "hw.memsize", "hw.physicalcpu"], root).splitlines()
        result.update(cpu_model=values[0], memory_bytes=int(values[1]), physical_cpus=int(values[2]))
    elif platform.system() == "Linux":
        result["memory_bytes"] = os.sysconf("SC_PHYS_PAGES") * os.sysconf("SC_PAGE_SIZE")
    return result


def parse_measurement(text):
    # libtest can print its test-name prefix on the same line as println!.
    reports = [line.split(PREFIX, 1)[1] for line in text.splitlines() if PREFIX in line]
    if len(reports) != 1:
        raise RuntimeError("expected exactly one measurement report")
    return json.loads(reports[0])


def run(binary, root, target, label, pair, output, expected_source, expected_inputs):
    if source_manifest(root) != expected_source:
        raise RuntimeError(f"{label} source changed after building")
    if component_inputs(target) != expected_inputs:
        raise RuntimeError(f"{label} component inputs changed after building")
    log_path = output / f"pair-{pair}-{label}.log"
    print(f"Measuring pair {pair}: {label}", flush=True)
    started = time.time()
    before = os.getloadavg()
    args = [str(binary), "cooperative_measurement::cooperative_revision_measurement",
            "--exact", "--ignored", "--nocapture", "--test-threads=1"]
    env = environment(target)
    env["RUNTARA_MEASUREMENT_ARTIFACTS"] = str(output / f"pair-{pair}-{label}-artifacts")
    with log_path.open("w") as log:
        result = subprocess.run(args, cwd=root, env=env, stdout=log, stderr=subprocess.STDOUT)
    outcome = {"pair": pair, "revision_label": label, "started_unix": started,
               "elapsed_seconds": time.time() - started, "load_before": before,
               "load_after": os.getloadavg(), "exit_code": result.returncode,
               "log_sha256": digest(log_path.read_bytes())}
    (output / f"pair-{pair}-{label}-outcome.json").write_text(json.dumps(outcome, indent=2) + "\n")
    if result.returncode:
        raise RuntimeError(f"{label} measurement failed; see {log_path}")
    report = parse_measurement(log_path.read_text())
    assert report["prepared_samples"] == 1000 and report["warmup_runs"] >= 5
    assert len(report["reports"]) == 14
    for case in report["reports"]:
        assert case["cached_full_run"]["samples"] == 1000
        assert len(case["cached_full_run"]["raw_us"]) == 1000
        assert not case["sizes"]["omit_runtime"]
    if source_manifest(root) != expected_source or component_inputs(target) != expected_inputs:
        raise RuntimeError(f"{label} inputs changed during measurement")
    outcome["measurement"] = report
    (output / f"pair-{pair}-{label}.json").write_text(json.dumps(outcome, indent=2) + "\n")
    return outcome


def validate_pairs(runs):
    for pair in range(1, 4):
        data = {r["revision_label"]: r["measurement"] for r in runs if r["pair"] == pair}
        old, new = data["baseline"], data["candidate"]
        for a, b in zip(old["reports"], new["reports"], strict=True):
            for key in ("name", "graph_sha256", "input_sha256", "track_events", "random_values"):
                assert a[key] == b[key], (pair, key)
            # A normal composed candidate must contain its generated cancellation
            # path for Agent workflows. No isolated task-service import is used.
            if b["random_values"]:
                assert b["sizes"]["abi"]["subtask_cancel"] > 0, b["name"]
                assert b["sizes"]["abi"]["waitable_set_wait"] > 0, b["name"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("baseline", "candidate", "baseline-target", "candidate-target", "output"):
        parser.add_argument(f"--{name}", required=True, type=Path)
    args = parser.parse_args()
    roots = {label: getattr(args, label).resolve() for label in ("baseline", "candidate")}
    targets = {label: getattr(args, f"{label}_target").resolve() for label in roots}
    if roots["baseline"] == roots["candidate"] or targets["baseline"] == targets["candidate"]:
        raise RuntimeError("source revisions and build targets must be separate")
    if any(a in b.parents for a, b in [(targets["baseline"], targets["candidate"]), (targets["candidate"], targets["baseline"])]):
        raise RuntimeError("build targets must not overlap")
    if command(["git", "status", "--porcelain"], roots["candidate"]):
        raise RuntimeError("commit or preserve candidate edits before measuring an exact revision")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    overlay(roots["baseline"], roots["candidate"])
    sources = {label: source_manifest(root) for label, root in roots.items()}
    assert sources["baseline"]["stack"]["wasmtime"] == sources["candidate"]["stack"]["wasmtime"]
    assert sources["baseline"]["harness_sha256"] == sources["candidate"]["harness_sha256"]
    diff = "".join(difflib.unified_diff(host_source(roots["baseline"]).splitlines(True), host_source(roots["candidate"]).splitlines(True), fromfile="baseline-host", tofile="candidate-host"))
    (output / "fixture-host.diff").write_text(diff)
    manifest = {"format_version": 1, "status": "building", "sources": sources,
                "targets": {k: str(v) for k, v in targets.items()}, "driver_sha256": digest(Path(__file__).read_bytes()),
                "fixture_host_diff": diff, "platform": platform.platform(), "machine": platform.machine(),
                "hardware": hardware(roots["candidate"]), "gzip": command(["gzip", "--version"], roots["candidate"]), "gzip_arguments": ["-n", "-c"], "rustc": command(["rustc", "-Vv"], roots["candidate"]),
                "cargo": command(["cargo", "-V"], roots["candidate"]),
                "orders": [["baseline", "candidate"], ["candidate", "baseline"], ["baseline", "candidate"]],
                "scope": "interim no-cancellation whole-revision normal-path comparison, including runtime changes; final plan gates remain pending"}
    manifest_path = output / "manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
    try:
        with ThreadPoolExecutor(max_workers=2) as pool:
            futures = {label: pool.submit(build, roots[label], targets[label], label, output) for label in roots}
            binaries = {label: future.result() for label, future in futures.items()}
        manifest["executables"] = {label: {"path": str(path), "sha256": digest(path.read_bytes())} for label, path in binaries.items()}
        manifest["component_inputs"] = {label: component_inputs(target) for label, target in targets.items()}
        manifest["status"] = "measuring"
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
        runs = []
        for pair, order in enumerate(manifest["orders"], 1):
            for label in order:
                runs.append(run(binaries[label], roots[label], targets[label], label, pair, output, sources[label], manifest["component_inputs"][label]))
        validate_pairs(runs)
        manifest["status"] = "complete"
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
        (output / "comparison.json").write_text(json.dumps({"manifest": manifest, "runs": runs}, indent=2) + "\n")
        print(f"Completed three pairs: {output / 'comparison.json'}", flush=True)
    except Exception as error:
        manifest["status"] = "failed"
        manifest["failure"] = str(error)
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
        raise


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Generate large Runtara workflows and measure local memory usage.

The harness is intentionally stdlib-only so it can run on a developer machine
without adding workspace dependencies. It samples the target process tree RSS
with ps(1), which works on macOS and Linux.
"""

from __future__ import annotations

import argparse
import base64
import csv
import json
import math
import os
import re
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable
from urllib import error as urllib_error
from urllib import request as urllib_request


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUTPUT_DIR = REPO_ROOT / ".data" / "memory-bench"
DEFAULT_SERVER_PID_FILE = REPO_ROOT / ".data" / "pids" / "runtara.pid"
DEFAULT_WASMTIME = Path.home() / ".wasmtime" / "bin" / "wasmtime"

CSV_FIELDS = [
    "run_id",
    "phase",
    "shape",
    "step_count",
    "payload_kb",
    "split_items",
    "split_parallelism",
    "run_index",
    "exit_code",
    "success",
    "duration_ms",
    "peak_rss_mb",
    "peak_vsz_mb",
    "peak_processes",
    "recommend_dev_mb",
    "recommend_prod_mb",
    "workflow_path",
    "binary_path",
    "sample_path",
    "command",
    "error",
]

SAMPLE_FIELDS = [
    "elapsed_ms",
    "rss_kb",
    "vsz_kb",
    "process_count",
    "pids",
]


@dataclass(frozen=True)
class Scenario:
    shape: str
    step_count: int
    workflow_path: Path

    @property
    def name(self) -> str:
        return f"{self.shape}-{self.step_count}"


@dataclass
class ProcessSample:
    elapsed_ms: int
    rss_kb: int
    vsz_kb: int
    process_count: int
    pids: list[int]


@dataclass
class CommandResult:
    command: list[str]
    exit_code: int
    duration_ms: int
    peak_rss_kb: int
    peak_vsz_kb: int
    peak_processes: int
    stdout: str
    stderr: str
    samples: list[ProcessSample]
    timed_out: bool = False


@dataclass
class ProvisionedRuntime:
    env: dict[str, str]
    server_pid: int
    process: subprocess.Popen[str] | None
    postgres_container: str | None
    postgres_url: str | None

    def close(self) -> None:
        if self.process and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=20)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()

        if self.postgres_container:
            subprocess.run(
                ["docker", "rm", "-f", self.postgres_container],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
            )


def parse_csv_list(value: str, *, cast=str) -> list[Any]:
    items = []
    for item in value.split(","):
        item = item.strip()
        if item:
            items.append(cast(item))
    return items


def mb_from_kb(kb: int) -> float:
    return round(kb / 1024.0, 2)


def shell_join(args: Iterable[str]) -> str:
    return " ".join(subprocess.list2cmdline([arg]) for arg in args)


def redact_command(args: list[str]) -> list[str]:
    redacted: list[str] = []
    skip_next = False
    for item in args:
        if skip_next:
            redacted.append("<redacted>")
            skip_next = False
            continue
        redacted.append(item)
        if item in {"--input", "--connection", "--binary"}:
            skip_next = True
    return redacted


def ref(value: str, default: Any | None = None) -> dict[str, Any]:
    mapping = {"valueType": "reference", "value": value}
    if default is not None:
        mapping["default"] = default
    return mapping


def imm(value: Any) -> dict[str, Any]:
    return {"valueType": "immediate", "value": value}


def condition_equals(left: dict[str, Any], right: dict[str, Any]) -> dict[str, Any]:
    return {
        "type": "operation",
        "op": "EQ",
        "arguments": [left, right],
    }


def base_graph(name: str, description: str) -> dict[str, Any]:
    return {
        "name": name,
        "description": description,
        "steps": {},
        "entryPoint": "",
        "executionPlan": [],
        "variables": {},
        "inputSchema": {
            "payload": {"type": "string"},
            "branchFlag": {"type": "boolean"},
            "items": {"type": "array"},
            "input": {"type": "object"},
        },
        "outputSchema": {},
    }


def count_steps(graph: dict[str, Any]) -> int:
    total = len(graph.get("steps", {}))
    for step in graph.get("steps", {}).values():
        subgraph = step.get("subgraph")
        if isinstance(subgraph, dict):
            total += count_steps(subgraph)
        on_wait = step.get("onWait")
        if isinstance(on_wait, dict):
            total += count_steps(on_wait)
    return total


def log_step(step_id: str, index: int, *, payload_ref: str = "data.payload") -> dict[str, Any]:
    return {
        "stepType": "Log",
        "id": step_id,
        "name": f"Log {index}",
        "level": "info",
        "message": f"memory benchmark step {index}",
        "context": {
            "stepIndex": imm(index),
            "payload": ref(payload_ref, ""),
        },
    }


def finish_step(*, source: str = "data.input") -> dict[str, Any]:
    return {
        "stepType": "Finish",
        "id": "finish",
        "inputMapping": {
            "result": ref(source, {}),
        },
    }


def build_linear_graph(step_count: int, *, payload_kb: int = 0, payload_shape: bool = False) -> dict[str, Any]:
    if step_count < 2:
        raise ValueError("step_count must be at least 2")

    graph = base_graph(
        "Memory Benchmark Linear",
        f"Linear workflow with {step_count} total steps.",
    )
    payload_ref = "variables.payloadBlob" if payload_shape else "data.payload"
    if payload_shape:
        graph["variables"]["payloadBlob"] = {
            "type": "string",
            "value": "x" * (payload_kb * 1024),
            "description": "Generated payload used to stress workflow JSON and event context size.",
        }

    previous_id: str | None = None
    for index in range(1, step_count):
        step_id = f"log_{index:04d}"
        graph["steps"][step_id] = log_step(step_id, index, payload_ref=payload_ref)
        if previous_id is None:
            graph["entryPoint"] = step_id
        else:
            graph["executionPlan"].append({"fromStep": previous_id, "toStep": step_id})
        previous_id = step_id

    graph["steps"]["finish"] = finish_step()
    graph["executionPlan"].append({"fromStep": previous_id, "toStep": "finish"})
    return graph


def build_branching_graph(step_count: int) -> dict[str, Any]:
    if step_count < 4:
        raise ValueError("branching workflows need at least 4 steps")

    graph = base_graph(
        "Memory Benchmark Branching",
        f"Branching workflow with {step_count} total steps.",
    )
    diamond_count = max(1, (step_count - 1) // 3)
    tail_logs = step_count - (diamond_count * 3) - 1
    pending_sources: list[str] = []

    for index in range(1, diamond_count + 1):
        cond_id = f"branch_{index:04d}"
        true_id = f"branch_{index:04d}_true"
        false_id = f"branch_{index:04d}_false"

        graph["steps"][cond_id] = {
            "stepType": "Conditional",
            "id": cond_id,
            "name": f"Branch {index}",
            "condition": condition_equals(ref("data.branchFlag", True), imm(True)),
        }
        graph["steps"][true_id] = log_step(true_id, index * 2 - 1)
        graph["steps"][false_id] = log_step(false_id, index * 2)

        if index == 1:
            graph["entryPoint"] = cond_id
        else:
            for source in pending_sources:
                graph["executionPlan"].append({"fromStep": source, "toStep": cond_id})

        graph["executionPlan"].append({"fromStep": cond_id, "toStep": true_id, "label": "true"})
        graph["executionPlan"].append({"fromStep": cond_id, "toStep": false_id, "label": "false"})
        pending_sources = [true_id, false_id]

    for index in range(1, tail_logs + 1):
        step_id = f"tail_{index:04d}"
        graph["steps"][step_id] = log_step(step_id, diamond_count * 2 + index)
        for source in pending_sources:
            graph["executionPlan"].append({"fromStep": source, "toStep": step_id})
        pending_sources = [step_id]

    graph["steps"]["finish"] = finish_step()
    for source in pending_sources:
        graph["executionPlan"].append({"fromStep": source, "toStep": "finish"})

    return graph


def split_subgraph(index: int) -> dict[str, Any]:
    log_id = f"split_{index:04d}_log"
    return {
        "name": f"Split {index} Body",
        "steps": {
            log_id: {
                "stepType": "Log",
                "id": log_id,
                "name": f"Split {index} Item Log",
                "level": "info",
                "message": f"processing split item {index}",
                "context": {
                    "item": ref("data", {}),
                    "itemValue": ref("data.value", None),
                },
            },
            "finish": {
                "stepType": "Finish",
                "id": "finish",
                "inputMapping": {
                    "item": ref("data", {}),
                },
            },
        },
        "entryPoint": log_id,
        "executionPlan": [{"fromStep": log_id, "toStep": "finish"}],
        "variables": {},
        "inputSchema": {},
        "outputSchema": {},
    }


def build_split_graph(step_count: int, *, split_parallelism: int) -> dict[str, Any]:
    if step_count < 4:
        raise ValueError("split workflows need at least 4 steps")

    graph = base_graph(
        "Memory Benchmark Split",
        f"Workflow with repeated Split steps and {step_count} total DSL steps including subgraphs.",
    )
    split_count = max(1, (step_count - 1) // 3)
    tail_logs = step_count - (split_count * 3) - 1
    previous_id: str | None = None

    for index in range(1, split_count + 1):
        step_id = f"split_{index:04d}"
        graph["steps"][step_id] = {
            "stepType": "Split",
            "id": step_id,
            "name": f"Split {index}",
            "config": {
                "value": ref("data.items", []),
                "parallelism": split_parallelism,
                "sequential": False,
                "dontStopOnFailed": False,
            },
            "subgraph": split_subgraph(index),
        }
        if previous_id is None:
            graph["entryPoint"] = step_id
        else:
            graph["executionPlan"].append({"fromStep": previous_id, "toStep": step_id})
        previous_id = step_id

    for index in range(1, tail_logs + 1):
        step_id = f"tail_{index:04d}"
        graph["steps"][step_id] = log_step(step_id, split_count + index)
        graph["executionPlan"].append({"fromStep": previous_id, "toStep": step_id})
        previous_id = step_id

    graph["steps"]["finish"] = {
        "stepType": "Finish",
        "id": "finish",
        "inputMapping": {
            "result": ref(f"steps.{previous_id}.outputs", {}),
        },
    }
    graph["executionPlan"].append({"fromStep": previous_id, "toStep": "finish"})
    return graph


def build_workflow(shape: str, step_count: int, *, payload_kb: int, split_parallelism: int) -> dict[str, Any]:
    if shape == "linear":
        graph = build_linear_graph(step_count)
    elif shape == "payload":
        graph = build_linear_graph(step_count, payload_kb=payload_kb, payload_shape=True)
        graph["name"] = "Memory Benchmark Payload"
        graph["description"] = f"Linear workflow with {payload_kb} KiB embedded payload variable."
    elif shape == "branching":
        graph = build_branching_graph(step_count)
    elif shape == "split":
        graph = build_split_graph(step_count, split_parallelism=split_parallelism)
    else:
        raise ValueError(f"unsupported shape: {shape}")

    actual = count_steps(graph)
    if actual != step_count:
        raise AssertionError(f"{shape} generated {actual} steps, expected {step_count}")
    return graph


def execution_input(*, payload_kb: int, split_items: int) -> str:
    payload = "y" * (payload_kb * 1024)
    items = [
        {
            "value": index,
            "status": "active" if index % 2 == 0 else "archived",
            "category": f"category-{index % 5}",
        }
        for index in range(split_items)
    ]
    return json.dumps(
        {
            "input": {"source": "memory-bench", "ok": True},
            "payload": payload,
            "branchFlag": True,
            "items": items,
        },
        separators=(",", ":"),
    )


def write_json(path: Path, data: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def read_process_table() -> dict[int, tuple[int, int, int]]:
    try:
        output = subprocess.check_output(
            ["ps", "-axo", "pid=,ppid=,rss=,vsz="],
            text=True,
            stderr=subprocess.DEVNULL,
        )
    except (OSError, subprocess.SubprocessError):
        return {}

    table: dict[int, tuple[int, int, int]] = {}
    for line in output.splitlines():
        parts = line.strip().split()
        if len(parts) < 4:
            continue
        try:
            pid = int(parts[0])
            ppid = int(parts[1])
            rss = int(parts[2])
            vsz = int(parts[3])
        except ValueError:
            continue
        table[pid] = (ppid, rss, vsz)
    return table


def collect_tree(root_pids: Iterable[int], table: dict[int, tuple[int, int, int]]) -> tuple[int, int, list[int]]:
    children: dict[int, list[int]] = {}
    for pid, (ppid, _rss, _vsz) in table.items():
        children.setdefault(ppid, []).append(pid)

    seen: set[int] = set()
    stack = [pid for pid in root_pids if pid > 0]
    while stack:
        pid = stack.pop()
        if pid in seen:
            continue
        if pid not in table:
            continue
        seen.add(pid)
        stack.extend(children.get(pid, []))

    rss_kb = sum(table[pid][1] for pid in seen)
    vsz_kb = sum(table[pid][2] for pid in seen)
    return rss_kb, vsz_kb, sorted(seen)


def sample_processes(started_at: float, root_pids: Iterable[int]) -> ProcessSample:
    table = read_process_table()
    rss_kb, vsz_kb, pids = collect_tree(root_pids, table)
    return ProcessSample(
        elapsed_ms=int((time.monotonic() - started_at) * 1000),
        rss_kb=rss_kb,
        vsz_kb=vsz_kb,
        process_count=len(pids),
        pids=pids,
    )


def run_monitored(
    command: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    sample_interval: float,
    timeout_seconds: int,
    extra_root_pids: list[int] | None = None,
) -> CommandResult:
    extra_root_pids = extra_root_pids or []
    started_at = time.monotonic()
    samples: list[ProcessSample] = []
    measured_command = time_wrapped_command(command)

    with tempfile.TemporaryFile(mode="w+t", encoding="utf-8") as stdout_file, tempfile.TemporaryFile(
        mode="w+t", encoding="utf-8"
    ) as stderr_file:
        proc = subprocess.Popen(
            measured_command,
            cwd=str(cwd),
            env=env,
            stdout=stdout_file,
            stderr=stderr_file,
            text=True,
        )
        root_pids = [proc.pid, *extra_root_pids]
        timed_out = False

        while True:
            samples.append(sample_processes(started_at, root_pids))
            exit_code = proc.poll()
            if exit_code is not None:
                break
            if time.monotonic() - started_at > timeout_seconds:
                timed_out = True
                proc.terminate()
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait()
                exit_code = proc.returncode if proc.returncode is not None else -signal.SIGKILL
                break
            time.sleep(sample_interval)

        duration_ms = int((time.monotonic() - started_at) * 1000)
        stdout_file.seek(0)
        stderr_file.seek(0)
        stdout = stdout_file.read()
        raw_stderr = stderr_file.read()

    time_peak_rss_kb = parse_time_peak_rss_kb(raw_stderr)
    stderr = strip_time_output(raw_stderr)
    peak_rss_kb = max(max((sample.rss_kb for sample in samples), default=0), time_peak_rss_kb)

    return CommandResult(
        command=command,
        exit_code=exit_code if exit_code is not None else -1,
        duration_ms=duration_ms,
        peak_rss_kb=peak_rss_kb,
        peak_vsz_kb=max((sample.vsz_kb for sample in samples), default=0),
        peak_processes=max((sample.process_count for sample in samples), default=0),
        stdout=stdout,
        stderr=stderr,
        samples=samples,
        timed_out=timed_out,
    )


def time_wrapped_command(command: list[str]) -> list[str]:
    time_path = Path("/usr/bin/time")
    if not time_path.exists():
        return command
    if sys.platform == "darwin":
        return [str(time_path), "-l", *command]
    return [str(time_path), "-v", *command]


def parse_time_peak_rss_kb(stderr: str) -> int:
    for line in stderr.splitlines():
        bsd_match = re.match(r"\s*(\d+)\s+maximum resident set size", line)
        if bsd_match:
            # BSD/macOS time -l reports bytes.
            return math.ceil(int(bsd_match.group(1)) / 1024)

        gnu_match = re.search(r"Maximum resident set size \(kbytes\):\s*(\d+)", line)
        if gnu_match:
            return int(gnu_match.group(1))
    return 0


def strip_time_output(stderr: str) -> str:
    bsd_labels = (
        "real",
        "user",
        "sys",
        "maximum resident set size",
        "average shared memory size",
        "average unshared data size",
        "average unshared stack size",
        "page reclaims",
        "page faults",
        "swaps",
        "block input operations",
        "block output operations",
        "messages sent",
        "messages received",
        "signals received",
        "voluntary context switches",
        "involuntary context switches",
        "instructions retired",
        "cycles elapsed",
        "peak memory footprint",
    )
    gnu_prefixes = (
        "Command being timed:",
        "User time (seconds):",
        "System time (seconds):",
        "Percent of CPU this job got:",
        "Elapsed (wall clock) time",
        "Average shared text size",
        "Average unshared data size",
        "Average stack size",
        "Average total size",
        "Maximum resident set size",
        "Average resident set size",
        "Major (requiring I/O) page faults",
        "Minor (reclaiming a frame) page faults",
        "Voluntary context switches",
        "Involuntary context switches",
        "Swaps",
        "File system inputs",
        "File system outputs",
        "Socket messages sent",
        "Socket messages received",
        "Signals delivered",
        "Page size (bytes)",
        "Exit status:",
    )

    cleaned: list[str] = []
    for line in stderr.splitlines():
        stripped = line.strip()
        if any(stripped.startswith(prefix) for prefix in gnu_prefixes):
            continue
        if re.match(r"^\d+(\.\d+)?\s+", stripped):
            label = re.sub(r"^\d+(\.\d+)?\s+", "", stripped)
            if any(label.startswith(item) for item in bsd_labels):
                continue
        cleaned.append(line)
    return "\n".join(cleaned).strip()


def write_samples(path: Path, samples: list[ProcessSample]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", newline="", encoding="utf-8") as fh:
        writer = csv.DictWriter(fh, fieldnames=SAMPLE_FIELDS)
        writer.writeheader()
        for sample in samples:
            writer.writerow(
                {
                    "elapsed_ms": sample.elapsed_ms,
                    "rss_kb": sample.rss_kb,
                    "vsz_kb": sample.vsz_kb,
                    "process_count": sample.process_count,
                    "pids": " ".join(str(pid) for pid in sample.pids),
                }
            )


def result_record(
    *,
    run_id: str,
    phase: str,
    scenario: Scenario,
    run_index: int,
    payload_kb: int,
    split_items: int,
    split_parallelism: int,
    command_result: CommandResult,
    binary_path: Path | None,
    sample_path: Path | None,
    error: str = "",
) -> dict[str, Any]:
    peak_rss_mb = mb_from_kb(command_result.peak_rss_kb)
    return {
        "run_id": run_id,
        "phase": phase,
        "shape": scenario.shape,
        "step_count": scenario.step_count,
        "payload_kb": payload_kb,
        "split_items": split_items,
        "split_parallelism": split_parallelism,
        "run_index": run_index,
        "exit_code": command_result.exit_code,
        "success": command_result.exit_code == 0 and not command_result.timed_out,
        "duration_ms": command_result.duration_ms,
        "peak_rss_mb": peak_rss_mb,
        "peak_vsz_mb": mb_from_kb(command_result.peak_vsz_kb),
        "peak_processes": command_result.peak_processes,
        "recommend_dev_mb": math.ceil(peak_rss_mb * 1.5),
        "recommend_prod_mb": math.ceil(peak_rss_mb * 2.0),
        "workflow_path": str(scenario.workflow_path),
        "binary_path": str(binary_path) if binary_path else "",
        "sample_path": str(sample_path) if sample_path else "",
        "command": shell_join(redact_command(command_result.command)),
        "error": error,
    }


def append_csv(path: Path, rows: list[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    exists = path.exists()
    with path.open("a", newline="", encoding="utf-8") as fh:
        writer = csv.DictWriter(fh, fieldnames=CSV_FIELDS)
        if not exists:
            writer.writeheader()
        writer.writerows(rows)


def default_tool_path(profile: str, name: str) -> Path:
    return REPO_ROOT / "target" / profile / name


def build_tools(args: argparse.Namespace, env: dict[str, str]) -> None:
    profile_args = ["--release"] if args.profile == "release" else []
    commands = [
        ["cargo", "build", "-p", "runtara-workflows", "--bin", "runtara-compile", *profile_args],
    ]
    if "execute" in args.phases:
        commands.extend(
            [
                ["cargo", "build", "-p", "runtara-management-sdk", "--bin", "runtara-ctl", *profile_args],
                ["cargo", "build", "-p", "runtara-environment", *profile_args],
            ]
        )

    for command in commands:
        print(f"building tool: {shell_join(command)}", flush=True)
        result = run_monitored(
            command,
            cwd=REPO_ROOT,
            env=env,
            sample_interval=args.sample_interval,
            timeout_seconds=args.timeout_seconds,
        )
        if result.exit_code != 0:
            sys.stderr.write(result.stderr)
            raise SystemExit(f"tool build failed: {shell_join(command)}")


def ensure_executable(path: Path, label: str) -> None:
    if not path.exists():
        raise SystemExit(f"{label} not found at {path}. Run with --build-tools or pass an explicit path.")
    if not os.access(path, os.X_OK):
        raise SystemExit(f"{label} is not executable: {path}")


def find_executable(name: str) -> Path | None:
    found = shutil.which(name)
    return Path(found).resolve() if found else None


def free_local_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def http_json(url: str, body: dict[str, Any] | None = None, *, timeout: float = 10.0) -> dict[str, Any]:
    data = None
    headers = {}
    method = "GET"
    if body is not None:
        data = json.dumps(body).encode("utf-8")
        headers["Content-Type"] = "application/json"
        method = "POST"

    req = urllib_request.Request(url, data=data, headers=headers, method=method)
    with urllib_request.urlopen(req, timeout=timeout) as resp:
        return json.loads(resp.read().decode("utf-8"))


def wait_for_health(env_addr: str, timeout_seconds: int) -> None:
    deadline = time.monotonic() + timeout_seconds
    url = f"http://{env_addr}/api/v1/health"
    last_error = ""
    while time.monotonic() < deadline:
        try:
            health = http_json(url, timeout=2.0)
            if health.get("healthy", True):
                return
        except Exception as exc:  # noqa: BLE001 - diagnostic loop
            last_error = str(exc)
        time.sleep(0.5)
    raise RuntimeError(f"runtime did not become healthy at {url}: {last_error}")


def ensure_wasm_target(args: argparse.Namespace, env: dict[str, str]) -> None:
    rustup = find_executable("rustup")
    if not rustup:
        return
    command = [str(rustup), "target", "add", "wasm32-wasip2"]
    print(f"provisioning rust target: {shell_join(command)}", flush=True)
    result = run_monitored(
        command,
        cwd=REPO_ROOT,
        env=env,
        sample_interval=args.sample_interval,
        timeout_seconds=args.timeout_seconds,
    )
    if result.exit_code != 0:
        sys.stderr.write(result.stderr)
        raise RuntimeError("failed to provision wasm32-wasip2 Rust target")


def copy_wasm_stdlib_cache(target_dir: Path, cache_dir: Path) -> None:
    wasm_release = target_dir / "wasm32-wasip2" / "release"
    wasm_deps = wasm_release / "deps"
    host_deps = target_dir / "release" / "deps"
    stdlib = wasm_release / "libruntara_workflow_stdlib.rlib"

    if not stdlib.exists():
        raise RuntimeError(f"compiled WASM stdlib missing at {stdlib}")

    deps_dir = cache_dir / "deps"
    if cache_dir.exists():
        shutil.rmtree(cache_dir)
    deps_dir.mkdir(parents=True, exist_ok=True)

    shutil.copy2(stdlib, cache_dir / stdlib.name)
    for rlib in wasm_deps.glob("*.rlib"):
        if "runtara_workflow_stdlib" in rlib.name:
            continue
        shutil.copy2(rlib, deps_dir / rlib.name)

    build_dir = wasm_release / "build"
    if build_dir.exists():
        for archive in build_dir.rglob("*.a"):
            shutil.copy2(archive, deps_dir / archive.name)

    dylib_ext = "dylib" if sys.platform == "darwin" else "so"
    for proc_macro in host_deps.glob(f"*.{dylib_ext}"):
        shutil.copy2(proc_macro, deps_dir / proc_macro.name)


def ensure_wasm_stdlib(args: argparse.Namespace, env: dict[str, str]) -> Path:
    cache_dir = (args.wasm_library_dir or (args.output_dir / "stdlib" / "wasm")).resolve()
    stdlib = cache_dir / "libruntara_workflow_stdlib.rlib"
    deps_dir = cache_dir / "deps"
    if stdlib.exists() and deps_dir.exists() and any(deps_dir.iterdir()):
        env["RUNTARA_WASM_LIBRARY_DIR"] = str(cache_dir)
        return cache_dir

    ensure_wasm_target(args, env)

    target_dir = Path(env.get("CARGO_TARGET_DIR", REPO_ROOT / "target")).resolve()
    commands = [
        [
            "cargo",
            "build",
            "-p",
            "runtara-workflow-stdlib",
            "--release",
            "--target",
            "wasm32-wasip2",
            "--no-default-features",
        ],
        ["cargo", "build", "-p", "runtara-workflow-stdlib", "--release"],
    ]
    build_env = env.copy()
    build_env["RUSTFLAGS"] = "-C embed-bitcode=yes"

    for index, command in enumerate(commands):
        command_env = build_env if index == 0 else env
        print(f"provisioning workflow stdlib: {shell_join(command)}", flush=True)
        result = run_monitored(
            command,
            cwd=REPO_ROOT,
            env=command_env,
            sample_interval=args.sample_interval,
            timeout_seconds=args.timeout_seconds,
        )
        if result.exit_code != 0:
            sys.stderr.write(result.stderr)
            raise RuntimeError(f"failed to provision workflow stdlib: {shell_join(command)}")

    copy_wasm_stdlib_cache(target_dir, cache_dir)
    env["RUNTARA_WASM_LIBRARY_DIR"] = str(cache_dir)
    return cache_dir


def read_pid_file(path: Path) -> int | None:
    try:
        content = path.read_text(encoding="utf-8").strip()
    except OSError:
        return None
    try:
        pid = int(content)
    except ValueError:
        return None
    try:
        os.kill(pid, 0)
    except OSError:
        return None
    return pid


def ensure_docker_available() -> None:
    docker = find_executable("docker")
    if not docker:
        raise RuntimeError("docker is required for isolated Postgres provisioning")
    result = subprocess.run(
        [str(docker), "version", "--format", "{{.Server.Version}}"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError("docker is installed but the daemon is not reachable")


def provision_postgres(args: argparse.Namespace) -> tuple[str | None, str]:
    if args.database_url:
        return None, args.database_url

    if args.postgres_mode == "external":
        raise RuntimeError("--postgres-mode external requires --database-url")

    if args.postgres_mode in ("auto", "docker"):
        ensure_docker_available()
        port = free_local_port()
        container = f"runtara-memory-bench-{int(time.time())}-{os.getpid()}"
        command = [
            "docker",
            "run",
            "-d",
            "--name",
            container,
            "-e",
            "POSTGRES_PASSWORD=runtara",
            "-e",
            "POSTGRES_USER=runtara",
            "-e",
            "POSTGRES_DB=runtara",
            "-p",
            f"127.0.0.1:{port}:5432",
            args.postgres_image,
        ]
        print(f"provisioning postgres: {shell_join(command)}", flush=True)
        result = subprocess.run(command, cwd=REPO_ROOT, text=True, capture_output=True, check=False)
        if result.returncode != 0:
            raise RuntimeError(f"failed to start Postgres container: {result.stderr.strip()}")

        deadline = time.monotonic() + args.runtime_start_timeout
        last_status = ""
        while time.monotonic() < deadline:
            status = subprocess.run(
                ["docker", "inspect", "-f", "{{.State.Status}}", container],
                text=True,
                capture_output=True,
                check=False,
            )
            if status.returncode == 0:
                last_status = status.stdout.strip()
                if last_status == "exited":
                    break
            check = subprocess.run(
                [
                    "docker",
                    "exec",
                    container,
                    "pg_isready",
                    "-U",
                    "runtara",
                    "-d",
                    "runtara",
                ],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
            )
            if check.returncode == 0:
                return container, f"postgres://runtara:runtara@127.0.0.1:{port}/runtara"
            time.sleep(0.5)

        logs = subprocess.run(
            ["docker", "logs", "--tail", "120", container],
            text=True,
            capture_output=True,
            check=False,
        )
        subprocess.run(["docker", "rm", "-f", container], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False)
        detail = logs.stderr or logs.stdout or last_status
        raise RuntimeError(f"Postgres container did not become ready. Container detail:\n{detail.strip()}")

    raise RuntimeError(f"unsupported postgres mode: {args.postgres_mode}")


def start_isolated_runtime(args: argparse.Namespace, env: dict[str, str]) -> ProvisionedRuntime:
    ensure_executable(args.runtara_environment, "runtara-environment")
    postgres_container, postgres_url = provision_postgres(args)
    env_port = free_local_port()
    core_port = free_local_port()
    data_dir = args.output_dir / "runtime-data"
    log_dir = args.output_dir / "logs"
    log_dir.mkdir(parents=True, exist_ok=True)
    data_dir.mkdir(parents=True, exist_ok=True)

    runtime_env = env.copy()
    runtime_env.update(
        {
            "DATA_DIR": str(data_dir),
            "RUNTARA_DATABASE_URL": postgres_url,
            "RUNTARA_ENV_HTTP_PORT": str(env_port),
            "RUNTARA_CORE_ADDR": f"127.0.0.1:{core_port}",
            "RUNTARA_RUNNER": args.runtime_runner,
            "RUNTARA_SKIP_CERT_VERIFICATION": "true",
            "RUST_LOG": args.runtime_log,
        }
    )
    if args.wasmtime_path:
        runtime_env["WASMTIME_PATH"] = str(args.wasmtime_path)

    command = [str(args.runtara_environment)]
    log_path = log_dir / "runtara-environment.log"
    print(f"starting isolated runtime: {shell_join(command)}", flush=True)
    log_fh = log_path.open("w", encoding="utf-8")
    process = subprocess.Popen(
        command,
        cwd=REPO_ROOT,
        env=runtime_env,
        stdout=log_fh,
        stderr=subprocess.STDOUT,
        text=True,
    )
    log_fh.close()

    try:
        wait_for_health(f"127.0.0.1:{env_port}", args.runtime_start_timeout)
    except Exception:
        if process.poll() is not None:
            try:
                tail = "\n".join(log_path.read_text(encoding="utf-8").splitlines()[-80:])
            except OSError:
                tail = ""
            raise RuntimeError(f"runtime exited during startup. Log tail:\n{tail}")
        raise

    runtime_env["RUNTARA_ENVIRONMENT_ADDR"] = f"127.0.0.1:{env_port}"
    runtime_env["RUNTARA_HTTP_URL"] = f"http://127.0.0.1:{core_port}"
    runtime_env["RUNTARA_CORE_HTTP_PORT"] = str(core_port)
    return ProvisionedRuntime(
        env=runtime_env,
        server_pid=process.pid,
        process=process,
        postgres_container=postgres_container,
        postgres_url=postgres_url,
    )


def register_image(args: argparse.Namespace, env: dict[str, str], binary_path: Path, scenario: Scenario) -> str:
    env_addr = env.get("RUNTARA_ENVIRONMENT_ADDR", "127.0.0.1:8002")
    runner_type = resolve_runner_type(args, binary_path)
    body = {
        "tenant_id": args.tenant,
        "name": f"memory-bench-{scenario.name}-{int(time.time())}",
        "description": "Generated by scripts/measure_memory.py",
        "binary": base64.b64encode(binary_path.read_bytes()).decode("ascii"),
        "runner_type": runner_type,
        "metadata": {
            "source": "scripts/measure_memory.py",
            "shape": scenario.shape,
            "step_count": scenario.step_count,
        },
    }
    try:
        response = http_json(
            f"http://{env_addr}/api/v1/images",
            body,
            timeout=args.http_timeout,
        )
    except urllib_error.HTTPError as exc:
        detail = exc.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"image registration failed: HTTP {exc.code}: {detail}") from exc
    if not response.get("success"):
        raise RuntimeError(f"image registration failed: {response.get('error')}")
    image_id = response.get("image_id")
    if not image_id:
        raise RuntimeError(f"image registration returned no image_id: {response}")
    return str(image_id)


def resolve_runner_type(args: argparse.Namespace, binary_path: Path) -> str:
    if args.runner_type != "auto":
        return args.runner_type
    compile_target = args.compile_target or os.environ.get("RUNTARA_COMPILE_TARGET") or "wasm32-wasip2"
    if compile_target == "wasm32-wasip2" or binary_path.suffix == ".wasm":
        return "wasm"
    return "oci"


def runtime_runner_for_args(args: argparse.Namespace) -> str:
    if args.runtime_runner != "auto":
        return args.runtime_runner
    compile_target = args.compile_target or os.environ.get("RUNTARA_COMPILE_TARGET") or "wasm32-wasip2"
    if compile_target == "wasm32-wasip2":
        return "wasm"
    return "oci"


def runtime_requires_wasm_stdlib(args: argparse.Namespace) -> bool:
    target = args.compile_target or os.environ.get("RUNTARA_COMPILE_TARGET") or "wasm32-wasip2"
    return target == "wasm32-wasip2" and any(phase in args.phases for phase in ("compile", "execute"))


def assert_completed(result: CommandResult) -> None:
    if result.exit_code != 0:
        return
    try:
        payload = json.loads(result.stdout)
    except json.JSONDecodeError:
        return
    status = payload.get("status")
    if status and status != "completed":
        result.exit_code = 1
        result.stderr = (result.stderr + "\n" if result.stderr else "") + f"instance status was {status}: {result.stdout}"


def run_health_check(args: argparse.Namespace, env: dict[str, str]) -> None:
    command = [str(args.runtara_ctl), "health"]
    result = run_monitored(
        command,
        cwd=REPO_ROOT,
        env=env,
        sample_interval=args.sample_interval,
        timeout_seconds=args.timeout_seconds,
    )
    if result.exit_code != 0:
        sys.stderr.write(result.stderr)
        raise RuntimeError("runtara-ctl health failed against isolated runtime")


def delete_image(args: argparse.Namespace, env: dict[str, str], image_id: str) -> None:
    command = [str(args.runtara_ctl), "delete-image", image_id, args.tenant]
    subprocess.run(command, cwd=REPO_ROOT, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False)


def compile_binary(args: argparse.Namespace, env: dict[str, str], scenario: Scenario, binary_path: Path) -> CommandResult:
    binary_path.parent.mkdir(parents=True, exist_ok=True)
    command = [
        str(args.runtara_compile),
        "--workflow",
        str(scenario.workflow_path),
        "--tenant",
        args.tenant,
        "--workflow-id",
        scenario.name,
        "--output",
        str(binary_path),
    ]
    return run_monitored(
        command,
        cwd=REPO_ROOT,
        env=env,
        sample_interval=args.sample_interval,
        timeout_seconds=args.timeout_seconds,
    )


def run_validate(args: argparse.Namespace, env: dict[str, str], scenario: Scenario) -> CommandResult:
    command = [
        str(args.runtara_compile),
        "--workflow",
        str(scenario.workflow_path),
        "--tenant",
        args.tenant,
        "--workflow-id",
        scenario.name,
        "--validate",
    ]
    return run_monitored(
        command,
        cwd=REPO_ROOT,
        env=env,
        sample_interval=args.sample_interval,
        timeout_seconds=args.timeout_seconds,
    )


def run_execute(
    args: argparse.Namespace,
    env: dict[str, str],
    scenario: Scenario,
    image_id: str,
    server_pids: list[int],
) -> CommandResult:
    input_json = execution_input(payload_kb=args.payload_kb, split_items=args.split_items)
    start_command = [
        str(args.runtara_ctl),
        "start",
        "--image",
        image_id,
        "--tenant",
        args.tenant,
        "--input",
        input_json,
    ]
    start_result = run_monitored(
        start_command,
        cwd=REPO_ROOT,
        env=env,
        sample_interval=args.sample_interval,
        timeout_seconds=args.timeout_seconds,
        extra_root_pids=server_pids,
    )
    if start_result.exit_code != 0:
        return start_result

    instance_id = start_result.stdout.strip().splitlines()[-1].strip()
    wait_command = [str(args.runtara_ctl), "wait", instance_id, "--poll", str(args.poll_ms)]
    wait_result = run_monitored(
        wait_command,
        cwd=REPO_ROOT,
        env=env,
        sample_interval=args.sample_interval,
        timeout_seconds=args.timeout_seconds,
        extra_root_pids=server_pids,
    )
    assert_completed(wait_result)

    combined_samples = [
        *start_result.samples,
        *wait_result.samples,
    ]
    return CommandResult(
        command=[*start_command, "&&", *wait_command],
        exit_code=wait_result.exit_code,
        duration_ms=start_result.duration_ms + wait_result.duration_ms,
        peak_rss_kb=max(start_result.peak_rss_kb, wait_result.peak_rss_kb),
        peak_vsz_kb=max(start_result.peak_vsz_kb, wait_result.peak_vsz_kb),
        peak_processes=max(start_result.peak_processes, wait_result.peak_processes),
        stdout=wait_result.stdout,
        stderr=start_result.stderr + wait_result.stderr,
        samples=combined_samples,
        timed_out=start_result.timed_out or wait_result.timed_out,
    )


def generate_scenarios(args: argparse.Namespace) -> list[Scenario]:
    workflows_dir = args.output_dir / "workflows"
    scenarios: list[Scenario] = []
    for shape in args.shapes:
        for step_count in args.step_counts:
            graph = build_workflow(
                shape,
                step_count,
                payload_kb=args.payload_kb,
                split_parallelism=args.split_parallelism,
            )
            workflow_path = workflows_dir / f"{shape}-{step_count}.json"
            write_json(workflow_path, graph)
            scenarios.append(Scenario(shape=shape, step_count=step_count, workflow_path=workflow_path))
    return scenarios


def write_manifest(args: argparse.Namespace, scenarios: list[Scenario]) -> None:
    manifest = {
        "generated_at_unix_ms": int(time.time() * 1000),
        "repo_root": str(REPO_ROOT),
        "output_dir": str(args.output_dir),
        "phases": args.phases,
        "shapes": args.shapes,
        "step_counts": args.step_counts,
        "payload_kb": args.payload_kb,
        "split_items": args.split_items,
        "split_parallelism": args.split_parallelism,
        "sample_interval": args.sample_interval,
        "scenarios": [
            {
                "shape": scenario.shape,
                "step_count": scenario.step_count,
                "workflow_path": str(scenario.workflow_path),
            }
            for scenario in scenarios
        ],
    }
    write_json(args.output_dir / "manifest.json", manifest)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate 100+ step Runtara workflows and measure local memory requirements.",
    )
    parser.add_argument(
        "--phases",
        default="validate",
        help="Comma-separated phases: generate, validate, compile, execute, all, e2e. Default: validate.",
    )
    parser.add_argument(
        "--shapes",
        default="linear,branching,split,payload",
        help="Comma-separated workflow shapes: linear, branching, split, payload.",
    )
    parser.add_argument(
        "--step-counts",
        default="100,250,500,1000",
        help="Comma-separated total DSL step counts. Nested split subgraph steps are included.",
    )
    parser.add_argument("--runs", type=int, default=1, help="Runs per scenario and phase. Default: 1.")
    parser.add_argument("--payload-kb", type=int, default=64, help="Payload size in KiB. Default: 64.")
    parser.add_argument("--split-items", type=int, default=25, help="Runtime items passed to Split workflows.")
    parser.add_argument("--split-parallelism", type=int, default=10, help="Split step parallelism.")
    parser.add_argument("--sample-interval", type=float, default=0.1, help="Sampling interval in seconds.")
    parser.add_argument("--timeout-seconds", type=int, default=1200, help="Timeout for each command.")
    parser.add_argument("--poll-ms", type=int, default=100, help="runtara-ctl wait poll interval.")
    parser.add_argument("--output-dir", type=Path, default=DEFAULT_OUTPUT_DIR)
    parser.add_argument("--profile", choices=["release", "debug"], default="release")
    parser.add_argument("--build-tools", action="store_true", help="Build runtara-compile and runtara-ctl first.")
    parser.add_argument("--runtara-compile", type=Path)
    parser.add_argument("--runtara-ctl", type=Path)
    parser.add_argument("--runtara-environment", type=Path)
    parser.add_argument("--tenant", default="memory-bench")
    parser.add_argument("--compile-target", help="Sets RUNTARA_COMPILE_TARGET for compilation phases.")
    parser.add_argument(
        "--provision",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Provision stdlib, Postgres, and an isolated runtime when needed. Default: true for e2e, false otherwise.",
    )
    parser.add_argument(
        "--postgres-mode",
        choices=["auto", "docker", "external"],
        default="auto",
        help="Postgres provisioning mode for isolated E2E execution.",
    )
    parser.add_argument("--postgres-image", default="postgres:16-alpine")
    parser.add_argument("--database-url", help="Use an existing Postgres database URL instead of provisioning one.")
    parser.add_argument("--wasm-library-dir", type=Path, help="Use or create a WASM stdlib cache at this path.")
    parser.add_argument("--wasmtime-path", type=Path, default=DEFAULT_WASMTIME if DEFAULT_WASMTIME.exists() else None)
    parser.add_argument(
        "--runner-type",
        choices=["auto", "oci", "native", "wasm"],
        default="auto",
        help="Runner type to store on registered images. Default: auto.",
    )
    parser.add_argument(
        "--runtime-runner",
        choices=["auto", "oci", "native", "wasm"],
        default="auto",
        help="Runner backend for isolated runtime. Default: auto.",
    )
    parser.add_argument("--runtime-log", default="runtara_environment=info,runtara_core=warn")
    parser.add_argument("--runtime-start-timeout", type=int, default=90)
    parser.add_argument("--http-timeout", type=float, default=30.0)
    parser.add_argument("--server-pid", type=int, help="PID of an already running local runtime.")
    parser.add_argument("--server-pid-file", type=Path, default=DEFAULT_SERVER_PID_FILE)
    parser.add_argument("--start-runtime", action="store_true", help="Provision and start an isolated runtime before execute phase.")
    parser.add_argument(
        "--cleanup-images",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="Delete registered benchmark images after execute phase. Default: true.",
    )
    parser.add_argument(
        "--continue-on-error",
        action="store_true",
        help="Continue remaining scenarios after a phase fails.",
    )

    args = parser.parse_args()
    phases = parse_csv_list(args.phases)
    if "e2e" in phases:
        phases = ["validate", "compile", "execute"]
        if args.step_counts == parser.get_default("step_counts"):
            args.step_counts = "100"
        if args.shapes == parser.get_default("shapes"):
            args.shapes = "linear"
        if args.provision is None:
            args.provision = True
        args.build_tools = True
    elif "all" in phases:
        phases = ["validate", "compile", "execute"]
    args.phases = phases
    args.shapes = parse_csv_list(args.shapes)
    args.step_counts = parse_csv_list(args.step_counts, cast=int)
    args.output_dir = args.output_dir.resolve()
    args.runtara_compile = (args.runtara_compile or default_tool_path(args.profile, "runtara-compile")).resolve()
    args.runtara_ctl = (args.runtara_ctl or default_tool_path(args.profile, "runtara-ctl")).resolve()
    args.runtara_environment = (
        args.runtara_environment or default_tool_path(args.profile, "runtara-environment")
    ).resolve()
    args.server_pid_file = args.server_pid_file.resolve()
    args.wasm_library_dir = args.wasm_library_dir.resolve() if args.wasm_library_dir else None
    args.wasmtime_path = args.wasmtime_path.resolve() if args.wasmtime_path else None
    args.provision = bool(args.provision) if args.provision is not None else False
    if args.start_runtime:
        args.provision = True
    args.runtime_runner = runtime_runner_for_args(args)

    valid_phases = {"generate", "validate", "compile", "execute"}
    unknown_phases = sorted(set(args.phases) - valid_phases)
    if unknown_phases:
        parser.error(f"unknown phase(s): {', '.join(unknown_phases)}")
    valid_shapes = {"linear", "branching", "split", "payload"}
    unknown_shapes = sorted(set(args.shapes) - valid_shapes)
    if unknown_shapes:
        parser.error(f"unknown shape(s): {', '.join(unknown_shapes)}")
    if args.runs < 1:
        parser.error("--runs must be at least 1")
    if args.sample_interval <= 0:
        parser.error("--sample-interval must be > 0")
    if any(count < 2 for count in args.step_counts):
        parser.error("--step-counts values must be >= 2")
    return args


def main() -> int:
    args = parse_args()
    args.output_dir.mkdir(parents=True, exist_ok=True)
    samples_dir = args.output_dir / "samples"
    binaries_dir = args.output_dir / "binaries"
    summary_csv = args.output_dir / "memory_results.csv"
    summary_json = args.output_dir / "memory_results.json"

    env = os.environ.copy()
    env.setdefault("DATA_DIR", str(args.output_dir / "data"))
    env.setdefault("RUNTARA_ENVIRONMENT_ADDR", "127.0.0.1:8002")
    env.setdefault("RUNTARA_SKIP_CERT_VERIFICATION", "true")
    if args.compile_target:
        env["RUNTARA_COMPILE_TARGET"] = args.compile_target

    scenarios = generate_scenarios(args)
    write_manifest(args, scenarios)
    print(f"generated {len(scenarios)} workflow(s) in {args.output_dir / 'workflows'}", flush=True)

    if args.phases == ["generate"]:
        print(f"manifest: {args.output_dir / 'manifest.json'}", flush=True)
        return 0

    if args.build_tools:
        build_tools(args, env)

    if args.provision and runtime_requires_wasm_stdlib(args):
        cache_dir = ensure_wasm_stdlib(args, env)
        print(f"using WASM stdlib cache: {cache_dir}", flush=True)

    if any(phase in args.phases for phase in ("validate", "compile", "execute")):
        ensure_executable(args.runtara_compile, "runtara-compile")
    if "execute" in args.phases:
        ensure_executable(args.runtara_ctl, "runtara-ctl")
        if args.provision:
            ensure_executable(args.runtara_environment, "runtara-environment")
            if args.runtime_runner == "wasm":
                if not args.wasmtime_path:
                    raise SystemExit("WASM runtime execution requires wasmtime. Install wasmtime or pass --wasmtime-path.")
                ensure_executable(args.wasmtime_path, "wasmtime")

    server_pids: list[int] = []
    provisioned_runtime: ProvisionedRuntime | None = None
    if "execute" in args.phases:
        if args.provision:
            provisioned_runtime = start_isolated_runtime(args, env)
            env = provisioned_runtime.env
            server_pids.append(provisioned_runtime.server_pid)
            run_health_check(args, env)
        elif args.server_pid:
            server_pids.append(args.server_pid)
        else:
            pid = read_pid_file(args.server_pid_file)
            if pid:
                server_pids.append(pid)
        if not server_pids:
            raise SystemExit(
                "execute phase needs a runtime PID for meaningful RSS sampling. "
                "Use --start-runtime, --server-pid, or --server-pid-file."
            )

    all_rows: list[dict[str, Any]] = []
    try:
        for scenario in scenarios:
            image_id: str | None = None
            try:
                binary_path = binaries_dir / f"{scenario.name}.bin"

                if "execute" in args.phases and not binary_path.exists():
                    print(f"preparing executable for {scenario.name}", flush=True)
                    prep_result = compile_binary(args, env, scenario, binary_path)
                    if prep_result.exit_code != 0:
                        sys.stderr.write(prep_result.stderr)
                        raise RuntimeError(f"failed to compile executable for {scenario.name}")

                if "execute" in args.phases:
                    image_id = register_image(args, env, binary_path, scenario)

                for phase in args.phases:
                    if phase == "generate":
                        continue
                    for run_index in range(1, args.runs + 1):
                        run_id = f"{scenario.name}-{phase}-run-{run_index}"
                        sample_path = samples_dir / f"{run_id}.csv"
                        print(f"running {run_id}", flush=True)

                        if phase == "validate":
                            command_result = run_validate(args, env, scenario)
                            phase_binary_path = None
                        elif phase == "compile":
                            phase_binary_path = binaries_dir / f"{scenario.name}-run-{run_index}.bin"
                            command_result = compile_binary(args, env, scenario, phase_binary_path)
                        elif phase == "execute":
                            if image_id is None:
                                raise RuntimeError("missing registered image")
                            command_result = run_execute(args, env, scenario, image_id, server_pids)
                            phase_binary_path = binary_path
                        else:
                            continue

                        write_samples(sample_path, command_result.samples)
                        error = ""
                        if command_result.exit_code != 0:
                            error = (command_result.stderr or command_result.stdout).strip().splitlines()[-1:]
                            error = error[0] if error else f"exit code {command_result.exit_code}"
                        row = result_record(
                            run_id=run_id,
                            phase=phase,
                            scenario=scenario,
                            run_index=run_index,
                            payload_kb=args.payload_kb,
                            split_items=args.split_items,
                            split_parallelism=args.split_parallelism,
                            command_result=command_result,
                            binary_path=phase_binary_path,
                            sample_path=sample_path,
                            error=error,
                        )
                        all_rows.append(row)
                        append_csv(summary_csv, [row])

                        status = "ok" if row["success"] else "failed"
                        print(
                            f"  {status}: peak_rss={row['peak_rss_mb']} MiB "
                            f"duration={row['duration_ms']} ms",
                            flush=True,
                        )
                        if not row["success"] and not args.continue_on_error:
                            raise RuntimeError(f"{run_id} failed: {error}")
            except Exception as exc:
                if args.continue_on_error:
                    print(f"scenario failed and was skipped: {scenario.name}: {exc}", file=sys.stderr, flush=True)
                    continue
                raise
            finally:
                if image_id and args.cleanup_images:
                    delete_image(args, env, image_id)
    finally:
        if provisioned_runtime:
            provisioned_runtime.close()

    write_json(summary_json, {"results": all_rows})
    print(f"summary csv: {summary_csv}", flush=True)
    print(f"summary json: {summary_json}", flush=True)

    if all_rows:
        peak = max(float(row["peak_rss_mb"]) for row in all_rows)
        print(f"max observed peak RSS: {peak:.2f} MiB", flush=True)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        raise SystemExit(130)

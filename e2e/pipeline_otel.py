#!/usr/bin/env python3
"""Exercise real admission, launch, runner and step instrumentation over OTLP."""
import json
import datetime
import math
import os
from pathlib import Path
import signal
import shutil
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
ARTIFACTS = Path(tempfile.mkdtemp(prefix="runtara-otel-e2e-"))
ENV = os.environ.copy()
ENV.setdefault("PGHOST", "127.0.0.1")
ENV.setdefault("PGPORT", "5432")
ENV.setdefault("PGUSER", "postgres")
PSQL = ENV.get("PSQL", "psql")
TAG = f"metrics_e2e_{os.getpid()}"
PUBLIC = int(ENV.get("TEST_PORT_PUBLIC", "17780"))
OTLP = int(ENV.get("TEST_OTLP_PORT", "14319"))
API = f"http://127.0.0.1:{PUBLIC}/api/runtime"
METRICS = ARTIFACTS / "metrics.jsonl"
PROCESSES = []


def sql(database, statement):
    result = subprocess.run([PSQL, "-X", "-v", "ON_ERROR_STOP=1", "-At", "-d", database, "-c", statement],
                            env=ENV, capture_output=True, text=True)
    if result.returncode:
        raise AssertionError("Isolated test SQL failed (database details omitted)")
    return result.stdout.strip()


def wait_for(check, label, seconds=90):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.2)
    raise AssertionError(f"Timed out: {label}")


def http(path, body=None, timeout=120):
    request = urllib.request.Request(API + path, data=None if body is None else json.dumps(body).encode(),
                                     headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return json.load(response)


def healthy():
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{PUBLIC}/health", timeout=1) as response:
            return response.status == 200
    except (OSError, urllib.error.URLError):
        return False


def spawn(command, name, environment=ENV):
    log = open(ARTIFACTS / f"{name}.log", "ab")
    process = subprocess.Popen(command, env=environment, cwd=ARTIFACTS, stdout=log, stderr=log)
    log.close()
    PROCESSES.append(process)
    return process


def stop(process, timeout=20):
    if process.poll() is None:
        process.send_signal(signal.SIGTERM)
        try:
            process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
            raise AssertionError("Test process did not shut down within its bounded grace")


def receiver():
    binary = ENV.get("OTEL_TEST_RECEIVER_BIN", str(ROOT / "target/debug/examples/otel_test_receiver"))
    process = spawn([binary, str(OTLP), str(METRICS)], "receiver")

    def listening():
        try:
            with socket.create_connection(("127.0.0.1", OTLP), timeout=0.2):
                return True
        except OSError:
            return False

    wait_for(listening, "OTLP receiver", 10)
    return process


def server(disabled=False):
    environment = ENV.copy()
    # Use a dedicated trust-authenticated local test cluster.
    base = f"postgresql://{ENV['PGUSER']}@{ENV['PGHOST']}:{ENV['PGPORT']}"
    environment.update({
        "RUNTARA_SERVER_DATABASE_URL": f"{base}/{TAG}_server",
        "OBJECT_MODEL_DATABASE_URL": f"{base}/{TAG}_server",
        "RUNTARA_DATABASE_URL": f"{base}/{TAG}_runtime",
        "TENANT_ID": TAG, "AUTH_PROVIDER": "local", "ENABLE_OPENAPI_DOCS": "true", "SERVER_HOST": "127.0.0.1",
        "SERVER_PORT": str(PUBLIC), "INTERNAL_PORT": str(PUBLIC + 1),
        "RUNTARA_CORE_PORT": str(PUBLIC + 10), "RUNTARA_ENVIRONMENT_PORT": str(PUBLIC + 11),
        "RUNTARA_CORE_HTTP_PORT": str(PUBLIC + 12), "RUNTARA_ENV_HTTP_PORT": str(PUBLIC + 13),
        "DATA_DIR": str(ARTIFACTS / "data"), "RUNTARA_DEV_MODE": "false",
        "RUNTARA_AGENT_COMPONENTS_DIR": str(ROOT / "target/wasm32-wasip2/release"),
        "VALKEY_HOST": "127.0.0.1", "VALKEY_PORT": ENV.get("TEST_VALKEY_PORT", "16399"),
        "OTEL_SDK_DISABLED": str(disabled).lower(), "OTEL_METRIC_EXPORT_INTERVAL": "500",
        "OTEL_EXPORTER_OTLP_ENDPOINT": f"http://127.0.0.1:{OTLP}",
        "OTEL_EXPORTER_OTLP_TIMEOUT": "1000", "OTEL_METRIC_EXPORT_TIMEOUT": "1000",
        "RUNTARA_MAX_CONCURRENT_RUNS": "2", "RUNTARA_TRIGGER_WORKERS": "2",
        "RUNTARA_TRIGGER_CONCURRENCY": "3", "RUNTARA_SHUTDOWN_GRACE_MS": "2000",
        "RUNTARA_SHUTDOWN_INTAKE_GRACE_MS": "2000", "RUNTARA_CORE_SHUTDOWN_GRACE_MS": "1000",
        "RUST_LOG": "warn", "SQLX_OFFLINE": "true",
    })
    process = spawn([ENV.get("RUNTARA_SERVER_BIN", str(ROOT / "target/debug/runtara-server"))],
                    "server-disabled" if disabled else "server", environment)
    wait_for(lambda: healthy() if process.poll() is None else (_ for _ in ()).throw(
        AssertionError("Server exited during startup; inspect the retained local log")), "local server")
    return process


def workflow(tracked):
    created = http("/workflows/create", {"name": f"otel-{'tracked' if tracked else 'untracked'}", "description": "OTLP pipeline test"})
    workflow_id = created["data"]["id"]
    graph = {"name": "otel-probe", "durable": False, "entryPoint": "finish",
             "steps": {"finish": {"stepType": "Finish", "id": "finish",
                                    "inputMapping": {"ok": {"valueType": "immediate", "value": True}}}},
             "executionPlan": [], "variables": {}, "inputSchema": {}, "outputSchema": {}}
    assert http(f"/workflows/{workflow_id}/update", {"executionGraph": graph, "trackEvents": tracked})["success"]
    versions = http(f"/workflows/{workflow_id}/versions")["data"]
    version = max(v["versionNumber"] for v in versions)
    assert http(f"/workflows/{workflow_id}/versions/{version}/compile", {}, timeout=900)["success"]
    return workflow_id


def execute(workflow_id):
    result = http(f"/workflows/{workflow_id}/execute", {"inputs": {"data": {}}})
    assert result["success"], "Execution must be admitted"
    instance = result["data"]["instanceId"]
    # Instance IDs came from our own API; no arbitrary SQL input.
    import uuid
    uuid.UUID(instance)
    wait_for(lambda: sql(f"{TAG}_runtime", f"SELECT status FROM instances WHERE instance_id = '{instance}'") == "completed",
             "workflow completion", 120)
    return instance


def metric_rows():
    if not METRICS.exists():
        return []
    rows = []
    for line in METRICS.read_text().splitlines():
        try:
            request = json.loads(line)
        except json.JSONDecodeError:
            continue  # concurrent final line
        for resource in request.get("resourceMetrics", []):
            for scope in resource.get("scopeMetrics", []):
                rows.extend(scope.get("metrics", []))
    return rows


def points(name):
    for metric in metric_rows():
        if metric["name"] == name:
            for kind in ("sum", "histogram", "gauge"):
                yield from metric.get(kind, {}).get("dataPoints", [])


def attrs(point):
    return {a["key"]: next(iter(a["value"].values())) for a in point.get("attributes", [])}


def positive(name):
    return any(float(p.get("asInt", p.get("asDouble", p.get("count", 0)))) > 0 for p in points(name))


def usage():
    # Explicit end covers the current minute; default UI reads complete minutes.
    end = datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(minutes=1)
    start = end - datetime.timedelta(hours=1)
    from urllib.parse import urlencode
    result = http("/metrics/tenant?" + urlencode({"startTime": start.isoformat(), "endTime": end.isoformat(), "granularity": "1m"}))
    assert result["success"]
    buckets = result["data"]["metrics"]
    return {
        "invocations": sum(b["invocation_count"] for b in buckets),
        "duration_count": sum(b["duration_observation_count"] for b in buckets),
        "duration_sum": sum((b["avg_duration_seconds"] or 0) * b["duration_observation_count"] for b in buckets),
        "memory_count": sum(b["memory_observation_count"] for b in buckets),
        "memory_sum": sum((b["avg_memory_bytes"] or 0) * b["memory_observation_count"] for b in buckets),
        "cpu_count": sum(b["cpu_observation_count"] for b in buckets),
        "cpu_sum": sum((b["avg_cpu_seconds"] or 0) * b["cpu_observation_count"] for b in buckets),
    }


def latest_points(name):
    for metric in reversed(metric_rows()):
        if metric["name"] == name:
            for kind in ("sum", "histogram"):
                if kind in metric:
                    return metric[kind].get("dataPoints", [])
    return []


def verify_usage_export(expected):
    wait_for(lambda: usage()["invocations"] == expected, "retained Usage counts")
    wait_for(lambda: sum(int(p["asInt"]) for p in latest_points("runtara.workflow.invocations.total")) == expected,
             "Usage OTEL invocation parity")
    for field, name in [("duration", "runtara.workflow.execution.duration"),
                        ("memory", "runtara.workflow.memory.peak"), ("cpu", "runtara.workflow.cpu.usage")]:
        wait_for(lambda: sum(int(p["count"]) for p in latest_points(name)) == usage()[field + "_count"],
                 f"Usage {field} observation parity")
        data = usage()
        observed = sum(float(p["sum"]) for p in latest_points(name))
        # The API preserves its integer-byte mean; rounding can lose <1 byte per observation.
        tolerance = data["memory_count"] if field == "memory" else 1e-6
        assert math.isclose(observed, data[field + "_sum"], abs_tol=tolerance), (field, observed, data)
        for point in latest_points(name):
            assert set(attrs(point)) == {"tenant_id", "status", "termination_reason"}


def main():
    print(f"Test artifacts: {ARTIFACTS}", flush=True)
    for suffix in ("server", "runtime"):
        sql("postgres", f"CREATE DATABASE {TAG}_{suffix}")
    collector = receiver()
    app = server()
    wait_for(lambda: METRICS.exists() and len(METRICS.read_text().splitlines()) >= 2,
             "configured 500ms export cadence", 5)
    docs = http("/openapi/docs.json")
    assert "/api/runtime/analytics/pipeline" not in docs["paths"]
    assert "/api/runtime/analytics/pipeline/stream" not in docs["paths"]
    assert http("/analytics/system")["success"]
    for path in ("/analytics/pipeline", "/analytics/pipeline/stream"):
        try:
            http(path)
            raise AssertionError("Retired pipeline route still responds")
        except urllib.error.HTTPError as error:
            assert error.code == 404
    untracked = workflow(False)
    execute(untracked)
    wait_for(lambda: positive("runtara.runner.runs"), "physical runner metrics")
    assert not positive("runtara.workflow.steps.started"), "Untracked workflows must not fabricate step progress"
    tracked = workflow(True)
    tracked_instance = execute(tracked)
    # Exercise late, duplicate resource reports as well as actual runner observations.
    for _ in range(2):
        sql(f"{TAG}_runtime", f"UPDATE instances SET memory_peak_bytes = coalesce(memory_peak_bytes, 4096), cpu_usage_usec = coalesce(cpu_usage_usec, 500000) WHERE instance_id = '{tracked_instance}'")
    verify_usage_export(2)
    sql(f"{TAG}_runtime", "DELETE FROM instances WHERE status IN ('completed', 'failed', 'cancelled')")
    assert usage()["invocations"] == 2, "Usage must survive raw instance cleanup"
    print("PASS: live Usage and OTLP counts/resources agree and survive raw cleanup", flush=True)
    required = ["runtara.admission.requests", "runtara.admission.duration", "runtara.trigger.events.total",
                "runtara.trigger.processing.duration", "runtara.launch.transitions", "runtara.launch.queue.duration",
                "runtara.pipeline.pool.hold.duration", "runtara.workflow.steps.started"]
    wait_for(lambda: all(positive(name) for name in required), "all pipeline instruments")
    capacities = {attrs(p)["pool"]: int(p["asInt"]) for p in points("runtara.pipeline.pool.capacity")}
    assert capacities["run"] == 2 and capacities["trigger"] == 6, capacities
    for name in required + ["runtara.runner.runs", "runtara.pipeline.pool.usage", "runtara.pipeline.pool.capacity"]:
        for point in points(name):
            assert set(attrs(point)) <= {"pool", "state", "reason", "outcome", "event", "trigger_type", "status"}, name
    print("PASS: real workflows exported bounded OTLP metrics; old routes absent; host API intact", flush=True)
    identities = set()
    for line in METRICS.read_text().splitlines():
        request = json.loads(line)
        for resource in request.get("resourceMetrics", []):
            identity = attrs(resource["resource"]).get("service.instance.id")
            import uuid
            uuid.UUID(identity)
            identities.add(identity)
    assert len(identities) == 1, "One process must retain one resource identity"
    stop(collector)
    shutil.copyfile(METRICS, ARTIFACTS / "metrics-enabled.jsonl")
    execute(tracked)
    before = time.monotonic()
    stop(app)
    assert time.monotonic() - before < 20
    print("PASS: exporter outage does not prevent workflow completion or bounded shutdown", flush=True)
    collector = receiver()  # truncates capture
    app = server(disabled=True)
    execute(tracked)
    wait_for(lambda: usage()["invocations"] == 4, "Usage history with OTEL disabled")
    time.sleep(1.5)
    assert not metric_rows(), "Disabled telemetry must not export"
    stop(app)
    stop(collector)
    print("PASS: disabled telemetry preserves Usage, executes workflows and exports no metrics", flush=True)


if __name__ == "__main__":
    try:
        main()
    finally:
        for process in reversed(PROCESSES):
            if process.poll() is None:
                stop(process)
        print(f"Retained isolated databases {TAG}_server / {TAG}_runtime and {ARTIFACTS}", flush=True)

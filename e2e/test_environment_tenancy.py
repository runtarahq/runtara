#!/usr/bin/env python3
"""Run tenant-isolation checks against an actual local runtara-server.

Requires Docker, a built server, and components from scripts/build-agent-components.sh.
Starts disposable PostgreSQL/Valkey containers on loopback, uses fresh databases,
and never reads repository dotenv files or connects to an existing deployment.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parents[1]


def run(*args, stdin=None):
    return subprocess.run(args, input=stdin, text=True, capture_output=True, check=True).stdout.strip()


def port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def wait_for(predicate, description, timeout=60):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.2)
    raise AssertionError(f"timed out: {description}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/runtara-server")
    parser.add_argument("--components", type=Path, default=ROOT / "target/wasm32-wasip2/release")
    args = parser.parse_args()
    assert args.binary.is_file(), "build runtara-server first"
    assert list(args.components.glob("runtara_agent_*.wasm")), "build agent components first"
    containers = []
    server = None
    directory = Path(tempfile.mkdtemp(prefix="runtara-tenancy-e2e-"))
    server_log = directory / "server.log"
    try:
        pg = run("docker", "run", "--rm", "-d", "-e", "POSTGRES_HOST_AUTH_METHOD=trust", "-p", "127.0.0.1::5432", "pgvector/pgvector:pg16")
        containers.append(pg)
        vk = run("docker", "run", "--rm", "-d", "-p", "127.0.0.1::6379", "valkey/valkey:8-alpine")
        containers.append(vk)
        pg_port = run("docker", "port", pg, "5432/tcp").rsplit(":", 1)[1]
        vk_port = run("docker", "port", vk, "6379/tcp").rsplit(":", 1)[1]
        wait_for(lambda: subprocess.run(["docker", "exec", pg, "pg_isready", "-h", "127.0.0.1", "-U", "postgres"], capture_output=True).returncode == 0, "PostgreSQL readiness")

        def sql(statement, database="runtime"):
            return run("docker", "exec", "-i", pg, "psql", "-U", "postgres", "-d", database, "-v", "ON_ERROR_STOP=1", "-At", stdin=statement)

        sql("CREATE DATABASE server;", "postgres")
        sql("CREATE DATABASE runtime;", "postgres")
        public_port, internal_port, core_port = port(), port(), port()
        tenant_a, tenant_b = "tenancy-e2e-a", "tenancy-e2e-b"
        env = {
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
            "RUNTARA_SERVER_DATABASE_URL": f"postgresql://postgres@127.0.0.1:{pg_port}/server",
            "OBJECT_MODEL_DATABASE_URL": f"postgresql://postgres@127.0.0.1:{pg_port}/server",
            "RUNTARA_DATABASE_URL": f"postgresql://postgres@127.0.0.1:{pg_port}/runtime",
            "TENANT_ID": tenant_a, "SERVER_HOST": "127.0.0.1", "SERVER_PORT": str(public_port),
            "INTERNAL_PORT": str(internal_port), "RUNTARA_CORE_HTTP_PORT": str(core_port),
            "RUNTARA_AGENT_COMPONENTS_DIR": str(args.components.resolve()),
            "DATA_DIR": str(directory / "data"), "AUTH_PROVIDER": "local",
            "VALKEY_HOST": "127.0.0.1", "VALKEY_PORT": vk_port,
            "OTEL_SDK_DISABLED": "true", "RUNTARA_DEV_MODE": "false",
            "RUST_LOG": "warn", "RUNTARA_SDK_BACKEND": "http",
            "RUNTARA_IMAGE_CLEANUP_POLL_INTERVAL_SECS": "1", "RUNTARA_IMAGE_CLEANUP_MAX_AGE_DAYS": "1",
        }
        base = f"http://127.0.0.1:{public_port}"

        def http(path, method="GET", body=None):
            request = urllib.request.Request(base + path, method=method, data=json.dumps(body).encode() if body is not None else None, headers={"Content-Type": "application/json"})
            try:
                with urllib.request.urlopen(request, timeout=5) as response:
                    return response.status, response.read().decode()
            except urllib.error.HTTPError as error:
                return error.code, error.read().decode()

        def healthy():
            assert server.poll() is None, f"server exited; private log: {server_log}"
            try:
                return http("/health")[0] == 200
            except (OSError, urllib.error.URLError):
                return False

        def start():
            with server_log.open("ab") as log:
                return subprocess.Popen([str(args.binary.resolve())], cwd=directory, env=env, stdout=log, stderr=subprocess.STDOUT)

        def stop(process):
            process.terminate()
            try:
                process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)

        server = start()
        wait_for(healthy, "server health")
        ids = {key: str(uuid.uuid4()) for key in ["a", "b", "missing", "pending_a", "pending_b", "image_a", "image_b", "launch_b"]}
        for tenant, key in [(tenant_a, "a"), (tenant_b, "b")]:
            image, instance = ids[f"image_{key}"], ids[key]
            marker = "owned-input" if key == "a" else "foreign-private-input"
            sql(f"""INSERT INTO images (image_id, tenant_id, name, binary_path) VALUES ('{image}', '{tenant}', 'tenancy-workflow:1', '/fixture');
                INSERT INTO instances (instance_id, tenant_id, status, input, created_at) VALUES ('{instance}', '{tenant}', 'suspended', convert_to('{json.dumps({'marker': marker})}', 'UTF8'), NOW());
                INSERT INTO instance_images (instance_id, image_id, tenant_id) VALUES ('{instance}', '{image}', '{tenant}');
                INSERT INTO instances (instance_id, tenant_id, status, created_at) VALUES ('{ids[f'pending_{key}']}', '{tenant}', 'pending', NOW() - INTERVAL '1 hour');""")
        sql(f"""INSERT INTO instance_launches (launch_id, instance_id, tenant_id, image_id, kind, state, available_at, deadline_at)
            VALUES ('{ids['launch_b']}', '{ids['pending_b']}', '{tenant_b}', '{ids['image_b']}', 'start', 'queued', NOW(), NOW() + INTERVAL '1 hour');""")
        code, body = http("/api/runtime/executions?size=100")
        assert code == 200, f"execution listing: {code}"
        page = json.loads(body)["data"]
        assert page["totalElements"] == 2, page
        assert ids["b"] not in body and tenant_b not in body and "foreign-private-input" not in body

        # Known foreign IDs behave like missing IDs through the HTTP adapters.
        paths = [
            "/api/runtime/workflows/instances/{id}",
            "/api/runtime/workflows/tenancy-workflow/instances/{id}/step-events",
            "/api/runtime/workflows/tenancy-workflow/instances/{id}/steps",
            "/api/runtime/workflows/tenancy-workflow/instances/{id}/checkpoints",
        ]
        for path in paths:
            foreign = http(path.format(id=ids["b"]))
            missing = http(path.format(id=ids["missing"]))
            assert foreign[0] == missing[0] == 404, (path, foreign[0], missing[0])
            assert "foreign-private-input" not in foreign[1] and tenant_b not in foreign[1]
        own = http(paths[0].format(id=ids["a"]))
        assert own[0] == 200, f"owned instance lookup: {own[0]}"
        for key in ["b", "missing"]:
            code, _ = http(f"/api/runtime/workflows/instances/{ids[key]}/stop", "POST", {})
            assert code == 404, f"foreign/missing stop: {code}"
        assert http(f"/api/runtime/workflows/instances/{ids['a']}/stop", "POST", {})[0] == 200
        assert sql(f"SELECT status FROM instances WHERE instance_id = '{ids['b']}';") == "suspended"
        assert sql(f"SELECT status FROM instances WHERE instance_id = '{ids['a']}';") == "cancelled"

        # Disk cleanup must not enumerate another tenant's namespace.
        paths_by_tenant = {}
        for tenant in [tenant_a, tenant_b]:
            tenant_root = directory / "data/tenants" / hashlib.sha256(tenant.encode()).hexdigest()
            old_run = tenant_root / "runs/old-fixture"
            old_run.mkdir(parents=True)
            os.utime(old_run, (time.time() - 864000, time.time() - 864000))
            image_id = str(uuid.uuid4())
            image_file = tenant_root / "images" / hashlib.sha256(image_id.encode()).hexdigest() / "binary"
            sql(f"INSERT INTO images (image_id, tenant_id, name, binary_path, updated_at) VALUES ('{image_id}', '{tenant}', 'stale-fixture', '{image_file}', NOW() - INTERVAL '10 days');")
            image_file.parent.mkdir(parents=True)
            image_file.write_bytes(b"private artifact")
            paths_by_tenant[tenant] = old_run, image_file, image_id
        wait_for(lambda: not paths_by_tenant[tenant_a][1].exists(), "owned image cleanup")
        assert paths_by_tenant[tenant_b][0].exists() and paths_by_tenant[tenant_b][1].exists()
        assert sql(f"SELECT COUNT(*) FROM images WHERE image_id = '{paths_by_tenant[tenant_b][2]}';") == "1"

        # Run cleanup has an eager startup pass and a long default interval.
        # Restart also must fail only A's abandoned pending instance.
        stop(server)
        server = start()
        wait_for(healthy, "server restart")
        wait_for(lambda: not paths_by_tenant[tenant_a][0].exists(), "owned run cleanup")
        assert sql(f"SELECT status FROM instances WHERE instance_id = '{ids['pending_a']}';") == "failed"
        assert sql(f"SELECT status FROM instances WHERE instance_id = '{ids['pending_b']}';") == "pending"
        assert sql(f"SELECT state FROM instance_launches WHERE launch_id = '{ids['launch_b']}';") == "queued"
        assert paths_by_tenant[tenant_b][0].exists() and paths_by_tenant[tenant_b][1].exists()
        print("PASS: local HTTP reads/control, tenant listing, artifact cleanup, dispatch isolation, and restart recovery")
    finally:
        print(f"Private server log: {server_log}", flush=True)
        if server is not None and server.poll() is None:
            server.terminate()
            try:
                server.wait(timeout=15)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait(timeout=5)
        for container in reversed(containers):
            subprocess.run(["docker", "stop", container], capture_output=True, check=False)


if __name__ == "__main__":
    main()

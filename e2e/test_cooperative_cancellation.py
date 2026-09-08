#!/usr/bin/env python3
"""Authenticated cooperative cancellation through an isolated real server.

Requires Docker (pgvector:pg18 and valkey:8-alpine), Python cryptography, a built
runtara-server binary and built Agent components. All credentials are generated
in memory. Only newly created processes/containers are stopped; databases and
logs are retained. This does not read repository dotenv files or existing keys.

Progress snapshot: single-server cases pass; starting the peer currently replaces
the live owner's generation before Stop. See AUDIT-23 in docs/wasm-emitter-audit.md.
"""
import argparse
import base64
import http.server
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request
import uuid

from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.asymmetric import padding, rsa


def eventually(check, seconds=30):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(0.1)
    raise AssertionError("Timed out waiting for fixture condition")


def free_port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def b64(value):
    return base64.urlsafe_b64encode(value).rstrip(b"=").decode()


def json_bytes(value):
    return json.dumps(value, separators=(",", ":")).encode()


class Fixture(http.server.ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self):
        self.key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
        self.calls = {}
        self.unexpected_calls = 0
        self.lock = threading.Lock()
        super().__init__(("127.0.0.1", 0), FixtureHandler)
        self.url = f"http://127.0.0.1:{self.server_port}"
        self.thread = threading.Thread(target=self.serve_forever, daemon=True)
        self.thread.start()

    def token(self, tenant, user="cancel-owner"):
        header = b64(json_bytes({"alg": "RS256", "kid": "cancellation-fixture", "typ": "JWT"}))
        claims = b64(json_bytes({"sub": user, "org_id": tenant, "jti": uuid.uuid4().hex,
                                 "iss": self.url, "aud": "cancellation-api", "exp": int(time.time()) + 3600}))
        message = f"{header}.{claims}".encode()
        signature = self.key.sign(message, padding.PKCS1v15(), hashes.SHA256())
        return f"{header}.{claims}.{b64(signature)}"

    def new_call(self, mode):
        ident = uuid.uuid4().hex
        call = {"started": threading.Event(), "closed": threading.Event(), "requests": 0,
                "mode": mode}
        with self.lock:
            self.calls[ident] = call
        return f"{self.url}/hang/{ident}", call

    def close(self):
        self.shutdown()
        self.server_close()
        self.thread.join(timeout=5)


class FixtureHandler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        if self.path == "/jwks":
            key = self.server.key.public_key().public_numbers()
            key_bytes = lambda n: n.to_bytes((n.bit_length() + 7) // 8, "big")
            body = json_bytes({"keys": [{"kty": "RSA", "use": "sig", "alg": "RS256",
                                         "kid": "cancellation-fixture", "n": b64(key_bytes(key.n)),
                                         "e": b64(key_bytes(key.e))}]})
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if self.path == "/unexpected":
            with self.server.lock:
                self.server.unexpected_calls += 1
            self.send_response(200)
            self.send_header("Content-Length", "2")
            self.end_headers()
            self.wfile.write(b"{}")
            return
        ident = self.path.removeprefix("/hang/")
        with self.server.lock:
            call = self.server.calls.get(ident)
            if call:
                call["requests"] += 1
        if not call:
            self.send_error(404)
            return
        if call["mode"] == "body":
            self.send_response(200)
            self.send_header("Content-Length", "1048576")
            self.end_headers()
            self.wfile.write(b"partial")
            self.wfile.flush()
        call["started"].set()
        self.connection.settimeout(0.2)
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            try:
                if not self.connection.recv(1):
                    call["closed"].set()
                    return
            except socket.timeout:
                continue
            except (ConnectionResetError, BrokenPipeError):
                call["closed"].set()
                return


class TestServer:
    def __init__(self, args):
        self.args = args
        self.ident = uuid.uuid4().hex[:12]
        self.tenant = "cooperative-" + self.ident
        self.directory = Path(tempfile.mkdtemp(prefix="runtara-cooperative-api-"))
        self.containers = []
        self.processes = []
        self.fixture = Fixture()
        self.token = self.fixture.token(self.tenant)
        self.base = None

    def command(self, *args, input=None):
        return subprocess.check_output(args, input=input, text=True, stderr=subprocess.PIPE).strip()

    def container(self, image, port, *options):
        name = "runtara-cancel-api-" + str(port) + "-" + self.ident
        self.command("docker", "run", "-d", "--name", name, "-p", f"127.0.0.1::{port}", *options, image)
        self.containers.append(name)
        bound = self.command("docker", "port", name, f"{port}/tcp")
        return name, int(bound.rsplit(":", 1)[1])

    def sql(self, database, query):
        raw = self.command("docker", "exec", "-i", self.postgres, "psql", "-U", "postgres",
                           "-d", database, "-X", "-A", "-t", "-v", "ON_ERROR_STOP=1", input=query)
        return json.loads(raw) if raw else None

    def start(self):
        self.postgres, pg_port = self.container("pgvector/pgvector:pg18", 5432,
                                                "-e", "POSTGRES_HOST_AUTH_METHOD=trust")
        def ready():
            return subprocess.run(["docker", "exec", self.postgres, "pg_isready", "-h", "127.0.0.1", "-U", "postgres"],
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0
        eventually(ready)
        for database in ["server", "runtime", "objects"]:
            self.command("docker", "exec", self.postgres, "createdb", "-U", "postgres", database)
        self.valkey, valkey_port = self.container("valkey/valkey:8-alpine", 6379)
        eventually(lambda: subprocess.run(["docker", "exec", self.valkey, "valkey-cli", "PING"],
                                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0)
        self.command("docker", "exec", self.valkey, "valkey-cli", "SET", "member:cancel-owner", '{"role":"owner"}')
        self.command("docker", "exec", self.valkey, "valkey-cli", "SET", "member:cancel-viewer", '{"role":"viewer"}')
        # Construct isolated DB endpoints in memory; never print or persist them.
        database_url = lambda name: "postgresql://postgres@127.0.0.1:" + str(pg_port) + "/" + name
        env = {name: os.environ[name] for name in ["PATH", "HOME", "TMPDIR"] if name in os.environ}
        env.update({
            "RUNTARA_SERVER_DATABASE_URL": database_url("server"),
            "RUNTARA_DATABASE_URL": database_url("runtime"),
            "OBJECT_MODEL_DATABASE_URL": database_url("objects"),
            "VALKEY_HOST": "127.0.0.1", "VALKEY_PORT": str(valkey_port),
            "AUTH_PROVIDER": "oidc", "RUNTARA_AUTH_MEMBERSHIP_POLICY": "required",
            "RUNTARA_AUTH_REQUIRE_JTI": "true", "TENANT_ID": self.tenant,
            "OAUTH2_JWKS_URI": self.fixture.url + "/jwks", "OAUTH2_ISSUER": self.fixture.url,
            "OAUTH2_AUDIENCE": "cancellation-api", "OAUTH2_MCP_AUDIENCE": "cancellation-mcp",
            "SERVER_HOST": "127.0.0.1", "INTERNAL_HOST": "127.0.0.1", "RUNTARA_EMBEDDED": "true",
            "RUNTARA_AGENT_COMPONENTS_DIR": str(self.args.components.resolve()),
            "DATA_DIR": str(self.directory / "data"), "OTEL_SDK_DISABLED": "true",
            "RUNTARA_PROXY_ALLOWED_HOSTS": "127.0.0.1", "MAX_CONCURRENT_EXECUTIONS": "2",
            "RUNTARA_PRICING_TIER": "enterprise", "RUST_LOG": "warn",
        })
        self.server_env = env
        self.base, _ = self.launch_server("owner")

    def launch_server(self, label):
        env = self.server_env.copy()
        ports = set()
        while len(ports) < 3:
            ports.add(free_port())
        public, internal, core = sorted(ports)
        base = f"http://127.0.0.1:{public}"
        env.update(SERVER_PORT=str(public), INTERNAL_PORT=str(internal), RUNTARA_CORE_HTTP_PORT=str(core))
        directory = self.directory / label
        directory.mkdir()
        # Prevent dotenv's parent-directory search from reaching real settings.
        (directory / ".env").touch(mode=0o600, exist_ok=False)
        log = open(directory / "server.log", "wb")
        process = subprocess.Popen([str(self.args.server.resolve())], cwd=directory, env=env,
                                   stdout=log, stderr=subprocess.STDOUT)
        log.close()
        self.processes.append(process)
        def api_ready():
            assert process.poll() is None, "server exited; inspect the isolated server log"
            try:
                status, _ = self.request("/api/runtime/workflows/instances/" + str(uuid.uuid4()) + "/stop", {}, base=base)
                return status == 404
            except (urllib.error.URLError, ConnectionError):
                return False
        eventually(api_ready, 120)
        print(f"Authenticated isolated server ready: {label}", flush=True)
        return base, process

    def request(self, path, body=None, token=True, base=None):
        headers = {"Content-Type": "application/json"}
        if token is True:
            headers["Authorization"] = "Bearer " + self.token
        elif isinstance(token, str):
            headers["Authorization"] = "Bearer " + token
        request = urllib.request.Request((base or self.base) + path, data=None if body is None else json_bytes(body), headers=headers)
        try:
            response = urllib.request.urlopen(request, timeout=120)
        except urllib.error.HTTPError as error:
            response = error
        raw = response.read()
        try:
            body = json.loads(raw) if raw else None
        except json.JSONDecodeError:
            body = {"error": raw.decode(errors="replace")[:1000]}
        return response.status, body

    def api(self, path, body=None):
        status, result = self.request("/api/runtime" + path, body)
        assert status == 200, (path, status, result)
        assert result.get("success", True), (path, result)
        return result.get("data", result)

    def state(self, ident):
        ident = str(uuid.UUID(ident))
        return self.sql("runtime", f"""SELECT json_build_object(
            'status', status, 'reason', termination_reason,
            'pending', (SELECT count(*) FROM pending_signals WHERE instance_id='{ident}' AND acknowledged_at IS NULL),
            'launches', (SELECT count(*) FROM instance_launches WHERE instance_id='{ident}'),
            'recovery_attempts', recovery_attempts,
            'run', (SELECT json_build_object('launch_id', launch_id, 'handle_id', container_id)
                    FROM container_registry WHERE instance_id='{ident}'),
            'registry', (SELECT count(*) FROM container_registry WHERE instance_id='{ident}'))
            FROM instances WHERE instance_id='{ident}';""")

    def execute(self, url):
        workflow = self.api("/workflows/create", {"name": "Cooperative HTTP " + uuid.uuid4().hex, "description": "Authenticated cooperative cancellation E2E"})["id"]
        graph = {"name": "Authenticated cancellation", "durable": True, "entryPoint": "fetch",
                 "steps": {"fetch": {"id": "fetch", "stepType": "Agent", "agentId": "http",
                                      "capabilityId": "http-request", "maxRetries": 3,
                                      "inputMapping": {"url": {"valueType": "immediate", "value": url},
                                                       "timeout_ms": {"valueType": "immediate", "value": 90000}}},
                           "finish": {"id": "finish", "stepType": "Finish"},
                           "recovery": {"id": "recovery", "stepType": "Finish"}},
                 "executionPlan": [{"fromStep": "fetch", "toStep": "finish"},
                                   {"fromStep": "fetch", "toStep": "recovery", "label": "onError"}],
                 "variables": {}, "inputSchema": {}, "outputSchema": {}}
        for step in ["after", "recovery"]:
            graph["steps"][step] = {"id": step, "stepType": "Agent", "agentId": "http",
                                     "capabilityId": "http-request", "maxRetries": 0,
                                     "inputMapping": {"url": {"valueType": "immediate", "value": self.fixture.url + "/unexpected"}}}
            graph["executionPlan"].append({"fromStep": step, "toStep": "finish"})
        graph["executionPlan"][0]["toStep"] = "after"
        self.api(f"/workflows/{workflow}/update", {"executionGraph": graph})
        versions = self.api(f"/workflows/{workflow}/versions")
        version = max(v.get("version", v.get("versionNumber", 1)) for v in versions)
        self.api(f"/workflows/{workflow}/versions/{version}/compile", {})
        return self.api(f"/workflows/{workflow}/execute", {"inputs": {"data": {}}})["instanceId"]

    def run(self):
        self.start()
        for mode, cross_owner in [("headers", False), ("body", False), ("headers", True), ("body", True)]:
            url, call = self.fixture.new_call(mode)
            ident = self.execute(url)
            assert call["started"].wait(30), "workflow never reached controlled HTTP"
            running = self.state(ident)
            assert running["status"] == "running" and running["run"], running
            # Start the peer only after the owner has dispatched this request,
            # so the execution cannot accidentally be claimed by the peer.
            stop_base, peer = self.launch_server("peer-" + mode) if cross_owner else (self.base, None)
            if cross_owner:
                after_start = self.state(ident)
                for key in ["status", "run", "launches", "recovery_attempts", "pending"]:
                    assert after_start[key] == running[key], (
                        "peer startup changed the live execution before Stop", key,
                        running[key], after_start[key])
            if cross_owner and mode == "headers":
                # Hold beyond a complete 30-second running lease, then shut
                # down an unrelated peer. Neither may retire the owner's run.
                def owner_lease():
                    return self.sql("runtime", f"""SELECT json_build_object(
                        'owner', lease_owner, 'expiry', EXTRACT(EPOCH FROM lease_expires_at))
                        FROM instance_launches WHERE launch_id='{running["run"]["launch_id"]}';""")
                initial_lease = owner_lease()
                assert initial_lease["owner"] and initial_lease["expiry"], initial_lease
                time.sleep(32)
                renewed_lease = owner_lease()
                assert renewed_lease["owner"] == initial_lease["owner"], "live execution changed owners"
                assert renewed_lease["expiry"] > initial_lease["expiry"], "running lease was not renewed"
                _, drain_peer = self.launch_server("drain-probe")
                self.stop_process(drain_peer)
                after_drain = self.state(ident)
                for key in ["status", "run", "launches", "recovery_attempts", "pending"]:
                    assert after_drain[key] == running[key], (
                        "renewal or peer drain changed the live execution", key,
                        running[key], after_drain[key])
                assert not call["closed"].is_set(), "renewal or peer drain closed the owner's HTTP"
                print("PASS sustained owner renewal and unrelated peer shutdown", flush=True)
            path = f"/api/runtime/workflows/instances/{ident}/stop"
            for label, token, expected in [
                ("missing", False, 401),
                ("invalid", "invalid", 401),
                ("wrong tenant", self.fixture.token("another-tenant"), 403),
                ("viewer", self.fixture.token(self.tenant, "cancel-viewer"), 403),
            ]:
                status, _ = self.request(path, {}, token, base=stop_base)
                assert status == expected, (label, status)
                assert not call["closed"].is_set(), "rejected request cancelled I/O"
                assert self.state(ident)["pending"] == 0, "rejected request persisted cancellation"
            started = time.monotonic()
            status, _ = self.request(path, {}, base=stop_base)
            assert status == 200, ("cross_owner", cross_owner, "status", status)
            assert call["closed"].wait(4), "cooperative stop did not close local HTTP promptly"
            def stopped():
                state = self.state(ident)
                assert state["status"] not in ["completed", "failed"], state
                return state if state["status"] == "cancelled" and state["registry"] == 0 else None
            state = eventually(stopped, 10)
            assert state["pending"] == 0, state
            assert state["reason"] != "aborted", "emergency abort cannot establish cooperative cleanup"
            assert call["requests"] == 1, "cancellation retried the request"
            assert self.fixture.unexpected_calls == 0, "cancellation ran success or error continuation"
            status, _ = self.request(path, {}, base=stop_base)
            assert status == 200, "duplicate Stop must remain idempotent"
            assert self.state(ident)["launches"] == state["launches"], "duplicate Stop relaunched workflow"
            print(f"PASS authenticated {mode} cancellation cross_owner={cross_owner} ({time.monotonic()-started:.3f}s); auth rejection and duplicate Stop", flush=True)
            if peer:
                self.stop_process(peer)

    @staticmethod
    def stop_process(process):
        if process.poll() is None:
            process.send_signal(signal.SIGTERM)
            try:
                process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)

    def close(self):
        for process in reversed(self.processes):
            self.stop_process(process)
        self.fixture.close()
        for name in reversed(self.containers):
            subprocess.run(["docker", "stop", "--time", "5", name], stdout=subprocess.DEVNULL, check=False)
        print(f"Isolated logs and retained data: {self.directory}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, required=True)
    parser.add_argument("--components", type=Path, required=True)
    args = parser.parse_args()
    assert args.server.is_file(), "build runtara-server first"
    assert args.components.is_dir(), "build Agent components first"
    test = TestServer(args)
    try:
        test.run()
    finally:
        test.close()


if __name__ == "__main__":
    main()

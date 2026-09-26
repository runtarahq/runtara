#!/usr/bin/env python3
"""Isolated local-server E2E for workflow-owned Slack/Mailgun attachments.

Requires Linux/OpenSSL, Docker (pgvector/pgvector:pg18), redis-server, a built runtara-server,
and scripts/build-agent-components.sh output. Uses synthetic connections and a
loopback HTTPS provider fixture; never contacts Slack, Mailgun, or real storage.
The fixture CA is trusted only by the child process through SSL_CERT_FILE.
Only the processes/container created here are stopped. Logs are retained in /tmp.
"""
import base64
import hashlib
import hmac
import json
import os
from pathlib import Path
import secrets
import socket
import ssl
import sys
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.error import HTTPError, URLError
from urllib.request import Request, build_opener, ProxyHandler
from urllib.parse import urlsplit, parse_qs

ROOT = Path(__file__).resolve().parents[1]
CONTENT = b"\x00\xffworkflow attachment\x80"
FILE_URL = "https://files.slack.com/files-pri/T-F/invoice.bin"
MESSAGE_URL = "https://storage-us-west1.api.mailgun.net/v3/domains/inbound.example/messages/key"
REQUESTS = []
UPLOADED = []
OPENER = build_opener(ProxyHandler({}))


def check(value, message):
    if not value:
        raise AssertionError(message)


def request(url, payload=None, headers=None, raw=None):
    headers = dict(headers or {})
    if payload is not None:
        raw = json.dumps(payload).encode()
        headers["Content-Type"] = "application/json"
    req = Request(url, data=raw, headers=headers)
    try:
        with OPENER.open(req, timeout=180) as response:
            body = response.read()
            return json.loads(body) if body else None
    except HTTPError as error:
        # Avoid dumping payloads/connection values in test output.
        detail = error.read().decode() if "/workflows/" in url else ""
        raise AssertionError(f"HTTP {error.code} from {req.get_method()} {req.full_url.split('?')[0]} {detail}") from None


class Provider(BaseHTTPRequestHandler):
    tls_context = None
    allowed_tls_hosts = {"slack.com", "files.slack.com", "storage-us-west1.api.mailgun.net", "api.mailgun.net"}

    def log_message(self, *_):
        pass

    def do_CONNECT(self):
        # Terminate TLS locally for these synthetic providers. Never tunnel onward.
        host, _, port = self.path.rpartition(":")
        if host not in self.allowed_tls_hosts or port != "443":
            self.send_error(502)
            return
        self.send_response(200, "Connection established")
        self.end_headers()
        self.wfile.flush()
        self.connection = self.tls_context.wrap_socket(self.connection, server_side=True)
        self.rfile = self.connection.makefile("rb", self.rbufsize)
        self.wfile = self.connection.makefile("wb", self.wbufsize)
        self.handle_one_request()
        self.close_connection = True

    def provider_request(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        parsed = urlsplit(self.path)
        path = parsed.path
        REQUESTS.append({"method": self.command, "path": path})
        status, response = 200, b""
        headers = {"Content-Type": "application/octet-stream"}
        if path == "/bounded":
            response = bytes(9 * 1024 * 1024)
        elif path == "/redirect":
            status, headers = 302, {"Location": "/must-not-follow"}
        elif path.endswith("/files.info"):
            args = json.loads(body) if body else {key: values[0] for key, values in parse_qs(parsed.query).items()}
            response = json.dumps({"ok": True, "file": {"id": args["file"], "name": "invoice.bin", "mimetype": "application/octet-stream", "size": len(CONTENT), "url_private_download": FILE_URL}}).encode()
        elif path == urlsplit(MESSAGE_URL).path:
            headers = {"Content-Type": "application/json"}
            response = json.dumps({"body-plain": "email text", "body-html": "<p>email text</p>", "attachments": [{"name": "invoice.bin", "url": MESSAGE_URL + "/attachments/0"}]}).encode()
        elif path in [urlsplit(FILE_URL).path, urlsplit(MESSAGE_URL).path + "/attachments/0"]:
            response = CONTENT
        elif self.command == "PUT" and path == "/attachment-test/invoice.bin":
            UPLOADED.append(body)
        else:
            status = 404
        self.send_response(status)
        for name, value in headers.items():
            self.send_header(name, value)
        self.send_header("Content-Length", str(len(response)))
        self.send_header("Connection", "close")
        self.end_headers()
        try:
            self.wfile.write(response)
        except (BrokenPipeError, ConnectionResetError):
            # The host can reject Content-Length before consuming the oversized body.
            if path != "/bounded":
                raise

    do_GET = provider_request
    do_POST = provider_request
    do_PUT = provider_request


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def wait_until(fn, seconds=90):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        result = fn()
        if result:
            return result
        time.sleep(0.25)
    raise AssertionError("Timed out waiting for local test condition")


def main():
    check(sys.platform.startswith("linux"), "This HTTPS fixture requires Linux/OpenSSL SSL_CERT_FILE trust")
    binary = Path(os.environ.get("RUNTARA_SERVER_BIN", ROOT / "target/debug/runtara-server")).resolve()
    components = Path(os.environ.get("RUNTARA_AGENT_COMPONENTS_DIR", ROOT / "target/wasm32-wasip2/release")).resolve()
    check(binary.is_file(), "Build runtara-server first")
    for name in ["runtara_agent_slack", "runtara_agent_mailgun", "runtara_agent_s3_storage", "runtara_workflow_stdlib", "runtara_workflow_runtime"]:
        check((components / (name + ".wasm")).is_file(), "Build agent components first")
    work = Path(tempfile.mkdtemp(prefix="runtara-attachments-e2e-"))
    print(f"Test logs: {work}", flush=True)
    container = "runtara-attachments-" + secrets.token_hex(5)
    children = []
    ca = work / "fixture-ca.pem"
    key = work / "fixture-key.pem"
    subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                    "-subj", "/CN=RuntaraOutboundFixture", "-addext",
                    "subjectAltName=" + ",".join("DNS:" + host for host in sorted(Provider.allowed_tls_hosts)),
                    "-keyout", str(key), "-out", str(ca)], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    key.chmod(0o600)
    Provider.tls_context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    Provider.tls_context.load_cert_chain(ca, key)
    provider = ThreadingHTTPServer(("127.0.0.1", 0), Provider)
    threading.Thread(target=provider.serve_forever, daemon=True).start()
    mock = f"http://127.0.0.1:{provider.server_port}"
    env = os.environ.copy()
    # Never read the workspace .env; the server runs from the isolated directory.
    env.update({"POSTGRES_PASSWORD": secrets.token_hex(24)})
    try:
        subprocess.run(["docker", "run", "--rm", "-d", "--name", container, "-p", "127.0.0.1::5432", "-e", "POSTGRES_PASSWORD", "pgvector/pgvector:pg18"], env=env, check=True, stdout=subprocess.DEVNULL)
        port = subprocess.check_output(["docker", "port", container, "5432"], text=True).strip().rsplit(":", 1)[1]
        wait_until(lambda: subprocess.run(["docker", "exec", container, "pg_isready", "-U", "postgres"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0)
        for db in ["attachment_server", "attachment_runtime"]:
            subprocess.run(["docker", "exec", container, "psql", "-U", "postgres", "-c", f"CREATE DATABASE {db}"], check=True, stdout=subprocess.DEVNULL)
        subprocess.run(["docker", "exec", container, "psql", "-U", "postgres", "-d", "attachment_server", "-c", "CREATE EXTENSION vector; CREATE EXTENSION pg_trgm; CREATE EXTENSION fuzzystrmatch;"], check=True, stdout=subprocess.DEVNULL)
        ports = [free_port() for _ in range(6)]
        public, core, environment, core_http, env_http, redis = ports
        redis_log = open(work / "redis.log", "w")
        children.append(subprocess.Popen(["redis-server", "--bind", "127.0.0.1", "--port", str(redis), "--save", "", "--appendonly", "no"], cwd=work, stdout=redis_log, stderr=subprocess.STDOUT))
        dbbase = f"postgresql://postgres:{env['POSTGRES_PASSWORD']}@127.0.0.1:{port}/"
        env.update({
            "RUSTC_WRAPPER": "", "RUNTARA_SERVER_DATABASE_URL": dbbase + "attachment_server", "OBJECT_MODEL_DATABASE_URL": dbbase + "attachment_server", "RUNTARA_DATABASE_URL": dbbase + "attachment_runtime",
            "TENANT_ID": "attachments-e2e", "SERVER_HOST": "127.0.0.1", "SERVER_PORT": str(public), "RUNTARA_CORE_PORT": str(core), "RUNTARA_ENVIRONMENT_PORT": str(environment), "RUNTARA_CORE_HTTP_PORT": str(core_http), "RUNTARA_ENV_HTTP_PORT": str(env_http),
            "RUNTARA_AGENT_COMPONENTS_DIR": str(components), "DATA_DIR": str(work / "data"), "AUTH_PROVIDER": "local", "SESSION_TOKEN_SECRET": secrets.token_hex(32),
            "VALKEY_HOST": "127.0.0.1", "VALKEY_PORT": str(redis), "OTEL_SDK_DISABLED": "true", "RUNTARA_SDK_BACKEND": "http", "SQLX_OFFLINE": "true", "RUST_LOG": "warn",
            "RUNTARA_PROXY_ALLOWED_HOSTS": "127.0.0.1", "RUNTARA_PROXY_ALLOW_HTTP_HOSTS": "127.0.0.1", "RUNTARA_CONNECTION_ALLOW_HTTP_HOSTS": "127.0.0.1",
            "HTTPS_PROXY": mock, "HTTP_PROXY": mock, "NO_PROXY": "127.0.0.1,localhost",
            "SSL_CERT_FILE": str(ca),
        })
        server_log = open(work / "server.log", "w")
        server = subprocess.Popen([str(binary)], env=env, cwd=work, stdout=server_log, stderr=subprocess.STDOUT)
        children.append(server)
        base = f"http://127.0.0.1:{public}"
        api = base + "/api/runtime"

        def healthy():
            check(server.poll() is None, "Local server exited; inspect retained server log")
            try:
                request(base + "/health")
                return True
            except (URLError, AssertionError, json.JSONDecodeError):
                return False
        wait_until(healthy, 180)
        print("Local server ready", flush=True)

        def connection(integration, parameters, **extra):
            response = request(api + "/connections", {"title": "Attachment E2E " + integration, "integrationId": integration, "connectionParameters": parameters, **extra})
            return response.get("connectionId") or response.get("id") or response.get("data", {}).get("id")

        slack_key = secrets.token_hex(32)
        mailgun_key = secrets.token_hex(32)
        slack = connection("slack_bot", {"bot_token": secrets.token_hex(24), "signing_secret": slack_key})
        mailgun = connection("mailgun", {"api_key": secrets.token_hex(24), "webhook_signing_key": mailgun_key, "domain": "inbound.example", "region": "us"})
        storage = connection("s3_compatible", {"endpoint": mock, "access_key_id": secrets.token_hex(12), "secret_access_key": secrets.token_hex(24), "region": "us-east-1", "path_style": True}, defaultFor=["object_storage"])
        check(slack and mailgun and storage, "Connections must be created")

        def workflow(name, steps, entry, edges, input_schema=None):
            created = request(api + "/workflows/create", {"name": name, "description": "Attachment E2E"})
            wid = created["data"]["id"]
            graph = {"name": name, "steps": steps, "entryPoint": entry, "executionPlan": edges, "variables": {}, "inputSchema": input_schema or {}, "outputSchema": {}}
            updated = request(api + f"/workflows/{wid}/update", {"executionGraph": graph})
            check(updated.get("success"), "Workflow update failed")
            versions = request(api + f"/workflows/{wid}/versions")["data"]
            version = max(v.get("version", v.get("versionNumber", 1)) for v in versions)
            compiled = request(api + f"/workflows/{wid}/versions/{version}/compile", {})
            check(compiled.get("success"), "Workflow compile failed")
            return wid

        finish = {"stepType": "Finish", "id": "finish", "inputMapping": {"result": {"valueType": "immediate", "value": "received"}}}
        wid = workflow("Attachment pass-through", {"finish": finish}, "finish", [])
        for conn in [slack, mailgun]:
            request(api + "/triggers", {"workflow_id": wid, "trigger_type": "CHANNEL", "active": True, "configuration": {"connection_id": conn, "session_mode": "per_message"}})

        def instance_list(workflow_id):
            response = request(api + f"/workflows/{workflow_id}/instances?size=100")
            data = response.get("data", {})
            return data if isinstance(data, list) else data.get("content", data.get("items", []))

        def completed(workflow_id, count):
            rows = instance_list(workflow_id)
            if len(rows) < count:
                return False
            details = [request(api + f"/workflows/instances/{row['id']}?full=true")["data"] for row in rows]
            check(not any(str(row["status"]).lower() == "failed" for row in details), "Workflow instance failed")
            return details if len(details) == count and all(str(row["status"]).lower() == "completed" for row in details) else False

        slack_event = {"type": "event_callback", "event_id": "attachment-event", "event": {"type": "message", "subtype": "file_share", "channel": "C-test", "user": "U-test", "files": [{"id": "F-test", "name": "invoice.bin", "size": len(CONTENT), "mimetype": "application/octet-stream", "url_private": FILE_URL}]}}
        slack_body = json.dumps(slack_event).encode()
        timestamp = str(int(time.time()))
        signature = "v0=" + hmac.new(slack_key.encode(), b"v0:" + timestamp.encode() + b":" + slack_body, hashlib.sha256).hexdigest()
        slack_headers = {"Content-Type": "application/json", "X-Slack-Request-Timestamp": timestamp, "X-Slack-Signature": signature}
        request(api + f"/events/webhook/slack/{slack}", raw=slack_body, headers=slack_headers)

        def mailgun_fields(event_id, **fields):
            token = secrets.token_hex(16)
            timestamp = str(int(time.time()))
            return {"sender": "sender@example.test", "Message-Id": event_id, "timestamp": timestamp, "token": token, "signature": hmac.new(mailgun_key.encode(), (timestamp + token).encode(), hashlib.sha256).hexdigest(), **fields}
        mail_event = mailgun_fields("stored-message", **{"message-url": MESSAGE_URL, "unknown": {"nested": [1, 2]}})
        request(api + f"/events/webhook/mailgun/{mailgun}", mail_event)
        parts = []
        for key, value in mailgun_fields("multipart-message").items():
            parts.append(f'--boundary\r\nContent-Disposition: form-data; name="{key}"\r\n\r\n{value}\r\n'.encode())
        parts.append(b'--boundary\r\nContent-Disposition: form-data; name="attachment-1"; filename="invoice.bin"\r\nContent-Type: application/octet-stream\r\n\r\n' + CONTENT + b'\r\n--boundary--\r\n')
        request(api + f"/events/webhook/mailgun/{mailgun}", raw=b"".join(parts), headers={"Content-Type": "multipart/form-data; boundary=boundary"})
        instances = wait_until(lambda: completed(wid, 3), 120)
        check(not REQUESTS, "Ingestion or pass-through workflow made unexpected provider/storage requests")
        inputs = [row["inputs"]["data"] for row in instances]
        slack_input = next(row for row in inputs if row["channel"] == "slack")
        check(slack_input["originalMessage"] == slack_event, "Slack raw event changed")
        check(slack_input["attachments"][0]["id"] == "F-test", "Slack file ID missing")
        check(slack_input["sourceConnectionId"] == slack, "Source connection missing")
        stored = next(row for row in inputs if row["originalMessage"].get("Message-Id") == "stored-message")
        check(stored["originalMessage"] == mail_event, "Mailgun nested raw event changed")
        multipart = next(row for row in inputs if row["originalMessage"].get("Message-Id") == "multipart-message")
        check(base64.b64decode(multipart["attachments"][0]["data"]) == CONTENT, "Multipart bytes changed")
        for row in inputs:
            check(all("storage_key" not in item for item in row["attachments"]), "Unexpected automatic storage reference")
        request(api + f"/events/webhook/slack/{slack}", raw=slack_body, headers=slack_headers)
        request(api + f"/events/webhook/mailgun/{mailgun}", mail_event)
        time.sleep(1)
        check(len(instance_list(wid)) == 3 and not REQUESTS, "Redelivery caused duplicate execution or downloads")
        print("PASS: raw/file-only/multipart ingestion, no downloads, redelivery deduplication", flush=True)

        for agent, capability, conn, values in [
            ("slack", "download-file", slack, {"file_id": "F-test"}),
            ("mailgun", "download-attachment", mailgun, {"url": MESSAGE_URL + "/attachments/0", "filename": "invoice.bin"}),
        ]:
            example = json.loads((ROOT / "docs/examples/attachments" / (agent + "-download.json")).read_text())
            step = example["steps"]["download"]
            step["connectionId"] = conn
            finish_download = {"stepType": "Finish", "id": "finish", "inputMapping": {"file": {"valueType": "reference", "value": "steps.download.outputs"}}}
            upload = {"stepType": "Agent", "id": "upload", "agentId": "s3-storage", "capabilityId": "storage-upload-file", "connectionId": storage, "inputMapping": {
                "bucket": {"valueType": "immediate", "value": "attachment-test"},
                "key": {"valueType": "immediate", "value": "invoice.bin"},
                "content": {"valueType": "reference", "value": "steps.download.outputs.content"},
                "content_type": {"valueType": "reference", "value": "steps.download.outputs.content_type"},
            }}
            download_wid = workflow(agent + " explicit download", {"download": step, "upload": upload, "finish": finish_download}, "download", [{"fromStep": "download", "toStep": "upload"}, {"fromStep": "upload", "toStep": "finish"}], example["inputSchema"])
            attachment = {"id": values["file_id"]} if agent == "slack" else {"url": values["url"], "name": "invoice.bin"}
            request(api + f"/workflows/{download_wid}/execute", {"inputs": {"data": {"attachments": [attachment]}, "variables": {}}})
            result = wait_until(lambda: completed(download_wid, 1), 120)[0]
            output = result["outputs"]["file"]
            check(base64.b64decode(output["content"]) == CONTENT, "Workflow download returned different bytes")
        check(UPLOADED == [CONTENT, CONTENT], "Explicit S3 upload must receive the exact downloaded bytes")
        print("PASS: compiled Slack/Mailgun workflows download and explicitly store exact binary content", flush=True)
        count = len(REQUESTS)
        metadata = request(api + "/agents/mailgun/capabilities/get-message/test", {"connectionId": mailgun, "input": {"url": MESSAGE_URL}})
        check(metadata["success"] and metadata["output"]["message"]["body-html"] == "<p>email text</p>", "Stored message retrieval failed")
        check(len(REQUESTS) == count + 1, "Get Message must not download attachments")
        print("PASS: Mailgun stored-message retrieval through the local server", flush=True)
        def http_request(path):
            return request(api + "/agents/http/capabilities/http-request/test", {"input": {
                "method": "GET", "url": mock + path, "response_type": "json", "fail_on_error": False,
            }})
        redirected = http_request("/redirect")
        check(redirected["success"] and redirected["output"]["status_code"] == 302, "Native outbound service followed a redirect")
        check(not any(item.get("path") == "/must-not-follow" for item in REQUESTS), "Redirect target was contacted")
        bounded = http_request("/bounded")
        check(bounded["success"] and bounded["output"]["status_code"] == 413, "Host accepted an oversized response")
        check(bounded["output"]["body"]["code"] == "RESPONSE_TOO_LARGE", "Size-limit error code missing")
        print("PASS: native outbound host calls refuse redirects and bound upstream responses", flush=True)
    finally:
        for child in reversed(children):
            child.terminate()
            try:
                child.wait(timeout=30)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
        provider.shutdown()
        subprocess.run(["docker", "stop", container], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


if __name__ == "__main__":
    main()

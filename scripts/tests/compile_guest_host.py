"""A stand-in for the workflow API a compile guest's host serves (M7.15), for
scripts/check-compiler-guest.sh: it serves one compile request and the
components it names, takes the composed component and the outcome, and writes
them to OUT_DIR. With WORKER, it serves a warm compiler (SYN-930) the same
compile twice, over /v1/compiles/*, each one's outcome to outcome-{id}.json and
the components each fetched to fetched-{id}.

Usage: compile_guest_host.py PORT WORKFLOW_JSON COMPONENTS_DIR OUT_DIR [WORKER]
"""
import hashlib
import http.server
import json
import os
import sys
import urllib.parse
import zlib

port, workflow, components_dir, out_dir = int(sys.argv[1]), sys.argv[2], sys.argv[3], sys.argv[4]
WORKER = len(sys.argv) > 5
QUEUED = ["j0", "j1"] if WORKER else []
# The compiles it issues, each to its own id as written here: a file is named
# only after one of these, never after a path a request names.
ISSUED = {compile_id: compile_id for compile_id in QUEUED}
files = {}
names = {}
for name in sorted(os.listdir(components_dir)):
    path = os.path.join(components_dir, name)
    if os.path.isfile(path):
        with open(path, "rb") as component:
            digest = hashlib.sha256(component.read()).hexdigest()
        files[digest] = path
        names[name] = digest
with open(workflow) as graph:
    execution_graph = json.load(graph)
REQUEST = json.dumps({
    "tenant_id": "t", "workflow_id": "guest", "version": 1,
    "execution_graph": execution_graph, "track_events": False,
    "child_workflows": [], "agent_catalog": None, "agent_slug": None,
    "source_checksum": None, "components": names, "extra_components": [],
}).encode()


class Host(http.server.BaseHTTPRequestHandler):
    def reply(self, status, body):
        self.send_response(status)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def body(self):
        return self.rfile.read(int(self.headers.get("Content-Length", 0)))

    @staticmethod
    def artifact(path):
        """The compile and component a path names: (id or None, digest)."""
        if path.startswith("/v1/compiles/"):
            compile_id, _, digest = path.removeprefix("/v1/compiles/").partition("/artifacts/")
            return ISSUED.get(compile_id, "unknown"), digest
        return None, path.removeprefix("/v1/artifacts/")

    def do_GET(self):
        if self.path == "/v1/run/input":
            return self.reply(200, json.dumps({"worker": {"idle_ms": 500, "deflate": True}}).encode() if WORKER else REQUEST)
        url = urllib.parse.urlparse(self.path)
        if url.path == "/v1/compiles/next":
            if not QUEUED:
                return self.reply(204, b"")
            return self.reply(200, json.dumps({"id": QUEUED.pop(0), "request": json.loads(REQUEST)}).encode())
        compile_id, digest = Host.artifact(url.path)
        query = urllib.parse.parse_qs(url.query)
        if digest in files:
            log = f"fetched-{compile_id}" if compile_id else "fetched"
            with open(os.path.join(out_dir, log), "a") as fetched:
                fetched.write(digest + "\n")
            with open(files[digest], "rb") as component:
                data = component.read()
            offset = int(query.get("offset", ["0"])[0])
            length = int(query.get("length", [str(len(data))])[0])
            return self.reply(200, data[offset:offset + length])
        return self.reply(404, b"{}")

    # The composed component, in parts, as a node's host origin bounds a
    # request.
    PARTS = {}

    def do_PUT(self):
        url = urllib.parse.urlparse(self.path)
        compile_id, digest = Host.artifact(url.path)
        query = urllib.parse.parse_qs(url.query)
        part, parts = int(query["part"][0]), int(query["parts"][0])
        made = Host.PARTS.setdefault(compile_id, {})
        made[part] = self.body()
        whole = len(made) == parts
        if whole:
            wasm = b"".join(made[i] for i in range(parts))
            if query.get("encoding") == ["deflate"]:
                wasm = zlib.decompress(wasm, -15)
            assert hashlib.sha256(wasm).hexdigest() == digest
            name = f"workflow-{compile_id}.wasm" if compile_id else "workflow.wasm"
            with open(os.path.join(out_dir, name), "wb") as out:
                out.write(wasm)
        self.reply(200, json.dumps({"sha256": digest, "whole": whole}).encode())

    def do_POST(self):
        name = "outcome.json"
        if self.path.startswith("/v1/compiles/"):
            compile_id = ISSUED.get(self.path.split("/")[3])
            if compile_id is None:
                return self.reply(404, b"{}")
            name = f"outcome-{compile_id}.json"
        with open(os.path.join(out_dir, name), "wb") as out:
            out.write(self.body())
        self.reply(200, b"{}")

    def log_message(self, *args):
        pass


http.server.HTTPServer(("127.0.0.1", port), Host).serve_forever()

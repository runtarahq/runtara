"""A stand-in for the workflow API a compile guest's host serves (M7.15), for
scripts/check-compiler-guest.sh: it serves one compile request and the
components it names, takes the composed component and the outcome, and writes
them to OUT_DIR.

Usage: compile_guest_host.py PORT WORKFLOW_JSON COMPONENTS_DIR OUT_DIR
"""
import hashlib
import http.server
import json
import os
import sys
import urllib.parse

port, workflow, components_dir, out_dir = int(sys.argv[1]), sys.argv[2], sys.argv[3], sys.argv[4]
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

    def do_GET(self):
        if self.path == "/v1/run/input":
            return self.reply(200, REQUEST)
        url = urllib.parse.urlparse(self.path)
        digest = url.path.removeprefix("/v1/artifacts/")
        query = urllib.parse.parse_qs(url.query)
        if digest in files:
            with open(os.path.join(out_dir, "fetched"), "a") as fetched:
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
        digest = url.path.removeprefix("/v1/artifacts/")
        query = urllib.parse.parse_qs(url.query)
        part, parts = int(query["part"][0]), int(query["parts"][0])
        Host.PARTS[part] = self.body()
        whole = len(Host.PARTS) == parts
        if whole:
            wasm = b"".join(Host.PARTS[i] for i in range(parts))
            assert hashlib.sha256(wasm).hexdigest() == digest
            with open(os.path.join(out_dir, "workflow.wasm"), "wb") as out:
                out.write(wasm)
        self.reply(200, json.dumps({"sha256": digest, "whole": whole}).encode())

    def do_POST(self):
        with open(os.path.join(out_dir, "outcome.json"), "wb") as out:
            out.write(self.body())
        self.reply(200, b"{}")

    def log_message(self, *args):
        pass


http.server.HTTPServer(("127.0.0.1", port), Host).serve_forever()

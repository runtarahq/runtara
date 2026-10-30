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

port, workflow, components_dir, out_dir = int(sys.argv[1]), sys.argv[2], sys.argv[3], sys.argv[4]
files = {}
names = {}
for name in sorted(os.listdir(components_dir)):
    path = os.path.join(components_dir, name)
    if os.path.isfile(path):
        digest = hashlib.sha256(open(path, "rb").read()).hexdigest()
        files[digest] = path
        names[name] = digest
REQUEST = json.dumps({
    "tenant_id": "t", "workflow_id": "guest", "version": 1,
    "execution_graph": json.load(open(workflow)), "track_events": False,
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
        digest = self.path.removeprefix("/v1/artifacts/")
        if digest in files:
            with open(os.path.join(out_dir, "fetched"), "a") as fetched:
                fetched.write(digest + "\n")
            return self.reply(200, open(files[digest], "rb").read())
        self.reply(404, b"{}")

    def do_PUT(self):
        wasm = self.body()
        open(os.path.join(out_dir, "workflow.wasm"), "wb").write(wasm)
        self.reply(200, json.dumps({"sha256": hashlib.sha256(wasm).hexdigest()}).encode())

    def do_POST(self):
        open(os.path.join(out_dir, "outcome.json"), "wb").write(self.body())
        self.reply(200, b"{}")

    def log_message(self, *args):
        pass


http.server.HTTPServer(("127.0.0.1", port), Host).serve_forever()

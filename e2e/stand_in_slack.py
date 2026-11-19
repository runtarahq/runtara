#!/usr/bin/env python3
"""A stand-in for Slack's Web API, for the Slack adapter's end-to-end test.

Answers `POST /api/chat.postMessage` as Slack does, and records each call
(its JSON body and whether it carried a bearer token, never the token) as a
line of JSON in the log file.

Usage: stand_in_slack.py <port> <log file>
"""
import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1])
LOG = sys.argv[2]


class Slack(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("content-length") or 0)
        body = json.loads(self.rfile.read(length) or b"{}")
        if self.path != "/api/chat.postMessage":
            return self.answer(404, {"ok": False, "error": "unknown_method"})
        auth = self.headers.get("authorization") or ""
        with open(LOG, "a") as log:
            log.write(json.dumps({
                "method": "chat.postMessage",
                "bearer": auth.startswith("Bearer ") and len(auth) > len("Bearer "),
                "body": body,
            }) + "\n")
        self.answer(200, {"ok": True, "channel": body.get("channel"), "ts": f"{time.time():.6f}"})

    def answer(self, status, body):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_):
        pass


ThreadingHTTPServer(("127.0.0.1", PORT), Slack).serve_forever()

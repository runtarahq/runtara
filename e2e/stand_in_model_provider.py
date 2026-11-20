# A stand-in OpenAI-style model provider for the e2e scripts that stream an
# AI Agent's answer (docs/ai-agent-streaming.md).
#
# Usage: python3 e2e/stand_in_model_provider.py <port> <log.jsonl>
#
# A request that asks to stream is answered with three pieces of text as
# server-sent events 400 ms apart; any other, whole. Each request's path, its
# `stream`, whether it carried the stand-in key, and the tools it offered
# (each one's name and parameters) are appended to the log.
import json, sys, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port, log = int(sys.argv[1]), sys.argv[2]
PIECES = ["Once", " upon", " a time."]

class Provider(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def body(self):
        """The request's body, sent with a length or in chunks (as seal of
        its own sends it)."""
        if self.headers.get("content-length") is not None:
            return self.rfile.read(int(self.headers["content-length"]))
        body = b""
        while True:
            size = int(self.rfile.readline().split(b";")[0].strip(), 16)
            if size == 0:
                while self.rfile.readline() not in (b"\r\n", b"\n", b""):
                    pass
                return body
            body += self.rfile.read(size)
            self.rfile.readline()

    def do_POST(self):
        asked = json.loads(self.body())
        streams = asked.get("stream") is True
        with open(log, "a") as out:
            tools = [{"name": tool.get("function", {}).get("name"),
                      "parameters": tool.get("function", {}).get("parameters")}
                     for tool in asked.get("tools") or []]
            out.write(json.dumps({"path": self.path, "stream": streams,
                                  "authorized": self.headers.get("authorization") == "Bearer sk-stand-in",
                                  "tools": tools}) + "\n")
        if not streams:
            body = json.dumps({"choices": [{"message": {"content": "".join(PIECES)}}],
                               "usage": {"prompt_tokens": 3, "completion_tokens": 3, "total_tokens": 6}}).encode()
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        # Closed when done: the body is what comes until then.
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("connection", "close")
        self.end_headers()
        for piece in PIECES:
            event = {"choices": [{"delta": {"content": piece}}]}
            self.wfile.write(f"data: {json.dumps(event)}\n\n".encode())
            self.wfile.flush()
            time.sleep(0.4)
        usage = {"choices": [], "usage": {"prompt_tokens": 3, "completion_tokens": 3, "total_tokens": 6}}
        self.wfile.write(f"data: {json.dumps(usage)}\n\ndata: [DONE]\n\n".encode())
        self.wfile.flush()

ThreadingHTTPServer(("127.0.0.1", port), Provider).serve_forever()

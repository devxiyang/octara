#!/usr/bin/env python3
"""A loopback-only OpenAI-compatible SSE peer for the real Chat window.

Start with --output NEW_DIRECTORY: creates endpoint.txt and events.jsonl there.
Alternatively --endpoint-file NEW_PATH writes endpoint/model/control JSON and
--events-file NEW_PATH selects its log (otherwise stderr). Existing outputs are
never overwritten. Use model `local-fixture`, `wire-a` or `wire-b` and no key.

The first word of the latest user message selects a scenario (also [hold], /hold):
  normal   bounded, paced Markdown with Chinese and emoji (the default)
  hold     response headers, then no first token until release or cancellation
  idle     one text delta, then silence until release or cancellation
  drop     first request for this exact question drops after partial text;
           regenerate that question succeeds, independently of other questions
  partial  ends with an incomplete SSE event, with no successful terminal marker
  401 / 429 (or http401 / http429)  an HTTP failure with private response details

GET /status returns content-free request facts and recent events. POST /control
with {"action":"release"|"disconnect","ordinal":N} controls an active request;
{"action":"shutdown"} stops this server. These HTTP controls never operate the OS
or inject app input. Ctrl+C also stops the server. No request text, auth headers,
or API keys are logged. Only the three allowlisted fixture model names are accepted
and recorded. History roles, byte counts and SHA-256 fingerprints let a
caller compare known fixture history without writing its text to the event log.
"""

import argparse
from collections import deque
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import select
import socket
import sys
import threading
import time


MAX_BODY = 1 << 20
MAX_REQUESTS = 256
MAX_OUTPUT = 96 * 1024
MODELS = frozenset({"local-fixture", "wire-a", "wire-b"})


class State:
    def __init__(self, events, interval, chunks):
        self.lock = threading.Lock()
        self.stopping = threading.Event()
        self.events = deque(maxlen=4096)
        self.requests = {}
        self.attempts = {}
        self.output = events
        self.interval = interval
        self.chunks = chunks
        self.next_ordinal = 1

    def event(self, kind, ordinal=None, **facts):
        record = {"event": kind, "monotonic_s": round(time.monotonic(), 6), **facts}
        if ordinal is not None:
            record["ordinal"] = ordinal
        with self.lock:
            self.events.append(record)
            print(json.dumps(record, ensure_ascii=False), file=self.output, flush=True)

    def begin(self, messages, model, generation):
        latest = next((m["content"] for m in reversed(messages) if m["role"] == "user"), "")
        fingerprint = hashlib.sha256(latest.encode()).hexdigest()
        first = latest.strip().split(maxsplit=1)
        mode = first[0].lower().strip("[]/") if first else "normal"
        mode = {"401": "http401", "429": "http429"}.get(mode, mode)
        if mode not in {"hold", "idle", "drop", "partial", "http401", "http429"}:
            mode = "normal"
        history = [{"role": m["role"], "bytes": len(m["content"].encode()),
                    "sha256": hashlib.sha256(m["content"].encode()).hexdigest()} for m in messages]
        with self.lock:
            if self.next_ordinal > MAX_REQUESTS:
                return None
            ordinal = self.next_ordinal
            self.next_ordinal += 1
            attempt = self.attempts.get(fingerprint, 0) + 1
            self.attempts[fingerprint] = attempt
            effective = "normal" if mode == "drop" and attempt > 1 else mode
            self.requests[ordinal] = {
                "model": model, "mode": effective, "attempt": attempt, "history": history, "generation": generation,
                "active": True, "control": None, "sent_chunks": 0, "sent_bytes": 0,
            }
        self.event("request_started", ordinal, model=model, mode=effective, attempt=attempt, history=history, generation=generation)
        return ordinal, effective

    def snapshot(self):
        with self.lock:
            return {"requests": {n: dict(r) for n, r in self.requests.items()},
                    "events": list(self.events), "stopping": self.stopping.is_set()}


class Server(ThreadingHTTPServer):
    daemon_threads = False
    block_on_close = True

    def __init__(self, state):
        super().__init__(("127.0.0.1", 0), Handler)
        self.state = state


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_):
        pass  # BaseHTTPRequestHandler's request/error text is outside the content-free log.

    def setup(self):
        super().setup()
        self.connection.settimeout(2)

    def json_reply(self, status, value):
        body = json.dumps(value, ensure_ascii=False).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
        self.close_connection = True

    def do_GET(self):
        if self.path == "/status":
            self.json_reply(200, self.server.state.snapshot())
        else:
            self.json_reply(404, {"error": "unknown fixture path"})

    def body(self):
        try:
            length = int(self.headers.get("Content-Length", "-1"))
        except ValueError:
            return None
        if not 0 <= length <= MAX_BODY:
            return None
        try:
            return json.loads(self.rfile.read(length))
        except (ValueError, UnicodeError):
            return None

    def do_POST(self):
        value = self.body()
        if not isinstance(value, dict):
            return self.json_reply(400, {"error": "expected a bounded JSON object"})
        if self.path == "/control":
            return self.control(value)
        if self.path != "/v1/chat/completions":
            return self.json_reply(404, {"error": "unknown fixture path"})
        model = value.get("model")
        if not isinstance(model, str) or len(model) > 64 or model not in MODELS:
            return self.json_reply(400, {"error": "use an allowlisted fixture model and no credentials"})
        messages = value.get("messages")
        if value.get("stream") is not True or not isinstance(messages, list) or not messages:
            return self.json_reply(400, {"error": "expected streaming chat messages"})
        if any(not isinstance(m, dict) or m.get("role") not in {"system", "user", "assistant"}
               or not isinstance(m.get("content"), str) for m in messages):
            return self.json_reply(400, {"error": "expected text message history"})
        generation = {key: value[key] for key in ("temperature", "max_completion_tokens") if key in value}
        begun = self.server.state.begin(messages, model, generation)
        if begun is None:
            return self.json_reply(503, {"error": "fixture request limit reached"})
        ordinal, mode = begun
        reason = "server_stopped"
        try:
            reason = self.stream(ordinal, mode)
        except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
            reason = "client_closed"
        except (TimeoutError, OSError):
            reason = "connection_ended"
        finally:
            with self.server.state.lock:
                self.server.state.requests[ordinal]["active"] = False
                self.server.state.requests[ordinal]["ended"] = reason
            self.server.state.event("request_ended", ordinal, reason=reason)
            self.close_connection = True

    def control(self, value):
        state = self.server.state
        action = value.get("action")
        if action == "shutdown":
            self.json_reply(200, {"ok": True})
            state.stopping.set()
            threading.Thread(target=self.server.shutdown, daemon=True).start()
            return
        if action not in {"release", "disconnect"}:
            return self.json_reply(400, {"error": "unknown control action"})
        ordinal = value.get("ordinal")
        with state.lock:
            request = state.requests.get(ordinal) if isinstance(ordinal, int) else None
            found = request is not None and request["active"]
            if found:
                request["control"] = action
        if not found:
            return self.json_reply(404, {"error": "no active request at that ordinal"})
        state.event("control", ordinal, action=action)
        self.json_reply(200, {"ok": True})

    def wait(self, ordinal, duration):
        """A socket becoming readable detects client EOF even before the first token."""
        state = self.server.state
        deadline = time.monotonic() + duration
        while not state.stopping.is_set():
            with state.lock:
                control = state.requests[ordinal]["control"]
            if control:
                return control
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return None
            readable, _, _ = select.select([self.connection], [], [], min(remaining, 0.05))
            if readable and not self.connection.recv(1, socket.MSG_PEEK):
                return "client_closed"
        return "server_stopped"

    def data(self, value):
        payload = ("data: " + (value if isinstance(value, str) else json.dumps(value, ensure_ascii=False)) + "\r\n\r\n").encode()
        self.raw(payload)

    def raw(self, payload):
        self.wfile.write(f"{len(payload):x}\r\n".encode() + payload + b"\r\n")
        self.wfile.flush()

    def text(self, ordinal, text):
        self.data({"choices": [{"index": 0, "delta": {"content": text}}]})
        state = self.server.state
        with state.lock:
            request = state.requests[ordinal]
            request["sent_chunks"] += 1
            request["sent_bytes"] += len(text.encode())

    def stream(self, ordinal, mode):
        state = self.server.state
        if mode.startswith("http"):
            status = int(mode[4:])
            self.json_reply(status, {"error": "private fixture upstream detail; do not show this body"})
            state.event("http_status", ordinal, status=status)
            return "http_error"
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream; charset=utf-8")
        self.send_header("Transfer-Encoding", "chunked")
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.flush()
        state.event("response_started", ordinal)
        if mode in {"hold", "idle"}:
            if mode == "idle":
                self.text(ordinal, "A prefix before the fixture becomes idle. 中文 😀\n\n")
            outcome = self.wait(ordinal, 60)
            if outcome != "release":
                return outcome or "hold_limit"
        self.text(ordinal, "## Local HTTP stream\n\n")
        count = 16 if mode in {"drop", "partial"} else state.chunks
        for index in range(count):
            with state.lock:
                if state.requests[ordinal]["sent_bytes"] >= MAX_OUTPUT:
                    break
            outcome = self.wait(ordinal, state.interval)
            if outcome not in {None, "release"}:
                return outcome
            if index % 8 == 7:
                text = "\n\n```rust\nlet reply = \"中文 😀\";\n```\n\n"
            else:
                text = "Streaming over real HTTP keeps a complete ordered answer. 中文输入与选择 😀。 "
            self.text(ordinal, text)
        if mode == "drop":
            state.event("intentional_drop", ordinal)
            return "intentional_drop"  # No terminal HTTP chunk, no SSE success marker.
        if mode == "partial":
            self.raw(b'data: {"choices":[{"index":0,"delta":{"content":"incomplete')
            self.wfile.write(b"0\r\n\r\n")
            self.wfile.flush()
            return "incomplete_sse"
        self.data({"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]})
        self.data("[DONE]")
        self.wfile.write(b"0\r\n\r\n")
        self.wfile.flush()
        state.event("response_completed", ordinal)
        return "completed"


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    destination = parser.add_mutually_exclusive_group(required=True)
    destination.add_argument("--output", type=Path, help="new directory for endpoint.txt and events.jsonl")
    destination.add_argument("--endpoint-file", type=Path, help="new JSON endpoint file; refuses overwrite")
    parser.add_argument("--events-file", type=Path, help="new JSONL log, otherwise stderr")
    parser.add_argument("--interval-ms", type=float, default=100, help="paced delta interval (5..1000 ms, default 100)")
    parser.add_argument("--chunks", type=int, default=20, help="normal answer delta count (1..1000, default 20), capped at 96 KiB")
    args = parser.parse_args()
    if not 5 <= args.interval_ms <= 1000 or not 1 <= args.chunks <= 1000:
        parser.error("interval must be 5..1000 ms and chunks 1..1000")
    if args.output:
        if args.events_file:
            parser.error("--events-file belongs to --endpoint-file mode")
        args.output.mkdir()
        args.events_file = args.output / "events.jsonl"
    events = args.events_file.open("x", encoding="utf-8") if args.events_file else sys.stderr
    state = State(events, args.interval_ms / 1000, args.chunks)
    server = Server(state)
    port = server.server_address[1]
    endpoint = {"endpoint": f"http://127.0.0.1:{port}/v1/chat/completions", "model": "local-fixture",
                "control": f"http://127.0.0.1:{port}/control", "status": f"http://127.0.0.1:{port}/status"}
    try:
        if args.output:
            (args.output / "endpoint.txt").write_text(endpoint["endpoint"] + "\n", encoding="utf-8")
        else:
            with args.endpoint_file.open("x", encoding="utf-8") as output:
                json.dump(endpoint, output, indent=2)
                output.write("\n")
        state.event("listening", endpoint=endpoint["endpoint"])
        server.serve_forever(poll_interval=0.1)
    except KeyboardInterrupt:
        pass
    finally:
        state.stopping.set()
        server.server_close()
        state.event("server_stopped")
        if events is not sys.stderr:
            events.close()


if __name__ == "__main__":
    main()

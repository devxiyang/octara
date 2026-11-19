#!/usr/bin/env python3
"""Verify model selection in a real Chat window against a local HTTP peer.

Build the model_selection example and octara CLI before running this script. It starts only
its own loopback server and window, uses an isolated store, and retains JSON evidence and GPU
screenshots in --output. All typing, keys and taps use Inspector framework events; this does
not verify OS input injection. Optional display captures report their actual pixel source.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import sys
import tempfile
import threading
import time
from urllib.parse import urlsplit
from urllib.request import ProxyHandler, Request, build_opener

REPO = Path(__file__).resolve().parents[2]
sys.dont_write_bytecode = True
sys.path.insert(0, str(REPO / "examples"))
from live_inspect import LiveWindow, SmokeFailure, find, nodes, png_pixels, require  # noqa: E402

PREFIX = "A prefix before the fixture becomes idle."
LABELS = {"local-a": "本地模型 A", "local-b": "本地模型 B"}


def save(path, value):
    with path.open("x", encoding="utf-8") as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write("\n")


def wait(timeout, sample, ready, reason):
    deadline = time.monotonic() + timeout
    while True:
        value = sample()
        if ready(value):
            return value
        require(time.monotonic() < deadline, f"timed out waiting for {reason}")
        threading.Event().wait(0.05)


def has(snapshot, node_id):
    return any(node.get("id") == node_id for node in nodes(snapshot))


def text_under(snapshot, node_id):
    pending = [find(snapshot, node_id)]
    text = []
    while pending:
        node = pending.pop()
        text.append(node.get("name", ""))
        pending.extend(node.get("children", []))
    return " ".join(text)


def last_answer(snapshot):
    ids = [int(n["id"][4:]) for n in nodes(snapshot) if re.fullmatch(r"msg-\d+", n.get("id", ""))]
    require(ids, "the transcript has no message")
    return max(ids)


def history_fact(role, text):
    raw = text.encode()
    return {"role": role, "bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest()}


def saved_history(store, count, timeout):
    def read():
        files = list(store.glob("*.json"))
        return json.loads(files[0].read_text()) if len(files) == 1 else {}

    saved = wait(timeout, read, lambda s: len(s.get("messages", [])) == count
                 and all(m["status"] == "Done" for m in s["messages"]), "the isolated conversation save")
    return [history_fact(m["author"].lower(), "\n\n".join(p["Text"] for _, p in m["parts"] if "Text" in p))
            for m in saved["messages"]]


class HTTPFixture:
    def __init__(self, args, output):
        self.args, self.output = args, output
        self.process = self.log = None
        self.endpoint = self.base = None
        self.opener = build_opener(ProxyHandler({}))

    def __enter__(self):
        self.log = (self.output / "http-server.log").open("x", encoding="utf-8")
        directory = self.output / "http"
        self.process = subprocess.Popen(
            [sys.executable, str(REPO / "apps/chat/serve_network_fixture.py"), "--output", str(directory),
             "--interval-ms", "15", "--chunks", "3"], cwd=REPO, stdout=self.log, stderr=subprocess.STDOUT)
        try:
            endpoint_file = directory / "endpoint.txt"
            def available():
                require(self.process.poll() is None, "HTTP fixture exited; see http-server.log")
                value = endpoint_file.read_text().strip() if endpoint_file.is_file() else ""
                return value if value.endswith("/v1/chat/completions") else None
            self.endpoint = wait(self.args.timeout, available, bool, "the loopback HTTP fixture")
            parsed = urlsplit(self.endpoint)
            require(parsed.scheme == "http" and parsed.hostname == "127.0.0.1" and parsed.port
                    and not parsed.username and not parsed.password, "fixture endpoint must be credential-free loopback HTTP")
            self.base = f"http://127.0.0.1:{parsed.port}"
            self.status()
            return self
        except BaseException:
            self.__exit__(*sys.exc_info())
            raise

    def __exit__(self, *_):
        if self.process and self.process.poll() is None:
            if self.base:
                try:
                    self.api("/control", {"action": "shutdown"})
                except OSError:
                    pass
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.terminate()
                try:
                    self.process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait(timeout=5)
        if self.log:
            self.log.close()

    def api(self, path, value=None):
        body = None if value is None else json.dumps(value).encode()
        request = Request(self.base + path, data=body, headers={"Content-Type": "application/json"})
        with self.opener.open(request, timeout=self.args.timeout) as response:
            return json.load(response)

    def status(self):
        return self.api("/status")

    def request(self, ordinal, ended=None):
        status = wait(self.args.timeout, self.status,
                      lambda s: str(ordinal) in s["requests"] and (ended is None
                                or s["requests"][str(ordinal)].get("ended") == ended), f"HTTP request {ordinal}: {ended or 'started'}")
        return status["requests"][str(ordinal)]

    def release(self, ordinal):
        require(self.api("/control", {"action": "release", "ordinal": ordinal}).get("ok"), "fixture release failed")


class ModelWindow(LiveWindow):
    def __init__(self, args, output, sockets, store, fixture):
        super().__init__("model-selection", args, output, sockets)
        self.store, self.fixture_server = store, fixture
        self.routes = []

    def environment(self):
        env = {k: v for k, v in os.environ.items() if not k.startswith("OCTARA_")}
        env.update(OCTARA_INSPECT=str(self.endpoint), OCTARA_CHAT_DIR=str(self.store),
                   OCTARA_CHAT_ENDPOINT=self.fixture_server.endpoint, NO_PROXY="127.0.0.1", no_proxy="127.0.0.1")
        return env

    def __enter__(self):
        self.log = (self.output / f"{self.fixture}-window.log").open("x", encoding="utf-8")
        self.transcript = (self.output / f"{self.fixture}-cli.jsonl").open("x", encoding="utf-8")
        self.process = subprocess.Popen([str(self.args.window)], cwd=REPO, env=self.environment(), stdout=self.log, stderr=subprocess.STDOUT)
        try:
            def available():
                self.alive()
                return self.endpoint.is_socket()
            wait(self.args.timeout, available, bool, "the real window Inspector")
            self.inspect()
            return self
        except BaseException:
            self.__exit__(*sys.exc_info())
            raise

    def __exit__(self, *error):
        super().__exit__(*error)
        if self.endpoint.is_socket():
            self.endpoint.unlink()  # This path is unique to this run; keep every evidence file.

    def guarded(self, op, **fields):
        snapshot = self.inspect()
        if op in {"tap", "type"}:
            target = find(snapshot, fields.get("id") or snapshot.get("focus"))["target"]
        elif op == "key":
            target = find(snapshot, snapshot.get("focus"))["target"]
        else:
            target = snapshot["target"]
        reply = self.requests({"op": op, "target": target, **fields})[0]
        live = reply.get("live", {})
        self.routes.append({"op": op, "route": live.get("route"), "delivery": live.get("delivery")})
        if op in {"tap", "type", "key"}:
            require(live.get("route") == "framework" and live.get("delivery") == "delivered",
                    f"input was not delivered through the framework: {reply}")
        return reply

    def observation(self, name):
        snapshot = self.inspect()
        save(self.output / f"{name}.json", snapshot)
        return snapshot

    def until(self, predicate, reason):
        return wait(self.args.timeout, self.inspect, predicate, reason)

    def say(self, question):
        self.guarded("type", id="composer", text=question)
        self.guarded("key", chord="enter")

    def select_model(self, model_id, capture=None):
        self.guarded("tap", id="model")
        self.until(lambda s: s.get("focus") == "model-search", "picker search focus")
        self.guarded("type", id="model-search", text="本地")
        snapshot = self.inspect()
        option = f"model-option-{model_id}"
        require(has(snapshot, option), f"Chinese search omitted {option}")
        if find(snapshot, "model-search").get("active_descendant") != option:
            self.guarded("key", chord="down")
        require(find(self.inspect(), "model-search").get("active_descendant") == option, "keyboard did not highlight the intended ID")
        if capture:
            self.observation(capture)
            self.gpu(capture + ".png")
        self.guarded("key", chord="enter")
        selected = self.until(lambda s: not has(s, "model-popover"), "picker dismissal")
        require(selected.get("focus") == "model", "closing the picker did not restore trigger focus")
        require(find(selected, "model").get("value") == LABELS[model_id], "picker label disagrees with selected ID")
        return selected

    def gpu(self, name):
        path = self.output / name
        reply = self.guarded("capture", path=str(path), pixels="painted", scale=1, settle_ms=1000)
        report = reply["capture"]
        require(report["renderer"] == "gpu" and report["pixels"] == "painted", f"unexpected GPU capture source: {report}")
        width, height, _ = png_pixels(path)
        require([width, height] == [report["width"], report["height"]], "GPU report disagrees with PNG size")
        return report

    def clean(self):
        issues = self.guarded("issues").get("issues", [])
        require(not issues, f"window layout or diagnostics: {issues}")


def run(args, output, sockets, store, server, summary):
    with ModelWindow(args, output, sockets, store, server) as app:
        require(find(app.observation("01-initial"), "model").get("value") == LABELS["local-a"], "A is not initially selected")
        question = "idle model selection 中文"
        app.say(question)
        first = app.until(lambda s: PREFIX in text_under(s, "transcript") and has(s, "stop"), "A's idle prefix")
        old_answer = last_answer(first)
        require(server.request(1)["model"] == "wire-a", "initial request did not use wire-a")
        app.select_model("local-b", capture="02-desktop-picker")
        selected = app.observation("03-selected-b-a-streaming")
        active = server.status()
        require(len(active["requests"]) == 1 and active["requests"]["1"]["active"], "picker selection replaced or stopped A's HTTP request")
        require(PREFIX in text_under(selected, "transcript") and has(selected, "stop"), "switching models lost A's streamed prefix")
        save(output / "03-http-a-still-active.json", active)

        resized = app.guarded("size", width=390, height=640)
        resize = resized["live"].get("resize", {})
        require(resized["live"].get("route") == "native-window" and resized["live"].get("delivery") == "delivered"
                and resize.get("matched") is True and resize.get("synchronized") is True,
                "native window did not confirm 390x640 resize")
        narrow = app.inspect()
        require(all(abs(a - b) < 1 for a, b in zip(narrow["viewport"], [390, 640])), "Inspector viewport disagrees with native resize")
        title, picker = find(narrow, "title")["bounds"], find(narrow, "model")["bounds"]
        require(title[2] > 0 and picker[2] > 0 and title[0] + title[2] <= picker[0] + 0.2
                and picker[0] + picker[2] <= 390, "narrow header title and picker overlap or overflow")
        app.guarded("tap", id="model")
        app.guarded("type", id="model-search", text="本地")
        popup = app.observation("04-narrow-picker")
        x, y, width, height = find(popup, "model-popover")["bounds"]
        require(x >= 0 and y >= 0 and x + width <= 390.2 and y + height <= 640.2, "narrow popup escapes the viewport")
        app.gpu("04-narrow-picker.png")
        app.clean()
        app.guarded("key", chord="escape")
        dismissed = app.inspect()
        require(dismissed.get("focus") == "model" and not has(dismissed, "model-popover"), "Escape did not close the picker and restore focus")
        require(find(dismissed, "model").get("value") == LABELS["local-b"], "Escape changed the selection")
        app.guarded("tap", id="stop")
        server.request(1, ended="client_closed")
        app.until(lambda s: has(s, f"regenerate-{old_answer}") and not has(s, "stop"), "A to stop")
        app.observation("05-stopped-a")

        app.guarded("tap", id=f"regenerate-{old_answer}")
        require(server.request(2)["model"] == "wire-b", "regenerate did not use selected wire-b")
        app.until(lambda s: has(s, "stop") and PREFIX in text_under(s, "transcript"), "B's regenerated prefix")
        server.release(2)
        server.request(2, ended="completed")
        app.until(lambda s: not has(s, "stop"), "B's regenerated answer to complete")
        app.observation("06-b-regenerated")
        saved_history(store, 2, args.timeout)

        app.say("normal followup on B 中文")
        require(server.request(3)["model"] == "wire-b", "followup did not use selected wire-b")
        server.request(3, ended="completed")
        app.until(lambda s: not has(s, "stop"), "B's followup to complete")
        saved_history(store, 4, args.timeout)
        app.select_model("local-a")
        app.say("idle A before queue 中文")
        require(server.request(4)["model"] == "wire-a", "subsequent A selection did not reach HTTP")
        app.until(lambda s: has(s, "stop") and PREFIX in text_under(s, "transcript"), "A's next idle answer")
        app.select_model("local-b")
        app.say("normal queued for B 中文")
        queued = app.until(lambda s: any(n.get("id", "").startswith("queued-model-") for n in nodes(s)), "B's queue row")
        row = next(n["id"] for n in nodes(queued) if n.get("id", "").startswith("queued-model-"))
        require("本地模型 B" in text_under(queued, row), "queue row does not show B")
        app.select_model("local-a")
        queued = app.observation("07-queued-b-selected-a")
        require("本地模型 B" in text_under(queued, row), "changing selection rewrote the queued model")
        require(len(server.status()["requests"]) == 4, "queued input sent an early HTTP request")
        app.gpu("07-queued-b-selected-a.png")
        server.release(4)
        server.request(4, ended="completed")
        require(server.request(5)["model"] == "wire-b", "queued request did not keep wire-b after selecting A")
        server.request(5, ended="completed")
        final = app.until(lambda s: not has(s, "stop") and not has(s, row), "the queue to finish")
        require(find(final, "model").get("value") == LABELS["local-a"], "draining the queue changed the current selection")
        history = saved_history(store, 8, args.timeout)
        facts = server.status()
        require(len(facts["requests"]) == 5, "unexpected HTTP request, possibly a network title request")
        for ordinal, expected in [(1, [history_fact("user", question)]), (2, history[:1]),
                                  (3, history[:3]), (4, history[:5]), (5, history[:7])]:
            require(facts["requests"][str(ordinal)]["history"] == expected, f"HTTP request {ordinal} history differs from the isolated conversation")
        save(output / "08-http-final.json", facts)
        app.observation("08-final")
        summary["gpu_capture"] = app.gpu("08-final.png")
        app.clean()
        # A display request may fall back; record its report without claiming OS pixels.
        try:
            summary["display_capture"] = app.guarded("capture", path=str(output / "08-display.png"), pixels="display", settle_ms=1000)["capture"]
        except SmokeFailure as error:
            summary["display_capture"] = {"available": False, "error": str(error)}
        save(output / "routes.json", app.routes)
        summary.update(ok=True, input_route="framework", os_input_verified=False,
                       model_sequence=[facts["requests"][str(n)]["model"] for n in range(1, 6)],
                       cancelled_socket=facts["requests"]["1"]["ended"], history_matches=True,
                       queued_model="wire-b", final_selection="local-a", viewport=final["viewport"],
                       fresh_targets=True, input_operations=sum(r["route"] == "framework" for r in app.routes),
                       native_resize_verified=True)


def default_binary(relative):
    candidates = [REPO / "target" / mode / relative for mode in ("release", "debug")]
    return next((path for path in candidates if path.is_file()), candidates[0])


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--window", type=Path, default=default_binary("examples/model_selection"), help="compiled model_selection example")
    parser.add_argument("--cli", type=Path, default=default_binary("octara"), help="compiled octara CLI")
    parser.add_argument("--output", type=Path, help="new or empty evidence directory; defaults to a new /tmp directory")
    parser.add_argument("--timeout", type=float, default=15, help="per-operation timeout, at most 30 seconds")
    args = parser.parse_args()
    require(os.name == "posix" and hasattr(socket, "AF_UNIX"), "this live check requires Unix-domain sockets")
    require(0 < args.timeout <= 30, "timeout must be greater than zero and at most 30 seconds")
    args.repo, args.octara = REPO, args.cli.resolve()
    args.window = args.window.resolve()
    for path in (args.window, args.octara):
        require(path.is_file() and os.access(path, os.X_OK), f"build the executable first: {path}")
    output = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="octara-chat-model-", dir="/tmp"))
    if args.output:
        output.mkdir(parents=True, exist_ok=True)
        require(not any(output.iterdir()), f"evidence directory must be empty: {output}")
    store = output / "store"
    store.mkdir()
    sockets = Path(tempfile.mkdtemp(prefix="octara-model-sock-", dir="/tmp"))
    summary = {"ok": False, "input_route": "framework", "os_input_verified": False}
    print(f"Model selection evidence: {output}", flush=True)
    try:
        with HTTPFixture(args, output) as server:
            run(args, output, sockets, store, server, summary)
    except BaseException as error:
        summary["error"] = str(error)
        raise
    finally:
        save(output / "summary.json", summary)
        try:
            sockets.rmdir()  # Never recursively delete evidence or another socket.
        except OSError:
            pass
    print("PASS real Chat window: Chinese picker, A cancellation, B regeneration, wire model/history, queued ID and GPU captures")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (SmokeFailure, OSError, ValueError, KeyError, StopIteration) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)

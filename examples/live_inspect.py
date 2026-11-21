#!/usr/bin/env python3
"""Check the live Inspector against actual desktop windows, using only Python's stdlib.

Build the window example and CLI first, then run from any directory:
    python3 examples/live_inspect.py --window target/debug/window --octara target/debug/octara

Requires an interactive desktop and Unix-domain sockets. Windows is skipped. Screenshots,
window logs and CLI transcripts stay in the printed output directory (or --output DIRECTORY).
Inputs are delivered through the framework. The --gpu-stale-paint check also uses a native
resize and requires macOS Accessibility access and a Swift compiler.
"""

import argparse
import contextlib
import hashlib
import json
import math
import os
from pathlib import Path
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
import zlib


class SmokeFailure(RuntimeError):
    pass


def require(condition, message):
    if not condition:
        raise SmokeFailure(message)


def nodes(snapshot):
    pending = [snapshot["root"]]
    while pending:
        node = pending.pop()
        yield node
        pending.extend(node.get("children", []))


def find(snapshot, target):
    for node in nodes(snapshot):
        if node.get("id") == target:
            return node
    raise SmokeFailure(f"snapshot has no {target!r} node")


def png_pixels(path, *, region=None):
    """Decode the RGBA8, noninterlaced PNGs written by Octara's Picture::save_png."""
    data = path.read_bytes()
    require(data[:8] == b"\x89PNG\r\n\x1a\n", f"{path}: not a PNG")
    width = height = None
    compressed = bytearray()
    position = 8
    while position < len(data):
        length = struct.unpack_from(">I", data, position)[0]
        kind = data[position + 4 : position + 8]
        payload = data[position + 8 : position + 8 + length]
        position += length + 12
        if kind == b"IHDR":
            width, height, depth, color, compression, filtering, interlace = struct.unpack(">IIBBBBB", payload)
            require((depth, color, compression, filtering, interlace) == (8, 6, 0, 0, 0), "expected Octara RGBA8 PNG")
        elif kind == b"IDAT":
            compressed.extend(payload)
        elif kind == b"IEND":
            break
    require(width and height, f"{path}: empty image")
    raw = zlib.decompress(compressed)
    stride = width * 4
    require(len(raw) == (stride + 1) * height, f"{path}: unexpected pixel length")
    pixels = bytearray()
    previous = bytearray(stride)
    for y in range(height):
        at = y * (stride + 1)
        mode = raw[at]
        require(mode <= 4, f"{path}: invalid PNG filter {mode}")
        row = bytearray(raw[at + 1 : at + stride + 1])
        for x in range(stride):
            left = row[x - 4] if x >= 4 else 0
            above = previous[x]
            upper_left = previous[x - 4] if x >= 4 else 0
            if mode == 1:
                prediction = left
            elif mode == 2:
                prediction = above
            elif mode == 3:
                prediction = (left + above) // 2
            elif mode == 4:
                estimate = left + above - upper_left
                distances = (abs(estimate - left), abs(estimate - above), abs(estimate - upper_left))
                prediction = (left, above, upper_left)[distances.index(min(distances))]
            else:
                prediction = 0
            row[x] = (row[x] + prediction) & 255
        pixels.extend(row)
        previous = row
    if region is None:
        return width, height, hashlib.sha256(pixels).hexdigest()
    x, y, w, h = region
    require(w > 0 and h > 0 and 0 <= x <= width - w and 0 <= y <= height - h, f"{path}: invalid pixel region {region}")
    digest = hashlib.sha256()
    for row in range(y, y + h):
        start = row * stride + x * 4
        digest.update(pixels[start : start + w * 4])
    return width, height, digest.hexdigest()


class LiveWindow:
    def __init__(self, fixture, args, output, sockets, window_args=None, *, env=None):
        self.fixture = fixture
        self.args = args
        self.output = output
        self.endpoint = sockets / f"{fixture}.sock"
        self.process = None
        self.log = None
        self.transcript = None
        self.window_args = window_args
        self.env = env or {}

    def __enter__(self):
        self.log = (self.output / f"{self.fixture}-window.log").open("w", encoding="utf-8")
        self.transcript = (self.output / f"{self.fixture}-cli.jsonl").open("w", encoding="utf-8")
        env = os.environ.copy()
        for name in ("OCTARA_CAPTURE", "OCTARA_CONFORMANCE", "OCTARA_EXIT_AFTER_FRAMES", "OCTARA_PICK"):
            env.pop(name, None)
        env.update(self.env)
        env["OCTARA_INSPECT"] = str(self.endpoint)
        fixture_args = self.window_args if self.window_args is not None else (["showcase", "search"] if self.fixture == "search" else [self.fixture])
        self.process = subprocess.Popen(
            [str(self.args.window), *fixture_args], cwd=self.args.repo, env=env,
            stdout=self.log, stderr=subprocess.STDOUT,
        )
        try:
            deadline = time.monotonic() + self.args.timeout
            while not self.endpoint.exists():
                self.alive()
                require(time.monotonic() < deadline, f"{self.fixture}: Inspector socket did not appear")
                threading.Event().wait(min(0.05, max(0, deadline - time.monotonic())))
            self.inspect()
            return self
        except BaseException:
            self.__exit__(*sys.exc_info())
            raise

    def __exit__(self, *_):
        if self.process and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        for stream in (self.log, self.transcript):
            if stream:
                stream.close()

    def alive(self):
        require(self.process.poll() is None, f"{self.fixture}: window exited; see {self.log.name}")

    def cli(self, command, payload=None, script=None, expected=0):
        self.alive()
        cmd = [str(self.args.octara), command, "--connect", str(self.endpoint), "--json"]
        if script:
            cmd.extend(["--script", str(script)])
        try:
            # Run outside the workspace: live mode must not require a manifest or build a harness.
            result = subprocess.run(cmd, input=payload, text=True, capture_output=True,
                                    cwd=self.output, timeout=self.args.timeout)
        except subprocess.TimeoutExpired as error:
            raise SmokeFailure(f"{self.fixture}: {command} timed out after {self.args.timeout}s") from error
        self.transcript.write(json.dumps({"command": command, "stdin": payload, "script": str(script) if script else None,
                                          "exit": result.returncode, "stdout": result.stdout, "stderr": result.stderr}) + "\n")
        self.transcript.flush()
        require(result.returncode == expected,
                f"{self.fixture}: CLI exit {result.returncode}, expected {expected}: {result.stderr}\n{result.stdout}")
        self.alive()
        return result.stdout

    def inspect(self):
        snapshot = json.loads(self.cli("inspect"))
        require(snapshot.get("schema") == "octara.inspect/2", f"{self.fixture}: wrong live snapshot schema")
        return snapshot

    def requests(self, *requests):
        payload = "".join(json.dumps(request) + "\n" for request in requests)
        replies = [json.loads(line) for line in self.cli("run", payload=payload).splitlines()]
        require(len(replies) == len(requests), f"{self.fixture}: requests/replies lost alignment")
        for request, reply in zip(requests, replies):
            require(reply.get("ok") is True, f"{self.fixture}: {request} failed: {reply}")
        return replies

    def capture(self, name, window=None):
        path = self.output / name
        request = {"op": "capture", "path": str(path), "scale": 1, "settle_ms": 2000, "pixels": "painted"}
        if window is not None:
            request["window"] = window
        report = self.requests(request)[0]["capture"]
        require(report["renderer"] == "gpu", f"{self.fixture}: capture used {report['renderer']!r}, not the window GPU")
        require(report["pixels"] == "painted", f"{self.fixture}: unexpected capture source")
        width, height, digest = png_pixels(path)
        require((report["width"], report["height"]) == (width, height), f"{self.fixture}: capture size report disagrees with PNG")
        return width, height, digest

    @contextlib.contextmanager
    def wire(self):
        connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        connection.settimeout(self.args.timeout)
        connection.connect(str(self.endpoint))
        try:
            with connection.makefile("rwb", buffering=0) as stream:
                yield stream
        finally:
            connection.close()

    def wire_request(self, stream, request):
        line = request if isinstance(request, str) else json.dumps(request)
        stream.write((line + "\n").encode())
        response = stream.readline()
        require(response, f"{self.fixture}: live connection ended without a reply")
        reply = json.loads(response)
        self.transcript.write(json.dumps({"wire": line, "reply": reply}) + "\n")
        self.transcript.flush()
        return reply

    def wait_for(self, observe, description):
        deadline = time.monotonic() + self.args.timeout
        while not (result := observe()):
            self.alive()
            require(time.monotonic() < deadline, f"{self.fixture}: timed out waiting for {description}")
            time.sleep(0.01)
        return result

    def trace_rows(self, offset, *schemas):
        labels = tuple(schema.encode() for schema in schemas)
        with Path(self.log.name).open("rb") as log:
            log.seek(offset)
            for line in log:
                if line.startswith(b"{") and line.endswith(b"\n") and any(label in line for label in labels):
                    row = json.loads(line)
                    if row.get("schema") in schemas:
                        yield row


def counter(args, output, sockets):
    with LiveWindow("counter", args, output, sockets) as app:
        first = app.inspect()
        require(find(first, "count")["name"] == "Count: 0", "counter: unexpected initial state")
        require("examples/src/apps.rs:" in find(first, "inc").get("source", ""), "counter: missing real node source")
        inc = find(first, "inc")
        x, y, width, height = inc["bounds"]
        picked = app.requests({"op": "pick", "x": x + min(4, width / 4), "y": y + height / 2,
                               "target": first["target"]})[0]
        node = picked["picked"]["node"]
        require(node.get("id") == "inc" and node["target"] == inc["target"] and node.get("source") == inc["source"],
                "counter: picking and the semantic snapshot disagree on the live element")
        require(picked.get("live", {}).get("route") == "query" and not picked.get("delivery"),
                "counter: read-only picking reported input delivery")
        before = app.capture("counter-before.png")
        app.requests({"op": "tap", "id": "inc"}, {"op": "expect", "id": "count", "text": "Count: 1"})
        after = app.capture("counter-after.png")
        require(before[:2] == after[:2] and before[2] != after[2], "counter: update did not change captured pixels")
        # Capture must leave the window and its current state available to a new connection.
        app.requests({"op": "tap", "id": "inc"}, {"op": "expect", "id": "count", "text": "Count: 2"})
        with app.wire() as stream:
            for request, code in (("{broken", "BAD_REQUEST"), ({"op": "fly"}, "BAD_REQUEST"),
                                  ({"op": "tap", "id": "absent"}, "NO_SUCH_ELEMENT"),
                                  ({"op": "inspect", "window": 999999}, "BAD_REQUEST")):
                reply = app.wire_request(stream, request)
                require(reply.get("ok") is False and reply.get("code") == code, f"counter: wrong failure: {reply}")
            for request in ({"op": "advance", "ms": 100},
                            {"op": "settle", "limit_ms": 30001},
                            {"op": "capture", "path": str(output / "invalid-scale.png"), "scale": 0},
                            {"op": "capture", "path": str(output / "invalid-limit.png"), "settle_ms": 30001}):
                reply = app.wire_request(stream, request)
                require(reply.get("ok") is False and reply.get("code") == "BAD_REQUEST",
                        f"counter: invalid/unsupported live operation was accepted: {request}: {reply}")
            settled = app.wire_request(stream, {"op": "settle", "limit_ms": 2000})
            require(settled.get("ok") is True and settled.get("live", {}).get("settled") is True
                    and settled["live"].get("pending") == [], f"counter: idle realtime settle failed: {settled}")
            require(app.wire_request(stream, {"op": "inspect"}).get("ok") is True, "counter: errors broke the connection")
        require(find(app.inspect(), "count")["name"] == "Count: 2", "counter: reconnect lost or changed live state")
        # A disconnected read has no side effect and cannot contaminate the following connection.
        with app.wire() as stream:
            stream.write(b'{"op":"inspect"}\n')
        require(find(app.inspect(), "count")["name"] == "Count: 2", "counter: disconnected reply contaminated reconnect")
        script = output / "counter-stop.jsonl"
        script.write_text('\n'.join(json.dumps(r) for r in (
            {"op": "tap", "id": "inc"}, {"op": "expect", "id": "count", "text": "Count: 3"},
            {"op": "expect", "id": "count", "text": "Count: 999"}, {"op": "tap", "id": "inc"},
        )) + '\n', encoding="utf-8")
        replies = [json.loads(line) for line in app.cli("run", script=script, expected=1).splitlines()]
        require(len(replies) == 3 and replies[-1].get("code") == "EXPECTATION_FAILED", "counter: script did not stop at its first failure")
        app.requests({"op": "expect", "id": "count", "text": "Count: 3"})
        # Losing an input's reply leaves delivery uncertain. Reread instead of resending it.
        with app.wire() as stream:
            stream.write(b'{"op":"tap","id":"inc"}\n')
        observed = find(app.inspect(), "count")["name"]
        require(observed in ("Count: 3", "Count: 4"), f"counter: disconnected input changed state unexpectedly: {observed}")
        count = int(observed.rsplit(" ", 1)[1])
        app.requests({"op": "tap", "id": "inc"}, {"op": "expect", "id": "count", "text": f"Count: {count + 1}"})


def require_realtime(before, after, started, minimum_ms, operation):
    elapsed_ms = (time.monotonic() - started) * 1000
    clock_ms = after["time_ms"] - before["time_ms"]
    require(elapsed_ms >= minimum_ms and minimum_ms <= clock_ms <= elapsed_ms + 200,
            f"{operation}: app clock jumped ahead of wall time: app={clock_ms}ms wall={elapsed_ms:.0f}ms")


def resize(app, width, height, window=None):
    request = {"op": "size", "width": width, "height": height}
    inspect = {"op": "inspect"}
    if window is not None:
        request["window"] = inspect["window"] = window
    reply, observed = app.requests(request, inspect)
    live = reply.get("live", {})
    require(live.get("route") == "native-window" and live.get("delivery") == "delivered",
            f"{app.fixture}: resize did not report native window delivery: {reply}")
    detail = live.get("resize", {})
    require(detail.get("matched") is True and detail.get("synchronized") is True,
            f"{app.fixture}: resize did not confirm native/framework agreement: {reply}")
    require(detail.get("requested") == [width, height], f"{app.fixture}: resize lost requested dimensions")
    snapshot = observed["snapshot"]
    actual = detail.get("actual", [])
    require(len(actual) == 2 and all(abs(a - b) < 0.2 for a, b in zip(snapshot["viewport"], actual)),
            f"{app.fixture}: semantic viewport disagrees with native size: {detail}, {snapshot['viewport']}")
    require(all(abs(a - b) < 1 for a, b in zip(actual, [width, height])),
            f"{app.fixture}: native window did not take the requested size: {detail}")
    physical, scale = detail.get("physical", []), detail.get("scale", 0)
    require(len(physical) == 2 and scale > 0 and all(abs(a * scale - p) < 0.2 for a, p in zip(actual, physical)),
            f"{app.fixture}: native logical and physical sizes disagree: {detail}")
    return snapshot


def gestures(args, output, sockets):
    with LiveWindow("gestures", args, output, sockets) as app:
        app.requests({"op": "expect", "clean": True})
        with app.wire() as stream:
            initial = app.wire_request(stream, {"op": "inspect"})
            operations = initial.get("live", {}).get("operations", [])
            require(all(op in operations for op in ("double_tap", "long_press", "size")),
                    f"gestures: live host did not advertise the implemented operations: {operations}")
            before = initial["snapshot"]
            started = time.monotonic()
            reply = app.wire_request(stream, {"op": "double_tap", "id": "double-target"})
            require(reply.get("ok") is True and reply.get("live", {}).get("route") == "framework"
                    and reply["live"].get("delivery") == "delivered", f"gestures: double click failed: {reply}")
            after = app.wire_request(stream, {"op": "inspect"})["snapshot"]
            require_realtime(before, after, started, 30, "double click")
            require(find(after, "doubles")["name"] == "Doubles: 1"
                    and find(after, "presses")["name"] == "Presses: 2", "gestures: double handler did not run exactly once")
            started = time.monotonic()
            reply = app.wire_request(stream, {"op": "double_tap", "id": "double-target"})
            require(reply.get("ok") is True and reply.get("live", {}).get("delivery") == "delivered",
                    f"gestures: consecutive double click failed: {reply}")
            repeated = app.wire_request(stream, {"op": "inspect"})["snapshot"]
            require_realtime(after, repeated, started, 400, "consecutive double click")
            require(time.monotonic() - started < 2, "gestures: double-click cooldown took longer than two seconds")
            require(find(repeated, "doubles")["name"] == "Doubles: 2"
                    and find(repeated, "presses")["name"] == "Presses: 4",
                    "gestures: consecutive requests became a triple/quadruple click instead of another double click")
            started = time.monotonic()
            reply = app.wire_request(stream, {"op": "long_press", "id": "hold-target"})
            require(reply.get("ok") is True and reply.get("live", {}).get("route") == "framework"
                    and reply["live"].get("delivery") == "delivered", f"gestures: long press failed: {reply}")
            held = app.wire_request(stream, {"op": "inspect"})["snapshot"]
            require_realtime(repeated, held, started, 450, "long press")
            require(find(held, "long")["name"] == "Long: 1" and find(held, "short")["name"] == "Short: 0",
                    "gestures: long press also triggered the short-press handler")
            require("pressed" not in find(held, "hold-target").get("states", []), "gestures: long press left the pointer held")
            removed = app.wire_request(stream, {"op": "double_tap", "id": "vanishing-double"})
            require(removed.get("ok") is False and removed.get("code") == "STEP_FAILED"
                    and removed.get("live", {}).get("delivery") == "delivered",
                    f"gestures: removing a double-click target lost the first click's delivery: {removed}")
            require(app.wire_request(stream, {"op": "expect", "id": "vanishing-double", "exists": False}).get("ok") is True,
                    "gestures: first click did not actually remove the target")
            require(app.wire_request(stream, {"op": "expect", "id": "vanished-doubles", "text": "Removed doubles: 0"}).get("ok") is True,
                    "gestures: double click activated a removed target")
            require(app.wire_request(stream, {"op": "tap", "id": "arm-remove"}).get("ok") is True, "gestures: could not arm removal")
            removed = app.wire_request(stream, {"op": "long_press", "id": "volatile-hold"})
            require(removed.get("ok") is False and removed.get("code") in ("STEP_FAILED", "NO_SUCH_ELEMENT"),
                    f"gestures: long press succeeded after target removal: {removed}")
            require(app.wire_request(stream, {"op": "expect", "id": "volatile-hold", "exists": False}).get("ok") is True,
                    "gestures: the pending removal did not execute on the real app clock")
            require(app.wire_request(stream, {"op": "expect", "id": "volatile-longs", "text": "Removed long presses: 0"}).get("ok") is True,
                    "gestures: long press activated a removed target")
            for op in ("double_tap", "long_press"):
                refused = app.wire_request(stream, {"op": op, "id": "missing"})
                require(refused.get("ok") is False and refused.get("code") == "NO_SUCH_ELEMENT"
                        and refused.get("live", {}).get("delivery") == "refused", f"gestures: missing target was not refused: {refused}")
        # A lost reply leaves delivery uncertain, but it must never leave a synthetic finger down.
        with app.wire() as stream:
            stream.write(b'{"op":"long_press","id":"hold-target"}\n')
            threading.Event().wait(0.15)
        recovered = app.inspect()
        require("pressed" not in find(recovered, "hold-target").get("states", []), "gestures: disconnect left a held pointer")
        require(find(recovered, "long")["name"] in ("Long: 1", "Long: 2"), "gestures: disconnected input ran more than once")
        app.requests({"op": "tap", "id": "hold-target"}, {"op": "expect", "id": "short", "text": "Short: 1"})
        resized = resize(app, 500, 560)
        root = find(resized, "gesture-root")["bounds"]
        require(all(abs(a - b) < 0.2 for a, b in zip(root, [0, 0, *resized["viewport"]])),
                f"gestures: root layout did not follow the native resize: {root}")
        image = app.capture("gestures-resized.png")
        require(image[:2] == tuple(round(v) for v in resized["viewport"]), "gestures: GPU capture did not follow native resize")
        # Asking for the current size must work even when the OS emits no new resize event.
        unchanged = resize(app, *resized["viewport"])
        require(unchanged["viewport"] == resized["viewport"], "gestures: repeated size request changed the viewport")
        app.requests({"op": "expect", "clean": True})
        with app.wire() as stream:
            for width, height in ((0, 400), (-1, 400), (500, 0), (9e30, 400)):
                refused = app.wire_request(stream, {"op": "size", "width": width, "height": height})
                require(refused.get("ok") is False and refused.get("code") == "BAD_REQUEST",
                        f"gestures: invalid size was accepted: {refused}")
            after = app.wire_request(stream, {"op": "inspect"})["snapshot"]
            require(after["viewport"] == resized["viewport"], "gestures: an invalid size changed the real window")
        app.requests({"op": "tap", "id": "hold-target"}, {"op": "expect", "id": "short", "text": "Short: 2"})


def search(args, output, sockets):
    with LiveWindow("search", args, output, sockets) as app:
        query = "Live Agent timer"
        with app.wire() as stream:
            for request in ({"op": "focus", "id": "query"}, {"op": "key", "chord": "mod+a"}):
                require(app.wire_request(stream, request).get("ok") is True, f"search: could not prepare query field: {request}")
            before = app.wire_request(stream, {"op": "inspect"})["snapshot"]
            started = time.monotonic()
            require(app.wire_request(stream, {"op": "type", "id": "query", "text": query}).get("ok") is True,
                    "search: typing the query failed")
            pending = app.wire_request(stream, {"op": "settle", "limit_ms": 0})
            require(pending.get("ok") is False and pending.get("code") == "STEP_FAILED"
                    and pending.get("live", {}).get("settled") is False and pending["live"].get("pending"),
                    f"search: zero-budget settle did not report the pending timer: {pending}")
            settled = app.wire_request(stream, {"op": "settle", "limit_ms": 2000})
            require(settled.get("ok") is True and settled.get("live", {}).get("settled") is True
                    and settled["live"].get("pending") == [], f"search: timer did not settle in realtime: {settled}")
            after = app.wire_request(stream, {"op": "inspect"})["snapshot"]
            elapsed_ms = (time.monotonic() - started) * 1000
            clock_ms = after["time_ms"] - before["time_ms"]
            require(elapsed_ms >= 400 and clock_ms >= 400 and clock_ms <= elapsed_ms + 200,
                    f"search: app clock jumped ahead of wall time: app={clock_ms}ms wall={elapsed_ms:.0f}ms")
            require(find(after, "query").get("value") == query, "search: live field did not keep the typed query")
            require(any(query in node.get("name", "") and "results for" in node.get("name", "")
                        for node in nodes(after)), "search: settled resource did not show the typed query's results")


def todo(args, output, sockets):
    with LiveWindow("todo", args, output, sockets) as app:
        app.requests({"op": "type", "id": "new-todo", "text": "Live Agent"},
                     {"op": "expect", "id": "new-todo", "value": "Live Agent"},
                     {"op": "tap", "id": "add"},
                     {"op": "expect", "id": "title-31", "text": "Live Agent"},
                     {"op": "expect", "id": "new-todo", "value": ""},
                     {"op": "expect", "id": "left", "text": "31 left"})
        app.requests({"op": "tap", "id": "remove-1"}, {"op": "expect", "id": "todo-1", "exists": False},
                     {"op": "expect", "id": "left", "text": "30 left"})

        observed = app.requests({"op": "inspect"})[0]
        require("scroll" in observed["live"]["operations"], "todo: live host did not advertise scroll")
        snapshot = observed["snapshot"]
        offscreen = find(snapshot, "toggle-31")
        require(offscreen["bounds"][1] >= snapshot["viewport"][1], "todo: expected last row below the window")
        with app.wire() as stream:
            refused = app.wire_request(stream, {"op": "tap", "target": offscreen["target"]})
        require(not refused["ok"] and refused["code"] == "STEP_FAILED" and refused["delivery"] == "refused",
                f"todo: offscreen tap was not refused: {refused}")

        offset = 0
        for _ in range(12):
            viewport = find(snapshot, "todos")
            require("scroll" in viewport["actions"], "todo: viewport did not advertise scroll")
            reply = app.requests({"op": "scroll", "target": viewport["target"], "dx": 0, "dy": 120})[0]
            require(reply["delivery"] == "delivered" and reply["live"]["route"] == "semantic",
                    f"todo: wrong scroll delivery: {reply}")
            dx, dy = reply["scroll"]["delta"]
            x, y = reply["scroll"]["offset"]
            require(dx == x == 0 and dy >= 0 and abs(y - offset - dy) < 0.01, f"todo: inconsistent scroll movement: {reply}")
            offset = y
            snapshot = app.inspect()
            if dy == 0:
                break
        else:
            raise SmokeFailure("todo: scrolling did not reach the end")

        viewport = find(snapshot, "todos")
        last = find(snapshot, "toggle-31")
        _, viewport_y, _, viewport_height = viewport["bounds"]
        _, row_y, _, row_height = last["bounds"]
        require(offset > 0 and viewport_y <= row_y + row_height / 2 < viewport_y + viewport_height,
                "todo: last item was not brought into the viewport")
        app.requests({"op": "tap", "target": last["target"]},
                     {"op": "expect", "target": last["target"], "checked": True},
                     {"op": "expect", "id": "left", "text": "29 left"})
        app.capture("todo-after.png")
        print(f"todo: row y {offscreen['bounds'][1]:g} -> {row_y:g}, scroll offset {offset:g}, edge delta 0, checked", flush=True)


def windows(args, output, sockets):
    with LiveWindow("windows", args, output, sockets) as app:
        app.requests({"op": "tap", "id": "new"})
        snapshot = app.inspect()
        notes = [w for w in snapshot["windows"] if w["title"] == "Note 1"]
        require(len(notes) == 1, "windows: did not open the real note window")
        note = notes[0]["window"]
        app.requests({"op": "type", "window": note, "id": "body", "text": "A live note"},
                     {"op": "expect", "window": note, "id": "body", "value": "A live note"})
        main_size = snapshot["viewport"]
        note_snapshot = resize(app, 440, 300, window=note)
        note_image = app.capture("note.png", window=note)
        require(note_image[:2] == tuple(round(v) for v in note_snapshot["viewport"]), "windows: secondary capture has the wrong size")
        require(app.inspect()["viewport"] == main_size, "windows: resizing the note also resized the main window")
        app.requests({"op": "tap", "window": note, "id": "close"})
        with app.wire() as stream:
            for request in ({"op": "inspect", "window": note}, {"op": "size", "window": note, "width": 400, "height": 250},
                            {"op": "double_tap", "window": note, "id": "close"}, {"op": "long_press", "window": note, "id": "close"}):
                reply = app.wire_request(stream, request)
                require(reply.get("ok") is False and reply.get("code") == "BAD_REQUEST", "windows: closed target remained valid")
        app.requests({"op": "expect", "id": "opened", "text": "1 notes opened"})
        app.requests({"op": "reload"}, {"op": "expect", "id": "opened", "text": "1 notes opened"},
                     {"op": "tap", "id": "new"})
        reopened = [w for w in app.inspect()["windows"] if w["title"] == "Note 2"]
        require(len(reopened) == 1 and reopened[0]["window"] != note,
                "windows: App state or window identity did not survive close/reload/reopen")
        app.requests({"op": "tap", "window": reopened[0]["window"], "id": "close"},
                     {"op": "expect", "id": "opened", "text": "2 notes opened"})


def activation(args, output, sockets):
    with LiveWindow("activation", args, output, sockets) as app:
        resize(app, 800, 640)
        first = app.inspect()
        field_target = find(first, "field")["target"]
        before = app.capture("activation-before.png")

        def expect(identifier, name):
            snapshot = app.inspect()
            require(find(snapshot, identifier)["name"] == name, f"activation: {identifier} expected {name}")
            return snapshot

        for probe in ("probe.semantic", "probe.accessibility"):
            app.requests({"op": "invoke", "command": probe})
            expect("accepted", "Accepted: true")
        expect("hits", "Hits: 2")
        for blurs, mode in enumerate(("disable", "hide", "modal"), 1):
            app.requests({"op": "focus", "id": "field", "target": field_target},
                         {"op": "invoke", "command": mode})
            retired = expect("blurs", f"Blurs: {blurs}")
            require(retired.get("focus") is None, "activation: ineligible field kept focus")
            for probe in ("probe.semantic", "probe.accessibility"):
                app.requests({"op": "invoke", "command": probe})
                expect("accepted", "Accepted: false")
            expect("hits", "Hits: 2")
            if mode == "modal":
                app.requests({"op": "key", "chord": "escape"})
                expect("dismissals", "Dismissals: 1")
            else:
                app.requests({"op": "invoke", "command": "enable"})
            restored = app.inspect()
            require(find(restored, "field")["target"] == field_target, "activation: retained field was replaced")
            require(find(restored, "field")["value"] == "中文 input", "activation: retiring focus changed committed text")
        app.requests({"op": "tap", "id": "activate"},
                     {"op": "type", "id": "field", "text": "测试"})
        restored = expect("hits", "Hits: 3")
        require("测试" in find(restored, "field")["value"], "activation: re-enabled field rejects framework text")
        app.requests({"op": "expect", "clean": True})
        after = app.capture("activation-after.png")
        require(before[:2] == after[:2] and before[2] != after[2], "activation: updates did not reach GPU pixels")
        app.requests({"op": "reload", "target": restored["target"]})
        require(find(app.inspect(), "field")["target"] != field_target, "activation: reload retained an old allocation")
        resize(app, 390, 640)
        app.requests({"op": "expect", "clean": True})
        app.capture("activation-reloaded.png")


def handlers(args, output, sockets):
    with LiveWindow("handlers", args, output, sockets) as app:
        resize(app, 800, 640)
        before = app.capture("handlers-before.png")
        app.requests({"op": "type", "id": "editor", "text": "你好"})

        def expect(identifier, name):
            snapshot = app.inspect()
            require(find(snapshot, identifier)["name"] == name, f"handlers: {identifier} expected {name}")
            return snapshot

        expect("model", "Model: 你好")
        expect("seen", "Seen: 你好")
        app.requests({"op": "key", "chord": "left"})
        expect("keys", "Keys: 1")
        app.requests({"op": "key", "chord": "down"}, {"op": "key", "chord": "down"})
        expect("keys", "Keys: 1")
        require(find(app.inspect(), "editor")["active_descendant"] == "editor-option-two", "handlers: popup navigation failed")
        app.capture("handlers-suggestions.png")
        app.requests({"op": "key", "chord": "enter"})
        expect("chosen", "Chosen: two")
        expect("keys", "Keys: 1")
        app.requests({"op": "key", "chord": "down"}, {"op": "focus", "id": "other"})
        expect("blur", "Blur: open=false")
        app.requests({"op": "expect", "clean": True})
        after = app.capture("handlers-after.png")
        require(before[:2] == after[:2] and before[2] != after[2], "handlers: changes did not reach GPU pixels")


def bindings(args, output, sockets):
    with LiveWindow("bindings", args, output, sockets) as app:
        resize(app, 800, 640)
        before = app.capture("bindings-before.png")

        def expect(identifier, name):
            require(find(app.inspect(), identifier)["name"] == name, f"bindings: {identifier} expected {name}")

        app.requests({"op": "tap", "id": "check"})
        expect("observed", "check:true")
        app.requests({"op": "key", "chord": "space"})
        expect("observed", "check:false")
        app.requests({"op": "tap", "id": "switch"})
        expect("observed", "switch:true")
        app.requests({"op": "set_value", "id": "level", "value": 6.3})
        expect("observed", "numeric:6.3:6.5")
        app.requests({"op": "focus", "id": "level"}, {"op": "key", "chord": "right"})
        expect("level-state", "Level: 7 / Keys: 0")
        app.requests({"op": "key", "chord": "a"})
        expect("level-state", "Level: 7 / Keys: 1")
        app.requests({"op": "tap", "id": "level"})
        level = find(app.inspect(), "level")["numeric_value"]
        expect("observed", f"press:{level:g}")
        app.requests({"op": "drag", "id": "level", "dx": 60, "dy": 0})
        level = find(app.inspect(), "level")["numeric_value"]
        require(level > 5, "bindings: drag did not update the slider")
        expect("observed", f"drag:End:{level:g}")

        def draft(value):
            app.requests({"op": "focus", "id": "number"}, {"op": "key", "chord": "mod+a"},
                         {"op": "type", "text": value})
            expect("raw", f"Raw: {value}")
            expect("observed", f"input:{value}")

        draft(" 6.30 ")
        app.requests({"op": "key", "chord": "enter"})
        expect("raw", "Raw: 6.5")
        expect("observed", "submit:6.5")
        draft("99")
        app.requests({"op": "focus", "id": "outside"})
        expect("observed", "blur:10")
        draft("1e")
        app.requests({"op": "key", "chord": "enter"})
        expect("observed", "submit:1e")
        app.requests({"op": "set_value", "id": "number", "value": 3.2})
        expect("observed", "number:3.2:3")
        app.requests({"op": "tap", "id": "details"})
        expect("observed", "popup:true")
        require(app.inspect()["focus"] == "details-input", "bindings: popup did not focus its editor")
        app.capture("bindings-popover.png")
        app.requests({"op": "key", "chord": "escape"})
        require(app.inspect()["focus"] == "details", "bindings: popup did not restore trigger focus")
        app.requests({"op": "focus", "id": "outside"}, {"op": "tap", "id": "actions"})
        expect("observed", "menu:true")
        app.capture("bindings-menu.png")
        app.requests({"op": "tap", "id": "actions-action-bindings.choose"})
        expect("observed", "command:menu=false")
        require(app.inspect()["focus"] == "outside", "bindings: menu did not restore editor focus")
        app.requests({"op": "focus", "id": "actions"}, {"op": "key", "chord": "a"})
        expect("observed", "menu:key")
        app.requests({"op": "key", "chord": "down"})
        expect("observed", "menu:key")
        require(app.inspect()["focus"] == "actions-action-bindings.choose", "bindings: keyboard menu opening failed")
        app.requests({"op": "key", "chord": "escape"})
        require(app.inspect()["focus"] == "actions", "bindings: menu did not restore trigger focus")
        resize(app, 390, 640)
        app.requests({"op": "expect", "clean": True})
        app.capture("bindings-narrow.png")
        resize(app, 800, 640)
        app.requests({"op": "expect", "clean": True})
        after = app.capture("bindings-after.png")
        require(before[:2] == after[:2] and before[2] != after[2], "bindings: changes did not reach GPU pixels")


def styles(args, output, sockets):
    with LiveWindow("styles", args, output, sockets) as app:
        def widths(expected):
            snapshot = app.inspect()
            actual = [find(snapshot, key)["bounds"][2] for key in ("static-last", "dynamic-last", "patch")]
            require(all(abs(a - b) < 0.1 for a, b in zip(actual, expected)),
                    f"styles: widths {actual}, expected {expected}")
            app.requests({"op": "expect", "clean": True})
            return snapshot

        def tap(identifier):
            target = find(app.inspect(), identifier)["target"]
            reply = app.requests({"op": "tap", "id": identifier, "target": target})[0]
            require(reply.get("live", {}).get("route") == "framework" and reply.get("delivery") == "delivered",
                    "styles: input route/delivery differs")

        resize(app, 390, 640)
        first = widths([80, 96, 160])
        identities = {key: find(first, key)["target"] for key in ("static-last", "dynamic-last", "patch")}
        before = app.capture("styles-before.png")
        tap("early")
        widths([80, 96, 160])
        tap("late")
        widths([80, 144, 160])
        tap("toggle-patch")
        updated = widths([80, 144, 112])
        require(all(find(updated, key)["target"] == target for key, target in identities.items()),
                "styles: a style update replaced a retained node")
        after = app.capture("styles-after.png")
        require(before[:2] == after[:2] and before[2] != after[2], "styles: changed styles did not reach GPU pixels")
        app.requests({"op": "reload", "target": updated["target"]})
        reloaded = widths([80, 144, 112])
        require(all(find(reloaded, key)["target"] != target for key, target in identities.items()),
                "styles: reload did not retire observed identities")
        resize(app, 800, 640)
        widths([120, 144, 112])
        resize(app, 390, 640)
        widths([80, 144, 112])
        tap("toggle-patch")
        widths([80, 144, 160])
        app.capture("styles-restored.png")


def content(args, output, sockets):
    for card in ("images", "drawing"):
        with LiveWindow(card, args, output, sockets, ["showcase", card]) as app:
            app.requests({"op": "expect", "clean": True})
            before = app.capture(f"{card}-before.png")
            if card == "images":
                expected = {
                    "Landscape at its natural aspect ratio": [200, 125],
                    "Landscape cropped to a circle": [96, 96],
                    "Landscape fitted inside a frame": [120, 96],
                }
                sizes = {n["name"]: n["bounds"][2:] for n in nodes(app.inspect())
                         if n["role"] == "image" and n.get("name") in expected}
                require(sizes == expected, f"images: accessible image names and sizes differ: {sizes}")
            if card == "drawing":
                chart = find(app.inspect(), "chart")
                require(chart["role"] == "image" and "Dec selected" in chart["name"], "canvas semantics differ")
                app.requests({"op": "drag", "id": "chart", "target": chart["target"], "dx": -80, "dy": 0})
                changed = find(app.inspect(), "chart")
                require(changed["target"] == chart["target"] and changed["name"] != chart["name"],
                        "canvas interaction lost its binding or replaced its node")
                after = app.capture("drawing-after.png")
                require(before[:2] == after[:2] and before[2] != after[2], "canvas update did not change GPU pixels")
            app.requests({"op": "reload", "target": app.inspect()["target"]}, {"op": "expect", "clean": True})
            app.capture(f"{card}-reloaded.png")

    if sys.platform != "darwin":
        print("SKIP native web content: this fixture requires the macOS WebKit host", flush=True)
        return
    with LiveWindow("web", args, output, sockets, ["showcase", "web"]) as app:
        def loaded(title, after=0):
            deadline = time.monotonic() + min(args.timeout, 10)
            while True:
                snapshot = app.inspect()
                status = find(snapshot, "web_status")["name"]
                count = int(status.rsplit("loaded ", 1)[1].split(" ", 1)[0])
                if title in status and count > after:
                    return snapshot, count
                require(time.monotonic() < deadline, f"web: no native title/load callback for {title}: {status}")
                threading.Event().wait(0.05)

        def capture(name):
            path = output / name
            report = app.requests({"op": "capture", "target": app.inspect()["target"],
                                   "path": str(path), "scale": 1, "settle_ms": 5000, "pixels": "painted"})[0]["capture"]
            require(report["renderer"] == "gpu" and report["pixels"] == "painted", "web: wrong capture source")
            require(any(n.get("id") == "web" and n["shown"] == "snapshot" for n in report["native"]),
                    f"web: native pixels were not captured: {report['native']}")
            return png_pixels(path)

        first, count = loaded("First page")
        web = find(first, "web")
        require(web["native"]["kind"] == "web" and web["native"]["slot"] == "overlay", "web: native content differs")
        before = capture("web-before.png")
        app.requests({"op": "tap", "id": "other_page", "target": find(first, "other_page")["target"]})
        second, count = loaded("Second page", count)
        require(find(second, "web")["target"] == web["target"], "native props update replaced the view")
        require("Second page" in find(second, "web")["native"]["props"]["html"], "native props binding did not update")
        after = capture("web-after.png")
        require(before[:2] == after[:2] and before[2] != after[2], "web: changed page did not change captured pixels")
        app.requests({"op": "reload", "target": second["target"]})
        rebuilt, _ = loaded("Second page", count)
        require(find(rebuilt, "web")["target"] != web["target"], "web: reload retained a retired native target")
        app.requests({"op": "expect", "clean": True})
        capture("web-reloaded.png")


def gpu_lab(args, output, sockets, *, scale=1):
    with LiveWindow("gpu_lab", args, output, sockets) as app:
        def color_digest(color):
            return hashlib.sha256(bytes(color) * 16).hexdigest()

        def capture(name, snapshot, region=None):
            path = output / name
            report = app.requests({"op": "capture", "target": snapshot["target"], "path": str(path),
                                   "scale": scale, "settle_ms": 2000, "pixels": "painted"})[0]["capture"]
            require(report["renderer"] == "gpu" and report["pixels"] == "painted" and report["settled"],
                    f"gpu_lab: wrong capture source or unsettled frame: {report}")
            if region is None:
                x, y, w, h = find(snapshot, "gpu-preview")["bounds"]
                region = tuple(int(value * scale) for value in (x + w / 4, y + h / 4, w / 2, h / 2))
            width, height, digest = png_pixels(path, region=region)
            expected = tuple(math.ceil(side * scale) for side in snapshot["viewport"])
            require((width, height) == expected, f"gpu_lab: wrong device pixels, expected {expected}, got {(width, height)}")
            return digest

        initial = app.inspect()
        preview = find(initial, "gpu-preview")
        require("Prism" in find(initial, "gpu-selected-material")["name"], "gpu_lab: wrong initial material")
        original = capture("gpu-lab.png", initial)
        sx, sy, sw, sh = find(initial, "gpu-swatch-prism")["bounds"]
        swatch = {png_pixels(output / "gpu-lab.png", region=(int((sx + sw * fraction) * scale), int((sy + sh / 2) * scale), 4, 4))[2]
                  for fraction in (0.25, 0.75)}
        require(len(swatch) == 2 and color_digest((0x12, 0x12, 0x22, 0xff)) not in swatch, "gpu_lab: swatch shader pixels missing")

        ember = find(initial, "gpu-material-ember")
        app.requests({"op": "tap", "target": ember["target"]})
        changed = app.inspect()
        require(changed.get("focus") == "gpu-material-ember", "gpu_lab: material action lost keyboard focus")
        require("Ember" in find(changed, "gpu-selected-material")["name"], "gpu_lab: selection did not change")
        require(find(changed, "gpu-preview")["target"] == preview["target"], "gpu_lab: selecting a material remounted its preview")
        require(capture("gpu-lab-ember.png", changed) != original, "gpu_lab: selection left the same canvas pixels")

        resized = resize(app, 800, 620)
        viewport = find(resized, "gpu-materials")
        reply = app.requests({"op": "scroll", "target": viewport["target"], "dx": 0, "dy": 180})[0]
        require(reply["scroll"]["delta"][1] > 0, "gpu_lab: shared material list did not scroll")
        resized = app.inspect()
        _, _, width, height = find(resized, "gpu-preview")["bounds"]
        require(width < preview["bounds"][2] and height > 0, "gpu_lab: native resize left the same preview bounds")
        app.requests({"op": "tap", "target": find(resized, "gpu-pass-open")["target"]})
        opened = app.inspect()
        popup = find(opened, "gpu-pass-popover")
        x, y, w, h = find(opened, "gpu-preview")["bounds"]
        px, py, pw, ph = popup["bounds"]
        require(x < px + pw and px < x + w and y < py + ph and py < y + h, "gpu_lab: popup does not overlap the drawing")
        sample_x, sample_y = px + 5, py + ph / 2
        require(x <= sample_x and sample_x + 4 <= x + w and y <= sample_y and sample_y + 4 <= y + h,
                "gpu_lab: popup sample did not cover canvas pixels")
        popup_color = capture("gpu-lab-passes.png", opened, region=(int(sample_x * scale), int(sample_y * scale), 4, 4))
        require(popup_color == color_digest((0x22, 0x23, 0x2e, 0xff)), "gpu_lab: popup did not draw over the canvas")
        app.requests({"op": "tap", "target": find(opened, "gpu-pass-close")["target"]},
                     {"op": "expect", "id": "gpu-pass-popover", "exists": False}, {"op": "expect", "clean": True})


def glass_effect(paint, target):
    effects = [effect for effect in paint.get("effects", []) if effect["target"] == target]
    require(len(effects) == 1 and effects[0]["id"] == "gpu-glass-dialog",
            f"expected one paint outcome for the Glass dialog: {paint}")
    return effects[0]


def gpu_glass(args, output, sockets, *, scale=1, probe_fallback=False):
    env = {"OCTARA_GPU_LAB_PROBE_FALLBACK": "1", "OCTARA_TRACE_GPU": "1"} if probe_fallback else None
    with LiveWindow("gpu_glass", args, output, sockets, ["gpu_lab"], env=env) as app:
        narrow = resize(app, 800, 620)
        opener = find(narrow, "gpu-glass-open")["target"]
        app.requests({"op": "tap", "target": opener})
        opened = app.inspect()
        dialog = find(opened, "gpu-glass-dialog")
        slider = find(opened, "gpu-glass-refraction")["target"]
        close = find(opened, "gpu-glass-cancel")["target"]
        require(opened.get("focus") == "gpu-glass-cancel", "gpu_glass: dialog did not receive focus")
        x, y, w, h = dialog["bounds"]
        region = tuple(int(v * scale) for v in (x + w - 14, y + h / 4, 8, h / 3))

        def capture(name, settle_ms):
            return {"op": "capture", "target": opened["target"], "path": str(output / name),
                    "scale": scale, "pixels": "painted", "settle_ms": settle_ms}

        def sample(name, outcome="rendered"):
            info = app.requests(capture(name, 2000))[0]["capture"]
            require(info["renderer"] == "gpu" and info["settled"], f"gpu_glass: expected settled GPU pixels: {info}")
            paint = info["paint"]
            require(paint["origin"] == "rerecorded" and "frame" not in paint, f"gpu_glass: incorrect capture provenance: {paint}")
            effect = glass_effect(paint, dialog["target"])
            require(effect["outcome"] == outcome and effect["source"] == dialog["source"],
                    f"gpu_glass: expected {outcome} paint from the dialog site: {effect}")
            width, height, digest = png_pixels(output / name, region=region)
            require((width, height) == (800 * scale, 620 * scale), "gpu_glass: wrong capture dimensions")
            return paint, effect, digest

        def inspect_submission(paint, expected):
            reply = app.requests({"op": "inspect"})[0]
            surface = reply["completion"]["surface"]
            if surface["state"] != "submitted":
                return None
            submitted = reply["paint"]
            require(submitted["revision"] == paint["revision"] and submitted["frame"] == surface["frame"],
                    f"gpu_glass: submitted paint and capture disagree on the settled revision: {submitted}")
            require(glass_effect(submitted, dialog["target"])["outcome"] == expected and submitted["origin"] == "submitted",
                    f"gpu_glass: Inspector reported the wrong submitted paint: {submitted}")
            cli = app.inspect()["paint"]
            require(cli["revision"] == submitted["revision"] and glass_effect(cli, dialog["target"])["outcome"] == expected,
                    f"gpu_glass: CLI lost the live paint evidence: {cli}")
            return submitted

        _, _, initial = sample("gpu-glass.png")
        app.requests({"op": "set_value", "target": slider, "value": 0.15})
        require(find(app.inspect(), "gpu-glass-refraction-value")["name"] == "15%", "gpu_glass: refraction binding did not update")
        adjusted, _, changed = sample("gpu-glass-adjusted.png")
        require(changed != initial, "gpu_glass: refraction did not change the backdrop pixels")
        last_good = failed_submission = None
        if probe_fallback:
            last_good = inspect_submission(adjusted, "rendered")
            app.requests({"op": "set_value", "target": slider, "value": 1.0})
            failed, effect, pixels = sample("gpu-glass-fallback.png", "fallback")
            expected = hashlib.sha256(bytes((0x28, 0x22, 0x3b, 0xff)) * (region[2] * region[3])).hexdigest()
            require(effect["reason"] == "encoding_failed" and pixels == expected,
                    f"gpu_glass: validation failure did not render the styled fallback: {effect}")
            failed_submission = inspect_submission(failed, "fallback")

        with app.wire() as connection:
            def request(step):
                result = app.wire_request(connection, step)
                require(result["ok"], f"gpu_glass: {step} failed: {result}")
                return result

            request({"op": "tap", "target": close})
            retired = request({"op": "inspect"})["snapshot"]
            require(retired.get("focus") == "gpu-glass-open", "gpu_glass: dismissal did not restore opener focus")
            require(not any(node.get("id") == "gpu-glass-dialog" for node in nodes(retired)), "gpu_glass: retired dialog was still interactive")
            exit_capture = request(capture("gpu-glass-exit.png", 0))["capture"]
            require(bool(exit_capture.get("pending")), "gpu_glass: exit was not active when captured")
            outgoing = glass_effect(exit_capture["paint"], dialog["target"])
            require(outgoing["exiting"], f"gpu_glass: exit still claims an actionable mount: {outgoing}")
            if last_good:
                require(outgoing["outcome"] == "retained" and outgoing["source_revision"] == last_good["revision"],
                        f"gpu_glass: exit retained the wrong submission: {outgoing}")
            elif probe_fallback:
                require((outgoing["outcome"] == "fallback" and outgoing["reason"] == "history_unavailable")
                        or (outgoing["outcome"] == "retained" and outgoing["source_revision"] <= adjusted["revision"]),
                        f"gpu_glass: exit reported an unavailable source as rendered: {outgoing}")
            request({"op": "tap", "target": opener})
            reopened = request({"op": "inspect"})["snapshot"]
            require(reopened["time_ms"] - retired["time_ms"] < 800, "gpu_glass: dialog reopened after the prior fade ended")
            new_target = find(reopened, "gpu-glass-dialog")["target"]
            require(new_target != dialog["target"], "gpu_glass: reopening reused the retired dialog")
            require(find(reopened, "gpu-glass-refraction-value")["name"] == "75%", "gpu_glass: new dialog inherited retired control state")
            if probe_fallback:
                new_paint = request(capture("gpu-glass-reopened.png", 0))["capture"]["paint"]
                new_effect = glass_effect(new_paint, new_target)
                require(new_effect["outcome"] == "rendered" and not new_effect["exiting"],
                        f"gpu_glass: reopened mount used the previous effect's outcome: {new_effect}")
            refused = app.wire_request(connection, {"op": "set_value", "target": slider, "value": 0.7})
            require(refused.get("code") == "STALE_TARGET" and refused.get("delivery") == "refused",
                    f"gpu_glass: retired slider accepted input: {refused}")
        app.requests({"op": "expect", "clean": True})
        if failed_submission:
            require(any(trace.get("paint") == failed_submission for trace in app.trace_rows(0, "octara.frame_trace/1")),
                    "gpu_glass: frame trace and Inspector disagree on the failed submission")


def gpu_paint_evidence(args, output, sockets):
    gpu_glass(args, output, sockets, probe_fallback=True)


def gpu_stale_paint(args, output, sockets):
    frame_schema = "octara.frame_trace/1"
    input_schema = "octara.inspect_delivery/1"
    pending_schema = "octara.pending_presentation/1"
    helper = sockets / "live-window"
    command = ["swiftc", "-module-cache-path", str(sockets / "swift-cache"), str(args.repo / "examples/live_window.swift"),
               "-o", str(helper)]
    built = subprocess.run(command, text=True, capture_output=True, timeout=args.timeout)
    require(built.returncode == 0, f"gpu_stale_paint: could not build the native resize helper: {built.stderr}")
    env = {"WGPU_BACKEND": "metal", "OCTARA_TRACE_INPUT": "1", "OCTARA_ASYNC_METAL": "1",
           "OCTARA_RENDER_WORKER": "1", "OCTARA_RENDER_WORKER_HOLD_MS": "1500"}
    with LiveWindow("gpu_stale_paint", args, output, sockets, ["gpu_lab"], env=env) as app:
        narrow = resize(app, 800, 620)
        window = narrow["window"]
        opener = find(narrow, "gpu-glass-open")["target"]
        log = Path(app.log.name)
        with app.wire() as stream:
            def send(op, **fields):
                reply = app.wire_request(stream, {"op": op, "window": window, **fields})
                require(reply.get("ok"), f"gpu_stale_paint: {op} failed: {reply}")
                return reply

            def visible_submission():
                reply = send("inspect")
                host = reply["live"]["window"]
                completion = reply["completion"]
                surface = completion["surface"]
                if completion["layout"] == "settled" and surface["state"] == "submitted" and host["visible"] and not host["occluded"]:
                    paint = reply["paint"]
                    require(paint["origin"] == "submitted" and paint["window"] == window and paint["frame"] == surface["frame"],
                            f"gpu_stale_paint: mismatched submission evidence: {reply}")
                    return reply
                return None

            send("tap", target=opener)
            send("settle", limit_ms=2000)
            before = app.wait_for(visible_submission, "the settled Glass window to submit (desktop must be unlocked)")
            accepted = before["paint"]
            opened = before["snapshot"]
            dialog = find(opened, "gpu-glass-dialog")["target"]
            close = find(opened, "gpu-glass-cancel")["target"]
            slider = find(opened, "gpu-glass-refraction")["target"]
            effect = glass_effect(accepted, dialog)
            require(effect["outcome"] == "rendered" and not effect["exiting"], f"gpu_stale_paint: bad baseline: {effect}")
            accepted_trace = next((row for row in app.trace_rows(0, frame_schema) if row["paint"] == accepted), None)
            require(accepted_trace and accepted_trace["render_worker"] and accepted_trace["submitted"],
                    "gpu_stale_paint: Glass baseline did not submit through the Metal render worker")
            trace_window = accepted_trace["window"]

            offset = log.stat().st_size
            edit = send("set_value", target=slider, value=0.15)
            require(edit["delivery"] == "delivered", "gpu_stale_paint: the refraction setting was not delivered")
            change = next((row for row in app.trace_rows(offset, input_schema)
                           if row["window"] == window and row["route"] == "semantic"), None)
            require(change and change["before_revision"] == accepted["revision"] and change["revision"] > accepted["revision"],
                    f"gpu_stale_paint: the edit did not publish a newer drawing: {change}")

            def parked_submission():
                return next((row for row in app.trace_rows(offset, pending_schema)
                             if row["paint"]["window"] == window and row["paint"]["revision"] == change["revision"]), None)

            parked = app.wait_for(parked_submission, "the edited GPU drawing to park (use a debug window build)")
            candidate = parked["paint"]
            effect = glass_effect(candidate, dialog)
            require(effect["outcome"] == "rendered" and not effect["exiting"] and candidate["frame"] == accepted["frame"] + 1
                    and candidate["surface_generation"] == accepted["surface_generation"]
                    and candidate["device_generation"] == accepted["device_generation"],
                    f"gpu_stale_paint: the parked drawing did not produce Glass on the original surface: {candidate}")

            # Inspector's size receipt waits for surface configuration. Resize out of band so
            # this connection can dismiss Glass while the worker still owns its old drawable.
            width = 840
            sized = subprocess.run([str(helper), "size", str(app.process.pid), str(width), "640"],
                                   text=True, capture_output=True, timeout=args.timeout)
            require(sized.returncode == 0, f"gpu_stale_paint: native resize failed ({sized.returncode}): {sized.stderr.strip()}")

            def resized_layout():
                reply = send("inspect")
                return reply if reply["snapshot"]["viewport"][0] == width else None

            resized = app.wait_for(resized_layout, "the native resize to reach the framework")
            viewport = resized["snapshot"]["viewport"]
            require(resized["completion"]["surface"]["state"] == "pending" and resized["live"]["window"]["frames"] == accepted["frame"]
                    and find(resized["snapshot"], "gpu-glass-refraction-value")["name"] == "15%",
                    "gpu_stale_paint: resize did not leave the edited Glass layout awaiting submission")
            closed = send("tap", target=close)
            require(closed["delivery"] == "delivered", "gpu_stale_paint: Glass dismissal was not delivered")
            retired = send("inspect")["snapshot"]
            require(retired.get("focus") == "gpu-glass-open" and not any(node.get("id") == "gpu-glass-dialog" for node in nodes(retired)),
                    "gpu_stale_paint: dismissed Glass remained interactive")

            # With zero settle time the capture pins the exit drawing before GPU readback can start.
            path = output / "gpu-stale-paint-exit.png"
            report = send("capture", path=str(path), scale=1, pixels="painted", settle_ms=0)["capture"]
            paint = report["paint"]
            outgoing = glass_effect(paint, dialog)
            require(report["renderer"] == "gpu" and report["pixels"] == "painted" and report.get("pending")
                    and paint["origin"] == "rerecorded" and paint["window"] == window and outgoing["exiting"]
                    and outgoing["outcome"] == "retained" and outgoing["source_revision"] == accepted["revision"],
                    f"gpu_stale_paint: exit did not use the accepted Glass revision {accepted['revision']}: {outgoing}")
            require(png_pixels(path)[:2] == tuple(math.ceil(side) for side in viewport),
                    "gpu_stale_paint: exit capture did not follow the native viewport")

            after = app.wait_for(visible_submission, "the resized window to submit")
            replacement = after["paint"]
            require(after["snapshot"]["target"] == opened["target"] and after["snapshot"]["viewport"] == viewport
                    and replacement["surface_generation"] > candidate["surface_generation"]
                    and replacement["device_generation"] == candidate["device_generation"],
                    f"gpu_stale_paint: the live window did not submit a replacement surface: {after}")

            rows = list(app.trace_rows(offset, pending_schema, input_schema, frame_schema))
            frames = [row for row in rows if row["schema"] == frame_schema and row["window"] == trace_window]
            attempts = [row for row in frames if row["revision"] == candidate["revision"]]
            discarded = next((row for row in attempts if row["skip_reason"] == "stale_render" and row["render_worker"]), None)
            require(discarded and not any(row["submitted"] for row in attempts)
                    and discarded["surface_generation"] == candidate["surface_generation"]
                    and discarded["device_generation"] == candidate["device_generation"]
                    and discarded["timeline_us"]["render_submitted"] <= parked["at_us"]
                    and discarded["timeline_us"]["present_called"] is None,
                    f"gpu_stale_paint: expected the GPU drawing to be discarded before presenting: {attempts}")
            dismissals = [row for row in rows if row["schema"] == input_schema and row["window"] == window and row["route"] == "framework"]
            require(len(dismissals) == 1 and parked["at_us"] < dismissals[0]["started_us"]
                    and rows.index(dismissals[0]) < rows.index(discarded),
                    "gpu_stale_paint: Glass did not close while the edited drawing was parked")
            require(any(row["paint"] == replacement and row["submitted"]
                        and row["size_px"] == [round(side * row["scale"]) for side in viewport] for row in frames),
                    "gpu_stale_paint: the replacement submission trace did not match the resized window")
            print(f"gpu_stale_paint: discarded revision {candidate['revision']}, exit kept revision {accepted['revision']}, "
                  f"surface {accepted['surface_generation']} -> {replacement['surface_generation']}", flush=True)


def gpu_recovery(args, output, sockets):
    with LiveWindow("gpu_recovery", args, output, sockets, ["gpu_lab"]) as app:
        def observe():
            reply = app.requests({"op": "inspect"})[0]
            host = reply["live"]["window"]
            return reply["snapshot"], host["device_generation"], host["surface_generation"]

        viewport = resize(app, 800, 620)
        app.requests({"op": "tap", "target": find(viewport, "gpu-glass-open")["target"]})
        glass = find(app.inspect(), "gpu-glass-refraction")["target"]
        app.requests({"op": "set_value", "target": glass, "value": 0.35})
        before, generation, surface = observe()
        # The debug host destroys its own device after delivering this first painted capture.
        first = app.capture("device-before.png")
        deadline = time.monotonic() + args.timeout
        while True:
            after, replacement, rebound = observe()
            if replacement > generation:
                break
            require(time.monotonic() < deadline, "gpu_recovery: replacement device did not arrive")
            threading.Event().wait(0.02)

        require(replacement == generation + 1, "gpu_recovery: expected one recovery for the injected loss")
        require(rebound > surface, "gpu_recovery: surface generation did not advance")
        require(after["target"] == before["target"] and after["focus"] == before["focus"], "gpu_recovery: interaction ownership changed")
        require(find(after, "gpu-glass-refraction")["target"] == glass, "gpu_recovery: the dialog was remounted")
        require(find(after, "gpu-glass-refraction-value")["name"] == "35%", "gpu_recovery: refraction state was reset")
        require(app.capture("device-after.png") == first, "gpu_recovery: pixels changed after replacement")
        app.requests({"op": "set_value", "target": glass, "value": 0.6}, {"op": "expect", "clean": True})
        require(find(app.inspect(), "gpu-glass-refraction-value")["name"] == "60%", "gpu_recovery: input failed after recovery")
        print(f"gpu_recovery: device {generation} -> {replacement}, surface {surface} -> {rebound}, identical pixels and live dialog target",
              flush=True)


def surface_drain(args, output, sockets):
    drain_schema = "octara.surface_drain/1"
    input_schema = "octara.inspect_delivery/1"
    env = {"WGPU_BACKEND": "metal", "OCTARA_TRACE_INPUT": "1", "OCTARA_SURFACE_DRAIN_DELAY_MS": "750"}
    with LiveWindow("surface_drain", args, output, sockets, ["windows"], env=env) as app, app.wire() as stream:
        def send(op, **fields):
            reply = app.wire_request(stream, {"op": op, **fields})
            require(reply.get("ok"), f"surface_drain: {op} failed: {reply}")
            if op != "inspect":
                require(reply.get("delivery") == "delivered", f"surface_drain: {op} was not delivered: {reply}")
            return reply

        def marker(offset, phase, start=None):
            def read():
                return next((row for row in app.trace_rows(offset, drain_schema) if row["phase"] == phase
                             and (start is None or (row["device_generation"] == start["device_generation"]
                                                    and row["at_us"] > start["at_us"]))), None)

            return app.wait_for(read, f"worker drain {phase} (use a debug window build)")

        def submitted(window):
            def read():
                reply = send("inspect", window=window)
                completion = reply["completion"]
                surface = completion["surface"]
                if completion["layout"] == "settled" and surface["state"] == "submitted":
                    paint = reply["paint"]
                    require(paint["origin"] == "submitted" and paint["window"] == window and paint["frame"] == surface["frame"],
                            f"surface_drain: window {window} has mismatched submission evidence")
                    return reply
                return None

            return app.wait_for(read, f"window {window} to submit its settled drawing")

        def open_note(number):
            send("tap", window=main, id="new")
            windows = send("inspect", window=main)["snapshot"]["windows"]
            notes = [window for window in windows if window["title"] == f"Note {number}"]
            require(len(notes) == 1, f"surface_drain: Note {number} did not open")
            return notes[0]["window"]

        def require_overlap(offset, start, inputs):
            finish = marker(offset, "finish", start)
            require(finish.get("ready") is True, f"surface_drain: worker returned without draining: {finish}")
            during = [(row["window"], row["route"]) for row in app.trace_rows(offset, input_schema)
                      if start["at_us"] < row["started_us"] <= row["at_us"] < finish["at_us"]]
            require(during == inputs, f"surface_drain: expected input {inputs} inside the drain, saw {during}")

        main = send("inspect")["snapshot"]["window"]
        main_before = submitted(main)["snapshot"]
        offset = Path(app.log.name).stat().st_size
        note = open_note(1)
        start = marker(offset, "start")
        field = find(send("inspect", window=note)["snapshot"], "body")["target"]
        focused = send("focus", window=main, id="open")
        edited = send("type", window=note, id="body", target=field, text="Edited during configuration")
        require(focused["completion"]["layout"] == edited["completion"]["layout"] == "settled"
                and edited["completion"]["surface"]["state"] == "pending",
                "surface_drain: input did not settle while the note was waiting for its first surface")
        require_overlap(offset, start, [(main, "semantic"), (note, "framework")])

        first = submitted(note)
        snapshot = first["snapshot"]
        body = find(snapshot, "body")
        require(snapshot["viewport"] == [360.0, 240.0] and body["target"] == field and body["value"] == "Edited during configuration"
                and first["paint"]["device_generation"] == start["device_generation"],
                "surface_drain: first note submission lost its viewport, target, edit or device")
        require(send("inspect", window=main)["snapshot"]["viewport"] == main_before["viewport"],
                "surface_drain: opening the note changed the main viewport")
        send("tap", window=note, id="close")

        offset = Path(app.log.name).stat().st_size
        closing = open_note(2)
        start = marker(offset, "start")
        closed = send("tap", window=closing, id="close")
        require(closed["completion"]["layout"] == "unavailable", "surface_drain: closing the waiting window did not retire it")
        replacement = open_note(3)
        require_overlap(offset, start, [(closing, "framework"), (main, "framework")])
        result = submitted(replacement)
        require(replacement != closing and result["snapshot"]["viewport"] == [360.0, 240.0]
                and result["paint"]["device_generation"] == start["device_generation"],
                "surface_drain: the replacement did not submit its own drawing")
        retired = app.wire_request(stream, {"op": "inspect", "window": closing})
        require(not retired.get("ok") and retired.get("code") == "BAD_REQUEST", "surface_drain: a closed window remained routable")
        require(not any(row.get("submitted") and (row.get("paint") or {}).get("window") == closing
                        for row in app.trace_rows(offset, "octara.frame_trace/1")),
                "surface_drain: the closed window submitted a drawing after its drain")
        send("tap", window=replacement, id="close")
        print("surface_drain: both windows settled input during the worker wait and a closed note did not submit", flush=True)


def main():
    repo = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--window", type=Path, default=repo / "target/debug/window", help="built window example binary")
    parser.add_argument("--octara", type=Path, default=repo / "target/debug/octara", help="built CLI binary")
    parser.add_argument("--output", type=Path, help="empty directory for screenshots and logs; default: a new /tmp directory")
    parser.add_argument("--timeout", type=float, default=30, help="startup/command timeout in seconds, at most 30 (default: 30)")
    probe = parser.add_mutually_exclusive_group()
    probe.add_argument("--gpu-recovery", action="store_true", help="verify one injected device loss with a debug window build")
    probe.add_argument("--gpu-paint-evidence", action="store_true", help="verify Shader Lab paint through a fallback, exit, and reopen")
    probe.add_argument("--gpu-stale-paint", action="store_true", help="verify a resized macOS window discards an in-flight Glass drawing")
    probe.add_argument("--surface-drain", action="store_true", help="verify input during shared surface configuration with a macOS debug window build")
    args = parser.parse_args()
    if os.name != "posix" or not hasattr(socket, "AF_UNIX"):
        print("SKIP: this live smoke check uses Unix-domain sockets")
        return 0
    if args.surface_drain and sys.platform != "darwin":
        print("SKIP: the surface drain check requires macOS")
        return 0
    if args.gpu_stale_paint and sys.platform != "darwin":
        print("SKIP: the stale GPU paint check requires macOS")
        return 0
    require(0 < args.timeout <= 30, "--timeout must be greater than zero and at most 30 seconds")
    args.repo = repo
    for name in ("window", "octara"):
        path = getattr(args, name).resolve()
        require(path.is_file() and os.access(path, os.X_OK), f"build the executable first: {path}")
        setattr(args, name, path)
    output = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="octara-live-", dir="/tmp"))
    if args.output:
        output.mkdir(parents=True, exist_ok=True)
        require(not any(output.iterdir()), f"--output must be empty: {output}")
    print(f"Live smoke artifacts: {output}", flush=True)
    scenarios = (counter, gestures, search, todo, windows, styles, activation, handlers, bindings, content, gpu_lab, gpu_glass)
    if args.gpu_recovery:
        os.environ["OCTARA_DESTROY_DEVICE_AFTER_CAPTURE"] = "1"
        scenarios = (gpu_recovery,)
    if args.gpu_paint_evidence:
        scenarios = (gpu_paint_evidence,)
    if args.gpu_stale_paint:
        scenarios = (gpu_stale_paint,)
    if args.surface_drain:
        scenarios = (surface_drain,)
    with tempfile.TemporaryDirectory(prefix="octara-agent-", dir="/tmp") as temporary:
        sockets = Path(temporary)
        for scenario in scenarios:
            scenario(args, output, sockets)
            print(f"PASS {scenario.__name__}", flush=True)
    if args.gpu_recovery:
        print("Live Inspector GPU recovery passed.")
    elif args.gpu_paint_evidence:
        print("Live Inspector author paint evidence passed.")
    elif args.gpu_stale_paint:
        print("Live Inspector stale GPU paint passed.")
    elif args.surface_drain:
        print("Live Inspector surface drain passed.")
    else:
        print("Live Inspector: inspect, gestures, native resize, realtime settle, GPU capture, errors, window routing and reconnect passed.")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (SmokeFailure, OSError, ValueError, KeyError, struct.error, subprocess.TimeoutExpired, zlib.error) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)

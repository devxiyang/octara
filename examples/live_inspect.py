#!/usr/bin/env python3
"""Check the live Inspector against actual desktop windows, using only Python's stdlib.

Build the window example and CLI first, then run from any directory:
    python3 examples/live_inspect.py --window target/debug/window --octara target/debug/octara

Requires an interactive desktop and Unix-domain sockets. Windows is skipped. Screenshots,
window logs and CLI transcripts stay in the printed output directory (or --output DIRECTORY).
This checks framework input delivered to a real window, not OS keyboard/gesture injection.
"""

import argparse
import contextlib
import hashlib
import json
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


def png_pixels(path):
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
    return width, height, hashlib.sha256(pixels).hexdigest()


class LiveWindow:
    def __init__(self, fixture, args, output, sockets, window_args=None):
        self.fixture = fixture
        self.args = args
        self.output = output
        self.endpoint = sockets / f"{fixture}.sock"
        self.process = None
        self.log = None
        self.transcript = None
        self.window_args = window_args

    def __enter__(self):
        self.log = (self.output / f"{self.fixture}-window.log").open("w", encoding="utf-8")
        self.transcript = (self.output / f"{self.fixture}-cli.jsonl").open("w", encoding="utf-8")
        env = os.environ.copy()
        for name in ("OCTARA_CAPTURE", "OCTARA_CONFORMANCE", "OCTARA_EXIT_AFTER_FRAMES", "OCTARA_PICK"):
            env.pop(name, None)
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
        require(snapshot.get("schema") == "octara.inspect/1", f"{self.fixture}: wrong live snapshot schema")
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
        app.capture("todo-after.png")
        # The appended row can be below the viewport; exercise the first reachable row instead.
        app.requests({"op": "tap", "id": "remove-1"}, {"op": "expect", "id": "todo-1", "exists": False},
                     {"op": "expect", "id": "left", "text": "30 left"})


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


def main():
    repo = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--window", type=Path, default=repo / "target/debug/window", help="built window example binary")
    parser.add_argument("--octara", type=Path, default=repo / "target/debug/octara", help="built CLI binary")
    parser.add_argument("--output", type=Path, help="empty directory for screenshots and logs; default: a new /tmp directory")
    parser.add_argument("--timeout", type=float, default=30, help="startup/command timeout in seconds, at most 30 (default: 30)")
    args = parser.parse_args()
    if os.name != "posix" or not hasattr(socket, "AF_UNIX"):
        print("SKIP: this live smoke check uses Unix-domain sockets")
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
    with tempfile.TemporaryDirectory(prefix="octara-agent-", dir="/tmp") as temporary:
        sockets = Path(temporary)
        for scenario in (counter, gestures, search, todo, windows, styles, activation, handlers, bindings, content):
            scenario(args, output, sockets)
            print(f"PASS {scenario.__name__}", flush=True)
    print("Live Inspector: inspect, gestures, native resize, realtime settle, GPU capture, errors, window routing and reconnect passed.")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (SmokeFailure, OSError, ValueError, KeyError, struct.error, zlib.error) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)

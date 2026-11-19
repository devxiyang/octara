#!/usr/bin/env python3
"""Validate retained Inspector targets in an already running targets fixture.

Only the built CLI's `run --connect` route is used. This script neither launches nor stops
the fixture, sends OS input, nor retries an input after a failure or lost reply.
"""

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import struct
import subprocess
import sys


class Failure(RuntimeError):
    pass


def require(condition, message):
    if not condition:
        raise Failure(message)


def find(snapshot, identifier):
    pending = [snapshot["root"]]
    found = []
    while pending:
        node = pending.pop()
        if node.get("id") == identifier:
            found.append(node)
        pending.extend(node.get("children", []))
    require(len(found) == 1, f"expected one {identifier!r} node, found {len(found)}")
    return found[0]


def token(value):
    target = value.get("target")
    require(isinstance(target, str) and target, "snapshot is missing a nonempty opaque target")
    return target


class Scenario:
    def __init__(self, args, output):
        self.args = args
        self.output = output
        self.step = 0
        self.main = None
        self.hits = 0
        self.generation = 0
        self.updates = 0
        self.openings = 0

    def request(self, name, request, route="query", delivery=None, stale=False):
        self.step += 1
        prefix = f"{self.step:03d}-{name}"
        entry = {"step": self.step, "name": name, "request": request}
        command = [str(self.args.octara), "run", "--connect", str(self.args.socket), "--json"]
        try:
            result = subprocess.run(command, input=json.dumps(request) + "\n", text=True,
                                    capture_output=True, cwd=self.output, timeout=30)
            entry.update(exit=result.returncode, stdout=result.stdout, stderr=result.stderr)
            require(len(result.stdout.encode()) <= 16 * 1024 * 1024, "CLI reply exceeded 16 MiB")
            lines = result.stdout.splitlines()
            require(len(lines) == 1, f"CLI returned {len(lines)} JSON lines for one request")
            reply = json.loads(lines[0])
            entry["reply"] = reply
        except (subprocess.TimeoutExpired, OSError, ValueError, Failure) as error:
            entry["error"] = str(error)
            self.record(prefix, entry)
            raise Failure(f"{prefix}: no usable reply; input was not retried: {error}") from error
        self.record(prefix, entry)
        require(result.returncode == 0, f"{prefix}: CLI exited {result.returncode}: {result.stderr}")
        require(reply.get("ok") is (not stale), f"{prefix}: unexpected reply: {reply}")
        require(reply.get("live", {}).get("route") == route, f"{prefix}: expected live.route={route!r}: {reply}")
        if stale:
            require(reply.get("code") == "STALE_TARGET", f"{prefix}: wrong refusal code: {reply}")
            delivery = "refused"
        if delivery:
            require(reply.get("delivery") == delivery, f"{prefix}: wrong Reply.delivery: {reply}")
            require(reply.get("live", {}).get("delivery") == delivery, f"{prefix}: wrong live.delivery: {reply}")
        print(f"PASS {prefix}", flush=True)
        return reply

    def record(self, prefix, entry):
        (self.output / f"{prefix}.json").write_text(json.dumps(entry, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        with (self.output / "transcript.jsonl").open("a", encoding="utf-8") as stream:
            stream.write(json.dumps(entry, ensure_ascii=False) + "\n")

    def inspect(self, name, window=None):
        request = {"op": "inspect"}
        if window is not None:
            request["window"] = window
        snapshot = self.request(name, request)["snapshot"]
        require(snapshot.get("schema") == "octara.inspect/2", "unexpected Inspector schema")
        token(snapshot)
        for info in snapshot["windows"]:
            token(info)
        if window is not None:
            require(snapshot["window"] == window, f"inspect routed to the wrong window: {snapshot['window']}")
        self.state(snapshot)
        return snapshot

    def state(self, snapshot):
        expected = {"targets-hits": f"Total hits: {self.hits}",
                    "targets-generation": f"Generation: {self.generation}",
                    "targets-updates": f"Value updates: {self.updates}"}
        if snapshot["window"] == self.main:
            expected["targets-child-openings"] = f"Child openings: {self.openings}"
            require(find(snapshot, "targets-error").get("name", "") == "", "fixture reported an open-window error")
        for identifier, wanted in expected.items():
            actual = find(snapshot, identifier).get("name")
            require(actual == wanted, f"{identifier}: expected {wanted!r}, got {actual!r}")

    def tap(self, name, snapshot, identifier="target-action", target=None):
        self.request(name, {"op": "tap", "window": snapshot["window"], "id": identifier,
                            "target": target if target is not None else token(find(snapshot, identifier))},
                     route="framework", delivery="delivered")

    def control(self, name, identifier):
        before = self.inspect(f"{name}-before", self.main)
        self.tap(name, before, identifier)

    def hit(self, name, snapshot, target=None):
        self.tap(name, snapshot, target=target)
        self.hits += 1
        return self.inspect(f"{name}-after", snapshot["window"])

    def refuse(self, name, request, route):
        self.request(name, request, route=route, stale=True)
        return self.inspect(f"{name}-unchanged", self.main)

    def capture(self, name, snapshot):
        path = self.output / f"{name}.png"
        reply = self.request(name, {"op": "capture", "window": snapshot["window"], "target": token(snapshot),
                                    "path": str(path), "pixels": "painted", "settle_ms": 5000}, route="capture")
        report = reply["capture"]
        require(report.get("renderer") == "gpu" and report.get("pixels") == "painted", f"not a GPU capture: {report}")
        require(report.get("settled") is True and not report.get("pending"), f"capture did not settle: {report}")
        with path.open("rb") as stream:
            header = stream.read(24)
        require(header[:8] == b"\x89PNG\r\n\x1a\n" and len(header) == 24, "capture is not a PNG")
        width, height = struct.unpack(">II", header[16:24])
        require((width, height) == (report["width"], report["height"]) and width > 0 and height > 0,
                "capture dimensions disagree with its report")
        issues = self.request(f"{name}-issues", {"op": "issues", "window": snapshot["window"], "target": token(snapshot)})
        require(issues.get("issues") == [], f"capture has layout issues or diagnostics: {issues}")


def child_info(snapshot):
    found = [info for info in snapshot["windows"] if info["title"] == "Octara Target Child"]
    require(len(found) == 1, f"expected one child window, found {len(found)}")
    return found[0]


def run(app):
    first = app.inspect("initial")
    app.main = first["window"]
    app.state(first)
    require(len(first["windows"]) == 1 and first["windows"][0]["title"] == "Octara Target Validation",
            "start with a fresh targets fixture and no child window")
    a = token(find(first, "target-action"))
    app.capture("initial-gpu", first)
    first_hit = app.hit("fresh-a", first, a)
    old_node = find(first_hit, "target-action")

    app.control("update", "targets-update")
    app.updates += 1
    moved = app.inspect("updated", app.main)
    new_node = find(moved, "target-action")
    require(token(new_node) == a, "value/geometry update changed the node target")
    require(new_node.get("name") != old_node.get("name") and new_node["bounds"][0] != old_node["bounds"][0],
            "update did not change both the target label and its position")
    app.hit("retained-a", moved, a)
    app.capture("updated-gpu", app.inspect("updated-picture-state", app.main))

    app.control("replace", "targets-replace")
    app.generation += 1
    replaced = app.inspect("replaced", app.main)
    b = token(find(replaced, "target-action"))
    require(b != a, "same-id replacement reused the old node target")
    for op, extra, route in (("tap", {}, "framework"), ("double_tap", {}, "framework"),
                             ("long_press", {}, "framework"), ("focus", {}, "semantic"),
                             ("hover", {"on": True}, "semantic"), ("type", {"text": "stale-input"}, "framework")):
        app.refuse(f"old-a-{op}", {"op": op, "window": app.main, "id": "target-action", "target": a, **extra}, route)
    current = app.hit("fresh-b", replaced, b)
    old_window = token(current)
    app.request("reload", {"op": "reload", "window": app.main, "target": old_window},
                route="lifecycle", delivery="delivered")
    reloaded = app.inspect("reloaded", app.main)
    c = token(find(reloaded, "target-action"))
    require(c != b and token(reloaded) != old_window, "reload did not retire the old node and window targets")
    app.refuse("old-b-after-reload", {"op": "tap", "window": app.main, "id": "target-action", "target": b}, "framework")
    app.refuse("old-window-after-reload", {"op": "size", "window": app.main, "width": 840, "height": 620,
                                           "target": old_window}, "native-window")
    current = app.hit("fresh-c", reloaded, c)
    app.capture("reloaded-gpu", current)

    app.control("open-child", "targets-open")
    app.openings += 1
    opened = app.inspect("child-open", app.main)
    info = child_info(opened)
    old_number, old_child_window = info["window"], token(info)
    require(find(opened, "targets-child")["name"] == f"Child: open (window {old_number})", "main did not report the open child")
    child = app.inspect("child-first", old_number)
    require(token(child) == old_child_window, "WindowInfo and Snapshot disagree on the child target")
    old_child_node = token(find(child, "target-action"))
    require(old_child_node != c, "child target was confused with the main target")
    child = app.hit("child-fresh", child)
    app.inspect("child-hit-shared", app.main)
    app.capture("child-gpu", child)

    app.control("close-child", "targets-close")
    closed = app.inspect("child-closed", app.main)
    require(not any(info["window"] == old_number for info in closed["windows"])
            and find(closed, "targets-child")["name"] == "Child: closed", "child was not closed")
    app.control("reopen-child", "targets-open")
    app.openings += 1
    reopened = app.inspect("child-reopened", app.main)
    info = child_info(reopened)
    new_number = info["window"]
    require(new_number != old_number and token(info) != old_child_window, "reopening reused the closed window identity")
    child = app.inspect("child-second", new_number)
    require(token(find(child, "target-action")) != old_child_node, "reopening reused the closed child node target")
    for label, number in (("closed-number", old_number), ("new-number", new_number)):
        app.refuse(f"{label}-old-node", {"op": "tap", "window": number, "id": "target-action", "target": old_child_node}, "framework")
        app.refuse(f"{label}-old-window", {"op": "size", "window": number, "width": 640, "height": 360,
                                          "target": old_child_window}, "native-window")
    child = app.hit("reopened-child-fresh", child)
    app.capture("reopened-child-gpu", child)
    app.request("legacy-id-only", {"op": "tap", "window": app.main, "id": "target-action"},
                route="framework", delivery="delivered")
    app.hits += 1
    final = app.inspect("final", app.main)
    app.inspect("final-shared-child", new_number)
    app.capture("final-gpu", final)
    return {"hits": app.hits, "generation": app.generation, "updates": app.updates,
            "child_openings": app.openings, "main_window": app.main, "child_window": new_number}


def main():
    repo = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--socket", required=True, type=Path, help="existing targets fixture Inspector socket")
    parser.add_argument("--octara", type=Path, default=repo / "target/debug/octara", help="built CLI executable")
    parser.add_argument("--output", required=True, type=Path, help="new evidence directory; must not already exist")
    args = parser.parse_args()
    args.socket, args.octara = args.socket.resolve(), args.octara.resolve()
    require(args.socket.is_socket(), f"not an existing Inspector socket: {args.socket}")
    require(args.octara.is_file() and os.access(args.octara, os.X_OK), f"not an executable CLI: {args.octara}")
    output = args.output.resolve()
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    metadata = {"started_at": datetime.now(timezone.utc).isoformat(), "socket": str(args.socket),
                "octara": str(args.octara), "route": "live Inspector framework/semantic input; no OS input"}
    (output / "run.json").write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
    app = Scenario(args, output)
    print(f"Target evidence: {output}", flush=True)
    try:
        result = run(app)
        summary = {"ok": True, "steps": app.step, **result}
    except (Failure, OSError, ValueError, KeyError, TypeError, struct.error) as error:
        summary = {"ok": False, "steps": app.step, "error": str(error), "input_retried": False}
    summary["finished_at"] = datetime.now(timezone.utc).isoformat()
    (output / "summary.json").write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({**summary, "output": str(output)}, ensure_ascii=False))
    return 0 if summary["ok"] else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (Failure, OSError, ValueError) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)

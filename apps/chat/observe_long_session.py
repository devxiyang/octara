#!/usr/bin/env python3
"""Record an existing Chat window through the built CLI, without OS input or app lifecycle actions.

Each request is sent once. roundtrip_ms measures CLI startup plus Inspector IPC, not frame time.
Scroll with computer use between named observations; this recorder never synthesizes a wheel.
"""

import argparse
from datetime import datetime, timezone
import json
import math
import os
from pathlib import Path
import re
import subprocess
import sys
import time


class Failure(RuntimeError):
    pass


def require(condition, message):
    if not condition:
        raise Failure(message)


def nodes(snapshot):
    pending = [snapshot["root"]]
    while pending:
        node = pending.pop()
        yield node
        pending.extend(reversed(node.get("children", [])))


def find(snapshot, identifier):
    found = [node for node in nodes(snapshot) if node.get("id") == identifier]
    require(len(found) == 1, f"expected one {identifier!r}, found {len(found)}")
    return found[0]


def target(value):
    token = value.get("target")
    require(isinstance(token, str) and token, "missing opaque target; rebuild the fixture and CLI")
    return token


def rect(value):
    require(isinstance(value, list) and len(value) == 4
            and all(isinstance(n, (int, float)) and not isinstance(n, bool) and math.isfinite(n) for n in value),
            f"invalid bounds: {value!r}")
    return value


def intersection(a, b):
    x, y = max(a[0], b[0]), max(a[1], b[1])
    return [x, y, max(0, min(a[0] + a[2], b[0] + b[2]) - x),
            max(0, min(a[1] + a[3], b[1] + b[3]) - y)]


def observe(snapshot):
    transcript = rect(find(snapshot, "transcript")["bounds"])
    visible = intersection(transcript, [0, 0, *snapshot["viewport"]])
    rows = []
    for node in nodes(snapshot):
        if re.fullmatch(r"msg-\d+", node.get("id", "")):
            bounds = rect(node["bounds"])
            clipped = intersection(bounds, visible)
            rows.append({"id": node["id"], "bounds": bounds, "relative_y": bounds[1] - transcript[1],
                         "visible": clipped[2] > 0 and clipped[3] > 0})
    rows.sort(key=lambda row: (row["bounds"][1], row["id"]))
    visible_rows = [row for row in rows if row["visible"]]
    jump = any(node.get("id") == "jump" for node in nodes(snapshot))
    return {"transcript": {"bounds": transcript, "visible_bounds": visible}, "mounted_rows": rows,
            "mounted_count": len(rows), "visible_rows": visible_rows,
            "anchor": {"id": visible_rows[0]["id"], "relative_y": visible_rows[0]["relative_y"]} if visible_rows else None,
            "jump_present": jump, "following_from_jump": not jump}


class Client:
    def __init__(self, args, output):
        self.args, self.output = args, output
        self.records = []
        self.snapshot = None
        self.window = None

    def request(self, request, route="query", delivered=False):
        record = {"request": request, "started_at": datetime.now(timezone.utc).isoformat()}
        started = time.perf_counter()
        try:
            command = [str(self.args.octara), "run", "--connect", str(self.args.socket), "--json"]
            result = subprocess.run(command, input=json.dumps(request, ensure_ascii=False) + "\n", text=True,
                                    capture_output=True, cwd=self.output, timeout=35)
            record.update(exit=result.returncode, stdout=result.stdout, stderr=result.stderr)
            require(len(result.stdout.encode()) <= 16 * 1024 * 1024, "CLI reply exceeded 16 MiB")
            lines = result.stdout.splitlines()
            require(len(lines) == 1, f"one request returned {len(lines)} reply lines")
            reply = json.loads(lines[0])
            record["reply"] = reply
            require(result.returncode == 0, f"CLI exited {result.returncode}: {result.stderr}")
            require(reply.get("ok") is True, f"{request['op']} failed: {reply.get('code')}: {reply.get('message')}")
            require(reply.get("live", {}).get("route") == route, f"unexpected route for {request['op']}: {reply.get('live')}")
            if delivered:
                require(reply.get("delivery") == "delivered" and reply.get("live", {}).get("delivery") == "delivered",
                        f"{request['op']} has no confirmed input delivery: {reply}")
            return reply
        except (OSError, ValueError, subprocess.TimeoutExpired, Failure) as error:
            record["error"] = str(error)
            raise Failure(f"{error}; request was not retried") from error
        finally:
            record["roundtrip_ms"] = (time.perf_counter() - started) * 1000
            self.records.append(record)

    def inspect(self):
        request = {"op": "inspect"}
        if self.window is not None:
            request["window"] = self.window
        snapshot = self.request(request)["snapshot"]
        require(snapshot.get("schema") == "octara.inspect/2", "unexpected snapshot schema")
        target(snapshot)
        self.snapshot, self.window = snapshot, snapshot["window"]
        return snapshot

    def input(self, operation, route, node=None):
        snapshot = self.inspect()
        guard = target(find(snapshot, node)) if node else target(snapshot)
        return self.request({**operation, "window": snapshot["window"], "target": guard}, route, delivered=True)


def load_anchor(value, output):
    path = Path(value)
    if not path.is_file():
        candidates = [output / f"{value}.meta.json", output / f"{value}.json"]
        path = next((candidate for candidate in candidates if candidate.is_file()), path)
    require(path.is_file(), f"anchor evidence not found: {value}")
    data = json.loads(path.read_text(encoding="utf-8"))
    if "root" in data:
        anchor = observe(data)["anchor"]
    elif "snapshot" in data and isinstance(data["snapshot"], dict):
        anchor = observe(data["snapshot"])["anchor"]
    else:
        anchor = data.get("observation", data).get("anchor")
    require(isinstance(anchor, dict) and isinstance(anchor.get("id"), str)
            and isinstance(anchor.get("relative_y"), (int, float)), f"no reading anchor in {path}")
    return path.resolve(), anchor


def size(value):
    match = re.fullmatch(r"([0-9]+(?:\.[0-9]+)?)x([0-9]+(?:\.[0-9]+)?)", value)
    if not match:
        raise argparse.ArgumentTypeError("size must be WIDTHxHEIGHT")
    width, height = map(float, match.groups())
    if not 0 < width <= 16384 or not 0 < height <= 16384:
        raise argparse.ArgumentTypeError("size must be positive and at most 16384 per dimension")
    return width, height


def main():
    repo = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--socket", required=True, type=Path)
    parser.add_argument("--octara", type=Path, default=repo / "target/release/octara")
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--name", required=True)
    parser.add_argument("--invoke", action="append", default=[], metavar="long.COMMAND")
    parser.add_argument("--size", type=size, metavar="WIDTHxHEIGHT")
    parser.add_argument("--composer-file", type=Path, help="replace the composer through focus, Mod+A and typing, each with a fresh composer-node guard")
    parser.add_argument("--anchor-from", help="prior evidence name in output, or a snapshot/meta JSON path")
    parser.add_argument("--anchor-mode", choices=["exact", "visible"], default="exact")
    parser.add_argument("--expect-tail", choices=["yes", "no"])
    parser.add_argument("--tail-id", help="latest message id; required with --expect-tail yes")
    parser.add_argument("--capture", choices=["painted", "display"])
    parser.add_argument("--settle-ms", type=int, default=0, help="capture wait budget; zero keeps pending streams observable")
    args = parser.parse_args()
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,79}", args.name):
        parser.error("name must be 1–80 filename characters starting with a letter or digit")
    if any(not re.fullmatch(r"long\.[A-Za-z0-9_.]+", command) for command in args.invoke):
        parser.error("--invoke accepts only long.* fixture commands")
    if args.tail_id and not re.fullmatch(r"msg-\d+", args.tail_id):
        parser.error("--tail-id must be msg-N")
    if args.expect_tail == "yes" and not args.tail_id:
        parser.error("--expect-tail yes requires --tail-id")
    if not 0 <= args.settle_ms <= 30000:
        parser.error("--settle-ms must be between 0 and 30000")
    args.socket, args.octara = args.socket.resolve(), args.octara.resolve()
    require(args.socket.is_socket(), f"not an existing Inspector socket: {args.socket}")
    require(args.octara.is_file() and os.access(args.octara, os.X_OK), f"not an executable CLI: {args.octara}")
    output = args.output.resolve()
    output.mkdir(mode=0o700, parents=True, exist_ok=True)
    paths = {"snapshot": output / f"{args.name}.json", "meta": output / f"{args.name}.meta.json",
             "png": output / f"{args.name}.png"}
    require(all(not path.exists() and not path.is_symlink() for path in paths.values()), "evidence name already exists; choose a new name")
    lock = output / f".{args.name}.lock"
    with lock.open("x", encoding="utf-8") as stream:
        stream.write("observation reserved\n")
    client = Client(args, output)
    meta = {"name": args.name, "started_at": datetime.now(timezone.utc).isoformat(), "socket": str(args.socket),
            "timing": "roundtrip_ms includes CLI startup and Inspector IPC; it is not frame time", "input_retried": False,
            "requests": client.records, "errors": []}
    try:
        baseline = load_anchor(args.anchor_from, output) if args.anchor_from else None
        client.inspect()
        for command in args.invoke:
            client.input({"op": "invoke", "command": command}, "semantic")
        if args.size:
            client.input({"op": "size", "width": args.size[0], "height": args.size[1]}, "native-window")
        if args.composer_file:
            require(args.composer_file.stat().st_size <= 1024 * 1024, "composer file exceeds 1 MiB")
            text = args.composer_file.read_text(encoding="utf-8")
            client.input({"op": "focus", "id": "composer"}, "semantic", node="composer")
            client.input({"op": "key", "chord": "mod+a"}, "framework", node="composer")
            client.input({"op": "type", "id": "composer", "text": text}, "framework", node="composer")
        if args.capture:
            snapshot = client.inspect()
            reply = client.request({"op": "capture", "window": snapshot["window"], "target": target(snapshot),
                                    "path": str(paths["png"]), "pixels": args.capture, "settle_ms": args.settle_ms}, "capture")
            meta["capture"] = reply.get("capture")
            require(isinstance(meta["capture"], dict) and paths["png"].is_file(), "capture returned no report or PNG")
        snapshot = client.inspect()
        observed = observe(snapshot)
        meta["observation"] = observed
        if args.composer_file:
            require(find(snapshot, "composer").get("value") == text, "composer does not match the requested file contents")
        if baseline:
            path, previous = baseline
            current = next((row for row in observed["mounted_rows"] if row["id"] == previous["id"]), None)
            delta = current["relative_y"] - previous["relative_y"] if current else None
            comparison = {"source": str(path), "mode": args.anchor_mode, "id": previous["id"],
                          "baseline_relative_y": previous["relative_y"], "current_relative_y": current["relative_y"] if current else None,
                          "delta": delta, "mounted": current is not None, "visible": bool(current and current["visible"])}
            meta["anchor_comparison"] = comparison
            require(comparison["visible"], f"baseline anchor is no longer visible: {comparison}")
            if args.anchor_mode == "exact":
                require(abs(delta) <= 1, f"reading anchor moved by more than 1 px: {comparison}")
        if args.tail_id:
            row = next((row for row in observed["visible_rows"] if row["id"] == args.tail_id), None)
            bounds = observed["transcript"]["visible_bounds"]
            distance = row["bounds"][1] + row["bounds"][3] - bounds[1] - bounds[3] if row else None
            meta["tail"] = {"id": args.tail_id, "visible": row is not None, "bottom_delta": distance}
        if args.expect_tail == "yes":
            require(not observed["jump_present"] and meta["tail"]["visible"] and abs(meta["tail"]["bottom_delta"]) <= 3,
                    f"latest message is not followed at the bottom: {meta.get('tail')}, jump={observed['jump_present']}")
        elif args.expect_tail == "no":
            require(observed["jump_present"], "expected the product Jump control after leaving the tail")
        meta["ok"] = True
    except (Failure, OSError, ValueError, KeyError, TypeError) as error:
        meta["ok"] = False
        meta["errors"].append(str(error))
    finally:
        meta["finished_at"] = datetime.now(timezone.utc).isoformat()
        meta["snapshot_stage"] = "last successful inspect; see request order and timestamps"
        with paths["snapshot"].open("x", encoding="utf-8") as stream:
            json.dump(client.snapshot, stream, ensure_ascii=False, indent=2)
            stream.write("\n")
        with paths["meta"].open("x", encoding="utf-8") as stream:
            json.dump(meta, stream, ensure_ascii=False, indent=2)
            stream.write("\n")
        lock.unlink()
    print(json.dumps({"ok": meta["ok"], "snapshot": str(paths["snapshot"]), "meta": str(paths["meta"]),
                      "observation": meta.get("observation"), "anchor_comparison": meta.get("anchor_comparison"),
                      "tail": meta.get("tail"), "capture": meta.get("capture"), "errors": meta["errors"]}, ensure_ascii=False))
    return 0 if meta["ok"] else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (Failure, OSError, ValueError) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)

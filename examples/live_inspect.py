#!/usr/bin/env python3
"""Verify counter delivery and native presentation through octara.inspect/2."""

import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time

PROTOCOL = "octara.inspect/2"


def request(endpoint, command):
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(2)
        connection.connect(str(endpoint))
        connection.sendall((json.dumps({"protocol": PROTOCOL, **command}) + "\n").encode())
        with connection.makefile(encoding="utf-8") as reader:
            reply = json.loads(reader.readline())
    if reply.get("protocol") != PROTOCOL:
        raise RuntimeError(f"Unexpected Inspector response {reply}")
    return reply


def node(reply, test_id):
    return next(item for item in reply["snapshot"]["nodes"] if item.get("test_id") == test_id)


def wait_for(endpoint, process, predicate):
    end = time.monotonic() + 12
    while time.monotonic() < end and process.poll() is None:
        try:
            reply = request(endpoint, {"op": "observe"})
            if reply["ok"] and predicate(reply):
                return reply
        except OSError:
            pass
        time.sleep(0.05)
    raise RuntimeError("No matching settled native observation before the deadline")


def smoke(executable):
    with tempfile.TemporaryDirectory(prefix="octara-live-", dir="/tmp") as directory:
        endpoint = Path(directory) / "inspect.sock"
        env = {**os.environ, "OCTARA_INSPECT": str(endpoint)}
        capture = Path(directory) / "counter.png"
        env.update({"OCTARA_CAPTURE": str(capture), "OCTARA_CAPTURE_AFTER_MS": "600"})
        with subprocess.Popen([executable, "counter"], env=env) as process:
            try:
                before = wait_for(endpoint, process, lambda reply: reply["progress"]["presented_frame"] is not None)
                if node(before, "count").get("text") != "Count: 0":
                    raise RuntimeError("Unexpected initial counter value")
                target = node(before, "inc")["target"]
                received = request(endpoint, {
                    "op": "act", "window": before["window"], "target": target,
                    "action": {"kind": "accessibility", "action": "press"},
                })
                if not received["ok"] or not received["handled"]:
                    raise RuntimeError(f"Counter action was not delivered: {received}")
                previous = before["progress"]["presented_frame"]
                after = wait_for(endpoint, process, lambda reply:
                    node(reply, "count").get("text") == "Count: 1"
                    and reply["progress"]["presented_frame"] > previous and capture.is_file())
                if node(after, "inc")["target"] != target:
                    raise RuntimeError("Counter button changed target during the update")
                if not capture.read_bytes().startswith(b"\x89PNG\r\n\x1a\n"):
                    raise RuntimeError("Native capture is missing a PNG header")
                print(f"Count: 0 → Count: 1. Frames {previous} → {after['progress']['presented_frame']}. Native PNG captured.")
            finally:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--window", type=Path, required=True, help="Path to the built window binary")
    args = parser.parse_args()
    smoke(args.window.resolve())

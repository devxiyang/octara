#!/usr/bin/env python3
"""Save one read-only live Inspector snapshot and check partial JSON expectations.

This sends only {"op":"inspect"}. It never focuses a window, sends input, changes an input
source or grants a permission. Use computer use for the OS actions between observations.
"""

import argparse
import ctypes
from datetime import datetime, timezone
import json
from pathlib import Path
import re
import socket
import sys


MAX_REPLY = 16 * 1024 * 1024
COUNTERS = {"ime-single-edits", "ime-multi-edits", "ime-password-edits", "ime-submits"}


def current_input_source():
    """Read the active macOS input mode, not its underlying keyboard layout."""
    if sys.platform != "darwin":
        return None
    c = ctypes
    cf = c.CDLL("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation")
    tis = c.CDLL("/System/Library/Frameworks/Carbon.framework/Carbon")
    tis.TISCopyCurrentKeyboardInputSource.restype = c.c_void_p
    tis.TISGetInputSourceProperty.argtypes = [c.c_void_p, c.c_void_p]
    tis.TISGetInputSourceProperty.restype = c.c_void_p
    cf.CFStringGetCString.argtypes = [c.c_void_p, c.c_char_p, c.c_long, c.c_uint32]
    cf.CFStringGetCString.restype = c.c_bool
    cf.CFRelease.argtypes = [c.c_void_p]
    source = tis.TISCopyCurrentKeyboardInputSource()
    try:
        key = c.c_void_p.in_dll(tis, "kTISPropertyInputSourceID").value
        value = tis.TISGetInputSourceProperty(source, key) if source else None
        buffer = c.create_string_buffer(1024)
        if not value or not cf.CFStringGetCString(value, buffer, len(buffer), 0x08000100):
            raise RuntimeError("the system did not return the current keyboard input source")
        return buffer.value.decode("utf-8")
    finally:
        if source:
            cf.CFRelease(source)


def inspect(path):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(10)
        connection.connect(str(path))
        connection.sendall(b'{"op":"inspect"}\n')
        with connection.makefile("rb") as stream:
            line = stream.readline(MAX_REPLY + 2)
    if not line.endswith(b"\n") or len(line) > MAX_REPLY + 1:
        raise RuntimeError("Inspector returned an incomplete or oversized reply")
    reply = json.loads(line)
    if reply.get("ok") is not True:
        raise RuntimeError(f"Inspector refused observation: {reply.get('code')}: {reply.get('message')}")
    snapshot = reply.get("snapshot")
    if not isinstance(snapshot, dict) or snapshot.get("schema") != "octara.inspect/1":
        raise RuntimeError("Inspector returned no octara.inspect/1 snapshot")
    return snapshot, reply.get("live")


def compare(expected, actual, path="snapshot"):
    if isinstance(expected, dict):
        if not isinstance(actual, dict):
            return [f"{path}: expected an object"]
        errors = []
        for key, value in expected.items():
            if key not in actual:
                errors.append(f"{path}.{key}: absent")
            else:
                errors.extend(compare(value, actual[key], f"{path}.{key}"))
        return errors
    if isinstance(expected, list):
        if not isinstance(actual, list) or len(expected) != len(actual):
            return [f"{path}: expected array {expected!r}, got {actual!r}"]
        return [error for index, (wanted, found) in enumerate(zip(expected, actual))
                for error in compare(wanted, found, f"{path}[{index}]")]
    if expected is None:
        matches = actual is None
    elif isinstance(expected, bool) or isinstance(actual, bool):
        matches = type(actual) is type(expected) and actual == expected
    else:
        matches = actual == expected
    return [] if matches else [f"{path}: expected {expected!r}, got {actual!r}"]


def absent(snapshot, path):
    value = snapshot
    for key in path.split("."):
        if not isinstance(value, dict) or key not in value:
            return True
        value = value[key]
    return False


def counters(snapshot):
    found = {}
    pending = [snapshot["root"]]
    while pending:
        node = pending.pop()
        if node.get("id") in COUNTERS:
            found[node["id"]] = node.get("name")
        pending.extend(node.get("children", []))
    return found


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--socket", type=Path, required=True, help="live Inspector Unix socket")
    parser.add_argument("--output", type=Path, required=True, help="directory for named snapshot evidence")
    parser.add_argument("--name", required=True, help="unique evidence name (letters, digits, dots, underscores or hyphens)")
    parser.add_argument("--expect", type=json.loads, default={}, metavar="JSON", help="recursive partial object match; arrays/scalars match exactly")
    parser.add_argument("--absent", action="append", default=[], metavar="PATH", help="require a missing dot-separated key; may be repeated")
    parser.add_argument("--input-source", help="require this macOS input source both before and after the observation")
    args = parser.parse_args()
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,79}", args.name):
        parser.error("name must be 1–80 characters and start with a letter or digit")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    snapshot_path = output / f"{args.name}.json"
    metadata_path = output / f"{args.name}.meta.json"
    if snapshot_path.exists() or metadata_path.exists():
        parser.error("evidence name already exists; choose a new name")

    before = current_input_source()
    snapshot, live = inspect(args.socket)
    after = current_input_source()
    errors = compare(args.expect, snapshot)
    errors.extend(f"snapshot.{path}: expected absence" for path in args.absent if not absent(snapshot, path))
    if args.input_source and (before != args.input_source or after != args.input_source):
        errors.append(f"input source: expected {args.input_source!r}, observed before={before!r}, after={after!r}")
    metadata = {"observed_at": datetime.now(timezone.utc).isoformat(), "socket": str(args.socket),
                "input_source_before": before, "input_source_after": after, "input_source_stable": before == after,
                "live": live, "errors": errors}
    snapshot_path.write_text(json.dumps(snapshot, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    metadata_path.write_text(json.dumps(metadata, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"ok": not errors, "snapshot": str(snapshot_path), "input_source": after,
                      "input_source_stable": before == after, "focus": snapshot.get("focus"),
                      "text_input_present": "text_input" in snapshot, "text_input": snapshot.get("text_input"),
                      "counters": counters(snapshot), "errors": errors}, ensure_ascii=False, separators=(",", ":")))
    return 1 if errors else 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, RuntimeError, KeyError) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)

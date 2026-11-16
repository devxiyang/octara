#!/usr/bin/env python3
"""Verify failure feedback against GPU-backed native windows.

Build octara-examples --bin window and octara-cli first. Uses the same live CLI
harness as live_inspect.py; inputs are framework events, not OS input.
An optional --native-helper executable receives activate/close and the test PID
(see live_window.swift). Without it, OS-close validation is explicitly unverified.
Visible, presented frames are required; a locked desktop is not a passing run.
"""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile
import time

from live_inspect import LiveWindow, find, require


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--window", type=Path, default=Path("target/debug/window"))
    parser.add_argument("--octara", type=Path, default=Path("target/debug/octara"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--native-helper", type=Path)
    parser.add_argument("--timeout", type=float, default=30)
    args = parser.parse_args()
    args.repo = Path(__file__).resolve().parents[1]
    args.window = args.window.resolve()
    args.octara = args.octara.resolve()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    results = []
    with tempfile.TemporaryDirectory(prefix="octara-failure-") as directory:
        for case in ("fail_event", "fail_task", "fail_binding"):
            with LiveWindow(case, args, args.output, Path(directory), ["failure"]) as live:
                if args.native_helper:
                    subprocess.run([str(args.native_helper.resolve()), "activate", str(live.process.pid)],
                                   check=True, timeout=args.timeout)
                deadline = time.monotonic() + args.timeout
                while True:
                    observation = live.requests({"op": "inspect"})[0]
                    host = observation["live"]["window"]
                    if host["frames"] > 0 and host.get("occluded") is not True:
                        break
                    require(time.monotonic() < deadline, "no visible presented frame; unlock/activate the desktop and rerun")
                    time.sleep(0.1)
                initial = live.inspect()
                require(not initial["failed"], "fresh App already failed")
                missing = json.loads(live.cli("run", payload='{"op":"tap","id":"missing"}\n'))
                require(missing["code"] == "NO_SUCH_ELEMENT", "ordinary refusal changed")
                live.requests({"op": "tap", "id": "accept"})
                require(find(live.inspect(), "status")["name"] == "Accepted: 1", "ordinary refusal stopped the App")
                live.capture(case + "-before.png")
                before = live.requests({"op": "inspect"})[0]
                target = find(before["snapshot"], case)["target"]
                failure = json.loads(live.cli("run", payload=json.dumps({"op": "tap", "id": case, "target": target}) + "\n"))
                require(failure["code"] == "APP_FAILED", f"wrong panic outcome: {failure}")
                require(failure["live"]["delivery"] == "unknown", "panic fabricated delivery evidence")
                after = live.requests({"op": "inspect"})[0]
                require(after["snapshot"]["failed"], "snapshot did not expose failure")
                require(after["live"]["operations"] == ["inspect", "issues"], "failed host advertised executable operations")
                frames = after["live"]["window"]["frames"]
                require(frames == before["live"]["window"]["frames"], "panic published another frame")
                for request in ({"op": "tap", "id": "accept"}, {"op": "reload"}, {"op": "settle"},
                                {"op": "size", "width": 700, "height": 500},
                                {"op": "capture", "path": str(args.output / (case + "-forbidden.png"))}):
                    reply = json.loads(live.cli("run", payload=json.dumps(request) + "\n"))
                    require(reply["code"] == "APP_FAILED" and reply["delivery"] == "refused", f"failed App admitted work: {reply}")
                issues = live.requests({"op": "issues"})[0]["issues"]
                require(any(issue["code"] == "APP_FAILED" for issue in issues), "diagnosis missing")
                time.sleep(0.2)
                final = live.requests({"op": "inspect"})[0]
                require(final["live"]["window"]["frames"] == frames, "failed App continued presenting")
                require(not final["live"]["window"]["pending"], "failed App retained presentation demand")
                closed = False
                if args.native_helper:
                    subprocess.run([str(args.native_helper.resolve()), "close", str(live.process.pid)], check=True, timeout=args.timeout)
                    require(live.process.wait(timeout=args.timeout) == 1, "failed host must exit with failure status")
                    require(not live.endpoint.exists(), "shutdown left its Inspector socket behind")
                    closed = True
                results.append({"case": case, "frames": frames, "failed": True,
                                "panic_delivery": "unknown", "os_close_verified": closed,
                                "host": final["live"]})
    (args.output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    print(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()

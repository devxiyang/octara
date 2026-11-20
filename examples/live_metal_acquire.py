#!/usr/bin/env python3
"""Compare Octara's synchronous and worker Metal acquisition on an unlocked macOS desktop.

Run with the same release window binary and the live Inspector CLI in both modes:
    python3 examples/live_metal_acquire.py --window target/release/window --octara target/debug/octara

Each mode injects 20 ms at acquisition. The Inspector client queues three inputs in
one write so the socket's idle polling interval cannot schedule the follow-up inputs.
The shell trace measures actual dispatch. The inputs exercise Octara's framework route
in real windows. A resize queued behind an edit must retire the worker's outstanding
drawable. Display captures must come from the window server.
Use --render-worker to also verify worker-side recording and submission.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import struct
import sys
import tempfile
import time

sys.dont_write_bytecode = True
from live_inspect import LiveWindow, SmokeFailure, find, require


DELAY_MS = 20
ACQUIRE = "octara.acquire_trace/1"
INPUT = "octara.inspect_delivery/1"
FRAME = "octara.frame_trace/1"
TRACE_SCHEMAS = tuple(schema.encode() for schema in (ACQUIRE, INPUT, FRAME))


def pipeline(app, *requests):
    """Queue the whole input batch, then read the ordered receipts."""
    payload = "".join(json.dumps(request) + "\n" for request in requests).encode()
    with app.wire() as stream:
        sent = time.monotonic_ns()
        require(stream.write(payload) == len(payload), "could not queue the complete input batch")
        elapsed = []
        replies = []
        for request in requests:
            line = stream.readline()
            elapsed.append(round((time.monotonic_ns() - sent) / 1e6, 3))
            require(line, "Inspector ended before replying to the full batch")
            reply = json.loads(line)
            app.transcript.write(json.dumps({"wire": request, "reply": reply}) + "\n")
            require(reply.get("ok") and reply.get("delivery") == "delivered", f"{app.fixture}: {request['op']} failed: {reply}")
            replies.append(reply)
    app.transcript.flush()
    return elapsed, replies


def submitted(app, window, *, after=None):
    until = time.monotonic() + app.args.timeout
    with app.wire() as stream:
        while True:
            reply = app.wire_request(stream, {"op": "inspect", "window": window})
            completion = reply.get("completion", {})
            surface = completion.get("surface", {})
            paint = reply.get("paint", {})
            if reply.get("ok") and completion.get("layout") == "settled" and surface.get("state") == "submitted":
                require(paint.get("origin") == "submitted" and paint["frame"] == surface["frame"] and paint["window"] == window,
                        f"{app.fixture}: current submission has no matching paint receipt")
                if after is None or paint["revision"] > after:
                    return reply["snapshot"], paint
            require(time.monotonic() < until, f"{app.fixture}: window {window} did not submit its current drawing")
            time.sleep(0.02)


def display_capture(app, name, snapshot, submitted_paint):
    path = app.output / f"{app.fixture}-{name}.png"
    window = snapshot["window"]
    report = app.requests({"op": "capture", "window": window, "pixels": "display", "path": str(path), "settle_ms": 2000})[0]["capture"]
    require(report["pixels"] == report["renderer"] == "display",
            f"{app.fixture}: {name} could not sample the window server: {report.get('fallback')}")
    paint = report["paint"]
    require(paint["origin"] == "submitted" and paint["window"] == window and paint["revision"] >= submitted_paint["revision"]
            and paint["surface_generation"] == submitted_paint["surface_generation"]
            and paint["device_generation"] == submitted_paint["device_generation"],
            f"{app.fixture}: {name} capture lost its current submission")
    pixels = path.read_bytes()
    require(pixels[:8] == b"\x89PNG\r\n\x1a\n" and len(pixels) >= 24, f"{app.fixture}: {name} capture is not a PNG")
    dimensions = struct.unpack(">II", pixels[16:24])
    require(dimensions == (report["width"], report["height"]) == tuple(round(side * report["scale"]) for side in snapshot["viewport"]),
            f"{app.fixture}: {name} capture does not match the viewport")
    return {
        "size_px": dimensions, "revision": paint["revision"], "frame": paint["frame"],
        "surface_generation": paint["surface_generation"], "sha256": hashlib.sha256(pixels).hexdigest(),
    }


def trace_rows(log, offset):
    with Path(log).open("rb") as file:
        file.seek(offset)
        for line in file:
            if not (line.startswith(b"{") and line.endswith(b"\n") and any(schema in line for schema in TRACE_SCHEMAS)):
                continue
            row = json.loads(line)
            if row.get("schema") in (ACQUIRE, INPUT, FRAME):
                yield row


def window_acquisitions(rows, window):
    acquisitions = [row for row in rows if row["schema"] == ACQUIRE and row["window"] == window]
    starts = {row["acquisition"]: row for row in acquisitions if row["phase"] == "start"}
    returns = {row["acquisition"]: row for row in acquisitions if row["phase"] == "return"}
    return starts, returns


def first_acquisition(starts, returns, after):
    candidates = [row for key, row in starts.items() if row["at_us"] >= after and key in returns]
    start = min(candidates, key=lambda row: row["at_us"], default=None)
    return (start, returns[start["acquisition"]]) if start else (None, None)


def window_submissions(rows, window):
    return [row for row in rows if row["schema"] == FRAME and row["submitted"] and (row.get("paint") or {}).get("window") == window]


def measure(log, offset, route, main, note, reply_ms):
    rows = list(trace_rows(log, offset))
    inputs = [row for row in rows if row["schema"] == INPUT]
    require([row["window"] for row in inputs] == [note, main, note],
            f"{route}: missing trace for the note, main, note input batch")
    first, other, latest = [row["at_us"] for row in inputs]
    starts, returns = window_acquisitions(rows, note)
    start, end = first_acquisition(starts, returns, first)
    require(start is not None, f"{route}: no acquisition followed the first note edit")
    require(start["route"] == end["route"] == route, f"expected {route} acquisition, got {start['route']} → {end['route']}")
    entered = end.get("started_us", start["at_us"])
    returned = end.get("returned_us", end["at_us"])
    acquire_ms = (returned - entered) / 1000
    require(acquire_ms >= DELAY_MS - 1, f"{route}: acquire lasted only {acquire_ms:.3f} ms")
    next_start = min(
        (row["at_us"] for row in starts.values() if row["at_us"] > start["at_us"]),
        default=float("inf"),
    )
    submissions = [row for row in window_submissions(rows, note) if returned <= row["timeline_us"]["present_returned"] < next_start]
    submitted_revision = submissions[0]["revision"] if submissions else None
    latest_revision = inputs[-1]["revision"]
    main_during = entered <= other < returned
    note_during = entered <= latest < returned
    if main_during and note_during:
        require(latest_revision is not None and submitted_revision is not None and submitted_revision >= latest_revision,
                f"{route}: first submission after the measured acquire used {submitted_revision}, latest note revision was {latest_revision}")
    return {
        "acquisition_ms": round(acquire_ms, 3),
        "note_edit_to_acquire_ms": round((entered - first) / 1000, 3),
        "acquisition_status": end.get("status"),
        "main_handled_ms_from_acquire_start": round((other - entered) / 1000, 3),
        "latest_note_handled_ms_from_acquire_start": round((latest - entered) / 1000, 3),
        "main_handled_ms_from_acquire_return": round((other - returned) / 1000, 3),
        "latest_note_handled_ms_from_acquire_return": round((latest - returned) / 1000, 3),
        "main_handled_during_acquire": main_during,
        "latest_note_handled_during_acquire": note_during,
        "both_handled_after_return": min(other, latest) >= returned,
        "latest_note_revision": latest_revision,
        "acquired_drawable_submitted_revision": submitted_revision,
        "render_worker": bool(submissions and submissions[0].get("render_worker") is True),
        "reply_ms_from_client_queue": reply_ms,
    }


def resize_during_acquire(app, note, previous, text):
    attempts = []
    for width, height in ((440, 300), (432, 296), (440, 300), (432, 296)):
        time.sleep(0.06)
        offset = Path(app.log.name).stat().st_size
        reply_ms, (_, resize_reply) = pipeline(
            app,
            {"op": "type", "window": note, "id": "body", "text": "r"},
            {"op": "size", "window": note, "width": width, "height": height},
        )
        text += "r"
        live = resize_reply["live"]
        detail = live["resize"]
        require(live["route"] == "native-window" and detail["matched"] and detail["synchronized"]
                and detail["requested"] == [width, height],
                f"{app.fixture}: resize did not synchronize: {detail}")
        snapshot, paint = submitted(app, note, after=previous["revision"])
        require(all(abs(view - actual) < 0.2 for view, actual in zip(snapshot["viewport"], detail["actual"]))
                and all(abs(actual * detail["scale"] - physical) < 0.2 for actual, physical in zip(detail["actual"], detail["physical"])),
                f"{app.fixture}: native, submitted, and framework windows disagree after resize: {detail}")
        require(find(snapshot, "body").get("value") == text, f"{app.fixture}: resize lost the note edit")
        require(paint["surface_generation"] > previous["surface_generation"] and paint["device_generation"] == previous["device_generation"],
                f"{app.fixture}: resize did not submit a new surface generation on the same device")

        rows = list(trace_rows(app.log.name, offset))
        edits = [row for row in rows if row["schema"] == INPUT and row["window"] == note and row["route"] == "framework"]
        require(len(edits) == 1, f"{app.fixture}: missing delivery trace for the resize batch's note edit")
        starts, returns = window_acquisitions(rows, note)
        start, end = first_acquisition(starts, returns, edits[0]["at_us"])
        attempt = {
            "size_px": detail["physical"],
            "acquisition": start["acquisition"] if start else None,
            "route": end["route"] if end else None,
            "acquisition_status": end["status"] if end else None,
            "old_surface_generation": previous["surface_generation"],
            "new_surface_generation": paint["surface_generation"],
            "reply_ms_from_client_queue": reply_ms,
        }
        attempts.append(attempt)
        if app.fixture == "sync":
            return snapshot, paint, attempts
        if end and end["route"] == "worker" and end["status"] == "stale":
            returned = end.get("returned_us", end["at_us"])
            following = next((row for row in window_submissions(rows, note) if row["timeline_us"]["present_returned"] >= returned), None)
            require(following is not None and following["surface_generation"] > previous["surface_generation"]
                    and following["size_px"] == detail["physical"],
                    f"worker: stale acquisition was not followed by a submission for the resized surface: {attempt}")
            return snapshot, paint, attempts
        previous = paint
    raise SmokeFailure(f"worker: the native resize did not retire a queued acquisition in four attempts: {attempts}")


def note_window(app, number):
    notes = [window["window"] for window in app.inspect()["windows"] if window["title"] == f"Note {number}"]
    require(len(notes) == 1, f"{app.fixture}: Note {number} did not open")
    return notes[0]


def run_mode(args, output, sockets, route):
    env = {
        "WGPU_BACKEND": "metal", "OCTARA_TRACE_INPUT": "1",
        "OCTARA_ASYNC_METAL": "1" if route == "worker" else "0",
        "OCTARA_METAL_ACQUIRE_DELAY_MS": str(DELAY_MS),
    }
    if args.render_worker and route == "worker":
        env["OCTARA_RENDER_WORKER"] = "1"
    with LiveWindow(route, args, output, sockets, ["windows"], env=env) as app:
        main = app.inspect()
        main_window = main["window"]
        app.requests({"op": "tap", "window": main_window, "id": "new"})
        note = note_window(app, 1)
        app.requests({"op": "focus", "window": note, "id": "body"})
        snapshot, paint = submitted(app, note)
        before = display_capture(app, "before", snapshot, paint)

        measurements = []
        text = ""
        for first, last in [("A", "a"), ("B", "b"), ("C", "c")]:
            app.requests({"op": "focus", "window": main_window, "id": "new"})
            _, previous = submitted(app, note)
            # Give an idle note a fresh immediate opportunity for the first edit.
            time.sleep(0.06)
            offset = Path(app.log.name).stat().st_size
            reply_ms, _ = pipeline(
                app,
                {"op": "type", "window": note, "id": "body", "text": first},
                {"op": "key", "window": main_window, "chord": "tab"},
                {"op": "type", "window": note, "id": "body", "text": last},
            )
            text += first + last
            snapshot, paint = submitted(app, note, after=previous["revision"])
            require(find(snapshot, "body").get("value") == text, f"{route}: newest note edit did not reach the submitted drawing")
            require(app.inspect().get("focus") == "open", f"{route}: tab did not reach the main window")
            measurements.append(measure(app.log.name, offset, route, main_window, note, reply_ms))

        after = display_capture(app, "after-input", snapshot, paint)
        require(before["sha256"] != after["sha256"], f"{route}: note pixels did not change after input")
        snapshot, resized_paint, resize_attempts = resize_during_acquire(app, note, paint, text)
        require(app.inspect()["viewport"] == main["viewport"], f"{route}: resize changed the wrong window")
        after_resize = display_capture(app, "after-resize", snapshot, resized_paint)

        time.sleep(0.06)
        pipeline(app, {"op": "type", "window": note, "id": "body", "text": "!"}, {"op": "tap", "window": note, "id": "close"})
        with app.wire() as stream:
            closed = app.wire_request(stream, {"op": "inspect", "window": note})
            require(not closed.get("ok") and closed.get("code") == "BAD_REQUEST", f"{route}: closed window still accepts requests")
        app.requests({"op": "tap", "window": main_window, "id": "new"})
        reopened = note_window(app, 2)
        require(reopened != note, f"{route}: reopened window reused the closed window ID")
        app.requests({"op": "type", "window": reopened, "id": "body", "text": "Reopened"})
        snapshot, reopened_paint = submitted(app, reopened)
        require(find(snapshot, "body").get("value") == "Reopened", f"{route}: reopened window did not receive input")
        after_reopen = display_capture(app, "after-reopen", snapshot, reopened_paint)
        app.requests({"op": "tap", "window": reopened, "id": "close"})

    return {"bursts": measurements, "after_input": after, "resize_attempts": resize_attempts,
            "after_resize": after_resize, "after_reopen": after_reopen}


def main():
    repo = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--window", type=Path, default=repo / "target/release/window")
    parser.add_argument("--octara", type=Path, default=repo / "target/debug/octara")
    parser.add_argument("--output", type=Path, help="new or empty directory in which to keep logs and captures")
    parser.add_argument("--timeout", type=float, default=20)
    parser.add_argument("--render-worker", action="store_true", help="also verify recording and submission on the worker")
    args = parser.parse_args()
    require(sys.platform == "darwin", "this fixture needs macOS and an unlocked desktop")
    require(0 < args.timeout <= 30, "--timeout must be greater than zero and at most 30 seconds")
    args.repo = repo
    for name in ("window", "octara"):
        path = getattr(args, name).resolve()
        require(path.is_file() and os.access(path, os.X_OK), f"build the executable first: {path}")
        setattr(args, name, path)

    # LiveWindow copies the fixture's environment for its app and Inspector CLI processes.
    allowed = {name: os.environ[name] for name in ("HOME", "LOGNAME", "USER", "LANG", "LC_ALL", "TMPDIR") if name in os.environ}
    os.environ.clear()
    os.environ.update(allowed, PATH="/usr/bin:/bin:/usr/sbin:/sbin")
    with tempfile.TemporaryDirectory(prefix="octara-metal-", dir="/tmp") as temporary:
        sockets = Path(temporary)
        output = args.output.resolve() if args.output else sockets
        output.mkdir(parents=True, exist_ok=True)
        require(not any(output.iterdir()), "--output must be empty")
        results = {route: run_mode(args, output, sockets, route) for route in ("sync", "worker")}
        print(json.dumps({"injected_ms": DELAY_MS, **results}, indent=2), flush=True)
        sync = results["sync"]["bursts"]
        require(not any(row["main_handled_during_acquire"] or row["latest_note_handled_during_acquire"] for row in sync)
                and any(row["both_handled_after_return"] for row in sync),
                "the trace did not show a synchronous acquire holding both queued inputs until it returned")
        overlapping = [
            row for row in results["worker"]["bursts"]
            if row["main_handled_during_acquire"] and row["latest_note_handled_during_acquire"]
        ]
        require(overlapping, "no worker acquisition handled both queued follow-up inputs within the injected wait")
        if args.render_worker:
            require(any(row["render_worker"] for row in overlapping),
                    "no render worker frame submitted the latest note edit after a measured acquisition")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (SmokeFailure, OSError, ValueError, KeyError, struct.error) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)

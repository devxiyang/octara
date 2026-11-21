#!/usr/bin/env python3
"""Compare two and three Metal drawables on a visible 1920x1080 @ 120 Hz display.

Build the release window and the Inspector CLI and put the window on that display.
Each run replays 27 seconds of Shader Lab. Inspector clicks, scrolling, and drag moves
are timed from framework dispatch to the first Metal display of the resulting revision.
The report counts changed revisions that had no attributable display separately.
Use --scenario admission to check window and capture ordering at the current display mode.
"""

import argparse
from collections import defaultdict
import json
from pathlib import Path
from statistics import quantiles
import struct
import sys
import tempfile
import time

sys.dont_write_bytecode = True
from live_inspect import LiveWindow, SmokeFailure, find, require


FRAME = "octara.frame_trace/1"
PRESENTED = "octara.presented_trace/1"
INPUT = "octara.inspect_delivery/1"
REPLAY = "octara.shader_lab_replay/1"
SURFACE = "octara.surface_trace/1"
ACQUIRE = "octara.acquire_trace/1"
ADMISSION = "octara.device_admission/1"
OUTPUT = "octara.capture_trace/1"
OPPORTUNITY = "octara.display_opportunity/1"
SCHEMAS = (FRAME, PRESENTED, INPUT, REPLAY, SURFACE, ACQUIRE, ADMISSION, OUTPUT, OPPORTUNITY)
PERIOD_US = 1_000_000 / 120
SLIDER = "gpu-glass-refraction"


def statistics(values):
    values = sorted(value / 1000 for value in values)
    if not values:
        return None
    p = quantiles(values, n=100, method="inclusive") if len(values) > 1 else values * 99
    return {"count": len(values), "p50_ms": round(p[49], 3), "p95_ms": round(p[94], 3),
            "p99_ms": round(p[98], 3), "max_ms": round(values[-1], 3)}


def frame_key(record):
    return tuple(record[field] for field in ("window", "frame", "revision", "surface_generation", "device_generation"))


def read_trace(log, schemas=SCHEMAS):
    rows = defaultdict(list)
    with Path(log).open("rb") as stream:
        offset = 0
        for line in stream:
            if line.startswith(b"{") and line.endswith(b"\n"):
                row = json.loads(line)
                if row.get("schema") in schemas:
                    rows[row["schema"]].append({"offset": offset, **row})
            offset += len(line)
    return rows


def window_frames(rows, window):
    replay = [row for row in rows[REPLAY] if row["window"] == window]
    require([row["event"] for row in replay] == ["start"] + ["checkpoint"] * 6 + ["finish"],
            f"window {window}: replay did not complete six checkpoints: {replay}")
    lo = replay[0]["app_clock_ms"] + 250
    hi = replay[0]["app_clock_ms"] + replay[0]["duration_ms"]
    submissions = {frame_key(row["paint"]): row for row in rows[FRAME]
                   if row["submitted"] and row["render_worker"] and row["paint"]["window"] == window
                   and lo <= row["app_clock_ms"] < hi}
    displays = sorted((row for row in rows[PRESENTED] if frame_key(row) in submissions), key=lambda row: row["presented_us"])
    require(len(displays) >= 2, f"window {window}: received fewer than two actual Metal displays")
    require(len({frame_key(row) for row in displays}) == len(displays), f"window {window}: duplicate presentation callback")
    return (lo, hi), submissions, displays


def frame_report(rows, window, bounds, submissions, displays):
    times = sorted(row["presented_time_s"] * 1_000_000 for row in displays)
    require(len(set(times)) == len(times), f"window {window}: several frames have the same display timestamp")
    gaps = [later - earlier for earlier, later in zip(times, times[1:])]
    # These intervals count attributable pending presentations, including compositor time.
    # A drawable acquired or rendered but not yet scheduled is outside this count.
    endpoints = sorted((at, change) for row in displays for at, change in
                       ((row["scheduled_us"], 1), (row["presented_us"], -1)))
    pending = peak = 0
    for _, change in endpoints:
        pending += change
        peak = max(peak, pending)
    acquisitions = {row["acquisition"]: row for row in rows[ACQUIRE]
                    if row["window"] == window and row["phase"] == "return" and row["status"] == "ready"}
    acquired = [acquisitions[row["acquisition_request"]] for row in submissions.values()
                if row["acquisition_request"] in acquisitions]
    winit_window = next(iter(submissions.values()))["window"]
    lo, hi = (ms * 1000 for ms in bounds)
    empty_ticks = [row["opportunity"]["target_us"] for row in rows[OPPORTUNITY]
                   if row["window"] == winit_window and row["skip_reason"] == "no_pending_drawing"
                   and lo <= row["opportunity"]["handled_us"] < hi]
    display_opportunities = [row["opportunity"] for row in submissions.values() if row["opportunity"]["source"] == "display_link"]
    submitted_targets = [opportunity["target_us"] for opportunity in display_opportunities if opportunity["target_us"] is not None]
    duplicates = len(submitted_targets) - len(set(submitted_targets))
    if any("expires_us" in opportunity for opportunity in display_opportunities):
        require(duplicates == 0, f"window {window}: submitted several frames for one display target")
    displayed_targets = {opportunity["target_us"] for row in displays
                         if (opportunity := submissions[frame_key(row)]["opportunity"])["source"] == "display_link"
                         and opportunity["target_us"] is not None}
    return {
        "submitted": len(submissions), "displayed": len(displays), "submitted_without_display": len(submissions) - len(displays),
        "display_fps": round(1_000_000 * len(gaps) / (times[-1] - times[0]), 3),
        "inter_display": statistics(gaps), "gaps_over_1_5_refresh": sum(gap > 1.5 * PERIOD_US for gap in gaps),
        "scheduled_to_display": statistics(row["presented_us"] - row["scheduled_us"] for row in displays),
        "acquisition_wait": statistics(row["at_us"] - row["started_us"] for row in acquired),
        "peak_matched_pending_presents": peak,
        "empty_display_ticks": len(empty_ticks),
        "empty_ticks_used_for_display": len(displayed_targets.intersection(empty_ticks)),
    }


def input_report(deliveries, displays, probes):
    results = defaultdict(lambda: {"dispatch_to_display": [], "publication_to_display": [], "unchanged": 0, "unmatched": 0})
    for kind, start, end in probes:
        inputs = [row for row in deliveries if start <= row["offset"] < end]
        require([row.get("drag_step") for row in inputs] == ([1, 2, 3, 4] if kind == "drag" else [None]),
                f"{kind}: missing or ambiguous input deliveries")
        result = results[kind]
        changed = []
        for row in inputs:
            if row["revision"] is None or row["before_revision"] is None:
                result["unmatched"] += 1
            elif row["revision"] == row["before_revision"]:
                result["unchanged"] += 1
            else:
                changed.append(row)
        for i, row in enumerate(changed):
            next_revision = changed[i + 1]["revision"] if i + 1 < len(changed) else float("inf")
            # A later drag move can replace this value. An arbitrary later frame is not evidence
            # for a skipped move. Click and scroll hold their value for this entire search bound.
            shown = next((frame for frame in displays
                          if row["at_us"] <= frame["presented_us"] < row["at_us"] + 200_000
                          and row["revision"] <= frame["revision"] < next_revision), None)
            if shown is None:
                result["unmatched"] += 1
                continue
            result["dispatch_to_display"].append(shown["presented_us"] - row["started_us"])
            result["publication_to_display"].append(shown["presented_us"] - row["at_us"])
    return {kind: {**result, **{field: statistics(result[field]) for field in ("dispatch_to_display", "publication_to_display")}}
            for kind, result in results.items()}


def admission_report(rows, windows, outputs):
    decisions = rows[ADMISSION]
    require(all(row["capture"] in windows and row["selected"] in ("window", "capture")
                and row["window"] in [None, *windows] and row["capture_wait_us"] >= 0
                for row in decisions), "admission: invalid work receipt")
    captures = [row for row in decisions if row["selected"] == "capture"]
    require(len(captures) == len(outputs) and all(
        start["capture"] == output["window"] and start["at_us"] <= output["started_us"]
        for start, output in zip(captures, outputs)
    ), "admission: completed captures did not match the GPU admissions in order")
    windows_first = [row for row in decisions if row["selected"] == "window"]
    require(all(row["window"] in windows and (row["frame_deadline_us"] is None or row["at_us"] < row["frame_deadline_us"])
                for row in windows_first), "admission: a window won with no current frame opportunity")

    png_intervals = [(start["device_generation"], output["started_us"], output["started_us"] + output["elapsed_us"])
                     for start, output in zip(captures, outputs)]
    frames_during_output = {window: 0 for window in windows}
    for frame in rows[FRAME]:
        if not (frame["submitted"] and frame["render_worker"]):
            continue
        paint = frame["paint"]
        submitted = frame["timeline_us"]["render_submitted"]
        if paint["window"] in frames_during_output and submitted is not None and any(
            paint["device_generation"] == device and start < submitted < end for device, start, end in png_intervals
        ):
            frames_during_output[paint["window"]] += 1
    require(all(frames_during_output.values()), "admission: both windows must submit while painted PNG output is running")
    captures_first = [row for row in captures if row["window"] is not None]
    return {"window_first": len(windows_first), "capture_first_with_window_ready": len(captures_first),
            "contended_capture_wait": statistics(row["capture_wait_us"] for row in captures_first),
            "frames_during_output": frames_during_output}


def exercise(app, stream, scenario):
    def request(op, window=None, **fields):
        message = {"op": op, **fields}
        if window is not None:
            message["window"] = window
        reply = app.wire_request(stream, message)
        require(reply.get("ok"), f"{app.fixture}: {message} failed: {reply}")
        return reply

    def inspect(window=None):
        return request("inspect", window)["snapshot"]

    main = inspect()["window"]
    windows = [main]
    if scenario in ("dual", "admission"):
        request("invoke", main, command="shader-lab.replay.open-peer")
        peer = [w["window"] for w in inspect()["windows"] if w["title"] == "Octara Shader Lab Peer"]
        require(len(peer) == 1, "the peer window did not open")
        windows.extend(peer)
    request("invoke", main, command="shader-lab.replay.start")
    origin = time.monotonic()
    if len(windows) == 2:
        request("invoke", windows[1], command="shader-lab.replay.start")

    log = Path(app.log.name)
    probes = []

    def at(second):
        time.sleep(max(0, origin + second - time.monotonic()))

    def probe(op, **fields):
        before = log.stat().st_size
        reply = request(op, main, **fields)
        require(reply.get("delivery") == "delivered", f"{op}: operation was not delivered")
        probes.append((op, before, log.stat().st_size))
        return reply

    def capture(window, cycle, index, size):
        path = app.output / f"{app.fixture}-{cycle}-{index}.png"
        report = request("capture", window, path=str(path), pixels="painted", scale=2, settle_ms=0)["capture"]
        require(report["pixels"] == "painted" and report["renderer"] == "gpu", "capture did not use GPU paint")
        require(report["paint"]["origin"] == "rerecorded" and report["paint"]["window"] == window,
                "capture did not record the requested window")
        with path.open("rb") as image:
            header = image.read(24)
        require(header[:8] == b"\x89PNG\r\n\x1a\n" and struct.unpack(">II", header[16:24]) == tuple(round(side * 2) for side in size),
                f"capture did not write a 2× PNG for {size}")
        path.unlink()

    for cycle in range(6):
        start = cycle * 4.5
        if scenario == "admission":
            for index, window in enumerate(windows):
                # Vary the sample phase relative to the display clock across rounds.
                at(start + 0.9 * (index + 1) + cycle * 0.003)
                capture(window, cycle, index, inspect(window)["viewport"])
            continue
        if scenario == "capture":
            for index, (second, size) in enumerate(((0.9, (1320, 820)), (1.8, (1440, 900)))):
                at(start + second)
                width, height = size
                detail = request("size", main, width=width, height=height)["live"]["resize"]
                require(detail["matched"] and detail["synchronized"] and detail["physical"] == [width, height],
                        f"native resize did not converge at 1×: {detail}")
                capture(main, cycle, index, size)

        at(start + 2.60)
        slider = find(inspect(main), SLIDER)
        probe("drag", id=SLIDER, dx=-0.4 * slider["bounds"][2], dy=0)
        require(find(inspect(main), SLIDER)["numeric_value"] < slider["numeric_value"], "drag did not lower refraction")
        at(start + 3.67)
        require(probe("scroll", id="gpu-materials", dx=0, dy=-40)["scroll"]["delta"][1] < 0, "materials did not scroll")
        at(start + 3.95)
        checked = find(inspect(main), "gpu-grid")["checked"]
        probe("tap", id="gpu-grid")
        require(find(inspect(main), "gpu-grid")["checked"] != checked, "tap did not toggle the grid")
        at(start + 4.25)
        if inspect(main).get("focus") != "gpu-glass-open":
            request("focus", main, id="gpu-glass-open")
    at(27.6)
    return windows, probes


def compare(args, temporary, scenario, depth, repeat):
    name = f"{scenario}-{depth}-r{repeat}"
    print(f"Measuring {name}", file=sys.stderr, flush=True)
    env = {"WGPU_BACKEND": "metal", "OCTARA_ASYNC_METAL": "1", "OCTARA_RENDER_WORKER": "1", "OCTARA_TRACE_INPUT": "1",
           "OCTARA_TRACE_PRESENTED": "1", "OCTARA_TRACE_FRAME_CLOCK": "0", "OCTARA_TRACE_GPU": "0",
           "OCTARA_METAL_FRAME_LATENCY": str(depth - 1), "OCTARA_METAL_ACQUIRE_DELAY_MS": "0"}
    with LiveWindow(name, args, temporary, temporary, ["gpu_lab", "replay", "120", "stress"], env=env) as app:
        with app.wire() as stream:
            windows, probes = exercise(app, stream, scenario)
        rows = read_trace(app.log.name)
    configured = [row for row in rows[SURFACE] if row["configured"]]
    require(configured and all(row["metal_drawables"] == depth for row in configured),
            f"{name}: unexpected number of drawables on a configured CAMetalLayer")
    outputs = [row for row in rows[OUTPUT] if row["phase"] == "painted_output" and row["saved"]]
    expected_outputs = 12 if scenario in ("capture", "admission") else 0
    require(len(outputs) == expected_outputs, f"{name}: expected {expected_outputs} completed worker outputs")
    result = {"scenario": scenario, "drawables": depth, "round": repeat, "painted_outputs": len(outputs), "windows": []}
    if scenario == "admission":
        result["admission"] = admission_report(rows, windows, outputs)
        for window in windows:
            _, submissions, displays = window_frames(rows, window)
            result["windows"].append({"window": window, "submitted": len(submissions), "displayed": len(displays)})
        return result
    for index, window in enumerate(windows):
        bounds, submissions, displays = window_frames(rows, window)
        expected_sizes = {(800, 620)} if index else {(1440, 900), (1320, 820)} if scenario == "capture" else {(1440, 900)}
        require({tuple(row["size_px"]) for row in displays} == expected_sizes, f"{name}: unexpected or missing display sizes")
        report = {"window": window, **frame_report(rows, window, bounds, submissions, displays)}
        if not index:
            report["inputs"] = input_report([row for row in rows[INPUT] if row["window"] == window], displays, probes)
        result["windows"].append(report)
    return result


def main():
    repo = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--window", type=Path, default=repo / "target/release/window")
    parser.add_argument("--octara", type=Path, default=repo / "target/debug/octara")
    parser.add_argument("--scenario", choices=("single", "dual", "capture", "admission"), action="append",
                        help="default: single, dual, and capture")
    parser.add_argument("--rounds", type=int, default=2, help="paired runs per scene, with alternating order")
    args = parser.parse_args()
    require(sys.platform == "darwin" and args.rounds >= 1, "run on macOS with at least one paired round")
    args.repo, args.timeout = repo, 5
    args.window, args.octara = args.window.resolve(), args.octara.resolve()
    require(args.window.is_file() and args.octara.is_file(), "build the release window example and the Octara Inspector CLI first")
    with tempfile.TemporaryDirectory(prefix="octara-latency-", dir="/tmp") as temporary:
        for scenario in args.scenario or ("single", "dual", "capture"):
            window_first = 0
            for repeat in range(1, args.rounds + 1):
                for depth in ((3, 2) if repeat % 2 else (2, 3)):
                    result = compare(args, Path(temporary), scenario, depth, repeat)
                    print(json.dumps(result), flush=True)
                    if scenario == "admission":
                        window_first += result["admission"]["window_first"]
            if scenario == "admission":
                require(window_first > 0, "admission: no ready window won a device turn while a capture waited")


if __name__ == "__main__":
    try:
        main()
    except (SmokeFailure, OSError, ValueError, KeyError, struct.error) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)

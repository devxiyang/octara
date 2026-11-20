#!/usr/bin/env python3
"""Measure system input during GPU drawing, resizing, and painted PNG captures on macOS."""

import argparse
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
import json
from pathlib import Path
from statistics import median
import struct
import subprocess
import sys
import tempfile
from threading import Event
import time

sys.dont_write_bytecode = True
from live_inspect import LiveWindow, SmokeFailure, find, require
from live_render_latency import FRAME, OUTPUT, PERIOD_US, PRESENTED, SCHEMAS, SURFACE, frame_key, frame_report, read_trace, statistics


NATIVE = "octara.window_input/1"
REPLAY = "octara.shader_lab_input/1"
CAPTURE_OFFSETS = (-0.015, 0.36, 0.775, 1.405)
CYCLES = 6
PER_CYCLE = 4.15
START = 0.65
TARGETS = ("pressure-toggle", "pressure-slider", "gpu-materials", "pressure-editor")
PRESENT_WINDOW_US = 250_000


@contextmanager
def native_driver(binary, pid):
    process = subprocess.Popen([binary, str(pid)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)

    def call(op, **fields):
        process.stdin.write(json.dumps({"op": op, **fields}) + "\n")
        process.stdin.flush()
        line = process.stdout.readline()
        require(line, "the PID-scoped native input driver stopped responding")
        reply = json.loads(line)
        require(not reply.get("error"), f"{op}: {reply.get('error')}")
        return reply

    try:
        yield call
    finally:
        process.stdin.close()
        try:
            status = process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
            raise SmokeFailure("the native driver timed out while restoring the input source")
        finally:
            process.stdout.close()
        require(status == 0, "the native driver could not finish normally")


def wait_until(when):
    time.sleep(max(0, when - time.monotonic()))


def capture_bursts(app, window, origin, stop):
    with app.wire() as stream:
        for cycle in range(CYCLES):
            for index, offset in enumerate(CAPTURE_OFFSETS):
                if stop.wait(max(0, origin + START + cycle * PER_CYCLE + offset - time.monotonic())):
                    return
                path = app.output / f"{app.fixture}-{cycle}-{index}.png"
                reply = app.wire_request(stream, {"op": "capture", "window": window, "path": str(path), "pixels": "painted",
                                                  "scale": 2, "settle_ms": 0})
                require(reply.get("ok"), f"painted capture failed: {reply}")
                report = reply["capture"]
                with path.open("rb") as image:
                    header = image.read(24)
                require(report["pixels"] == "painted" and report["renderer"] == "gpu" and header[:8] == b"\x89PNG\r\n\x1a\n"
                        and struct.unpack(">II", header[16:24]) in {(2640, 1640), (2880, 1800)}, "capture lost its sampled 2× pixels")
                path.unlink()


def probe_inputs(native, before, origin, pinyin):
    frame = native("bounds")["bounds"]
    title_height = frame[3] - before["viewport"][1]
    require(0 <= title_height < 100 and frame[2] == before["viewport"][0], "window bounds disagree with Inspector viewport")
    controls = {name: find(before, name)["bounds"] for name in TARGETS}
    sent, resizes, dismissals = [], [], []

    def point(target, fraction=0.5):
        x, y, w, h = controls[target]
        return [frame[0] + x + w * fraction, frame[1] + title_height + y + 0.5 * h]

    def event(kind, label=None, **fields):
        stamp = native(kind, **fields)["sent_s"]
        if kind != "move":
            position = None
            if kind == "drag":
                x, y = fields["point"]
                position = (x - frame[0], y - frame[1] - title_height)
            sent.append((kind, label, stamp, position))

    def down(target, fraction=0.5):
        position = point(target, fraction)
        event("move", point=position)
        time.sleep(0.012)
        event("down", point=position)
        return position

    def tap(target, label=None, fraction=0.5):
        position = down(target, fraction)
        time.sleep(0.012)
        event("up", label, point=position)

    def resize(width, height):
        nonlocal frame
        update = native("resize", point=[width, height + title_height])
        deadline = time.monotonic() + 0.5
        while True:
            frame = native("bounds")["bounds"]
            if frame[2:] == [width, height + title_height]:
                break
            require(time.monotonic() < deadline, f"native resize did not reach {width}×{height}: {frame}")
            time.sleep(0.003)
        resizes.append((update["sent_s"], (width, height)))

    for cycle in range(CYCLES):
        start = origin + START + cycle * PER_CYCLE
        for i in range(3):
            wait_until(start + 0.09 * i)
            tap("pressure-toggle", "click")
        wait_until(start + 0.39)
        first, last = (0.16, 0.84) if cycle % 2 == 0 else (0.84, 0.16)
        down("pressure-slider", first)
        for i in range(1, 9):
            wait_until(start + 0.41 + i * 0.022)
            fraction = first + (last - first) * i / 8
            event("drag", "drag", point=point("pressure-slider", fraction))
            if i == 1:
                resize(1320, 820)
        event("up", point=point("pressure-slider", last))

        wait_until(start + 0.775)
        event("move", point=point("gpu-materials"))
        for i in range(6):
            wait_until(start + 0.80 + i * 0.024)
            direction = -1 if cycle % 2 == 0 or cycle == CYCLES - 1 else 1
            event("wheel", "wheel", delta=direction * 20, point=point("gpu-materials"))

        if pinyin and cycle == CYCLES // 2:
            native("pinyin")
        wait_until(start + 1.31)
        tap("pressure-editor", fraction=0.9)
        wait_until(start + 1.46)
        chinese = pinyin and cycle >= CYCLES // 2
        # Hardware virtual key codes for a, b, Left Arrow and n, i, h, a, o, 1.
        if chinese:
            keys = ((45, "pinyin"), (34, "pinyin"), (4, "pinyin"), (0, "pinyin"), (31, "pinyin"), (18, "commit"))
        else:
            keys = ((0, "key"), (123, "selection"), (11, "key"))
        for index, (code, label) in enumerate(keys):
            wait_until(start + 1.47 + index * 0.080)
            event("key", label, code=code, shift=label == "selection")
            if index == 0:
                resize(1440, 900)
        if chinese:
            # Close prediction UI after the measured candidate selection. Keep this cleanup
            # key outside the observed input sequence, whether AppKit or the IME handles it.
            wait_until(start + 2.22)
            dismissals.append(native("key", code=53)["sent_s"])
    return sent, resizes, dismissals


def first_response(displays, publication, superseding_revision):
    return next((frame["presented_us"] for frame in displays
                 if publication["at_us"] <= frame["presented_us"] < publication["at_us"] + PRESENT_WINDOW_US
                 and publication["revision"] <= frame["revision"] < superseding_revision), None)


def motion_intervals(displays, samples):
    runs = []
    for timestamp in samples:
        if not runs or timestamp - runs[-1][-1] > 300_000:
            runs.append([])
        runs[-1].append(timestamp)
    feedback, cadence = [], []
    for run in runs:
        feedback.extend(later - earlier for earlier, later in zip(run, run[1:]))
        frames = [row["presented_time_s"] * 1_000_000 for row in displays if run[0] <= row["presented_us"] <= run[-1]]
        cadence.extend(later - earlier for earlier, later in zip(frames, frames[1:]))
    return {"inter_display": statistics(cadence), "feedback_interval": statistics(feedback),
            "long_display_intervals": sum(gap > 1.5 * PERIOD_US for gap in cadence)}


def delivery_report(rows, displays, sent, resizes, outputs, dismissals):
    # Metal records the process-relative clock together with the absolute Core Animation time.
    epoch = median(frame["presented_time_s"] * 1_000_000 - frame["presented_us"] for frame in displays)
    probes = []
    cleanup_keys = [stamp * 1_000_000 - epoch for stamp in dismissals]
    unmatched_drag = {"posted": 0, "delivered": 0}
    for kind in ("down", "up", "drag", "wheel", "key"):
        events = [(label, stamp * 1_000_000 - epoch, position) for event, label, stamp, position in sent if event == kind]
        if not events:
            continue
        delivered = [row for row in rows if row["kind"] == kind and events[0][1] - 5 <= row["entered_us"] < events[-1][1] + PRESENT_WINDOW_US]
        if kind == "key":
            delivered = [row for row in delivered if not any(at - 5 <= row["entered_us"] < at + PRESENT_WINDOW_US for at in cleanup_keys)]
        if kind == "drag":
            remaining = list(events)
            for row in delivered:
                match = next((i for i, (_, stamp, position) in enumerate(remaining)
                              if stamp - 5 <= row["entered_us"] < stamp + PRESENT_WINDOW_US
                              and all(abs(observed - expected) <= 2 for observed, expected in zip(row["position"], position))), None)
                if match is None:
                    unmatched_drag["delivered"] += 1
                else:
                    label, stamp, _ = remaining.pop(match)
                    probes.append((label, stamp, row))
            unmatched_drag["posted"] = len(remaining)
            continue
        require(len(events) == len(delivered), f"{kind}: expected {len(events)} posted events, observed {len(delivered)}")
        for (label, stamp, _), row in zip(events, delivered, strict=True):
            require(stamp - 5 <= row["entered_us"] < stamp + PRESENT_WINDOW_US, f"{kind}: event and delivery clocks disagree")
            if label:
                probes.append((label, stamp, row))
    probes.sort(key=lambda probe: probe[1])
    resized = []
    for stamp, size in resizes:
        began = stamp * 1_000_000 - epoch
        shown = next((frame["presented_us"] for frame in displays if frame["presented_us"] >= began and tuple(frame["size_px"]) == size), None)
        require(shown is not None and shown - began < 500_000, f"native resize {size} did not produce a timely displayed frame")
        resized.append((began, shown))
    encoding = [(row["started_us"], row["started_us"] + row["elapsed_us"]) for row in outputs]

    results = defaultdict(lambda: {"sent": 0, "unchanged": 0, "without_display": 0, "dispatch": [], "publish": [], "display": [],
                                   "total": [], "during_encode": [], "during_resize": [], "during_both": []})
    # Continuous motions and edits can publish another value before an intermediate revision is shown.
    groups = {"drag": "slider", "wheel": "scroll", "click": "grid"}
    motion = defaultdict(list)
    for i, (label, stamp, row) in enumerate(probes):
        result = results[label]
        result["sent"] += 1
        result["dispatch"].append(row["started_us"] - stamp)
        result["publish"].append(row["at_us"] - row["started_us"])
        revision = row["revision"]
        if revision is None or revision == row["before_revision"]:
            result["unchanged"] += 1
            continue
        group = groups.get(label, "editor")
        following = next((r["revision"] for name, _, r in probes[i + 1:]
                          if groups.get(name, "editor") == group and r["revision"] is not None and r["revision"] > revision), float("inf"))
        shown = first_response(displays, row, following)
        if shown is None:
            result["without_display"] += 1
            continue
        result["display"].append(shown - row["at_us"])
        if label in ("drag", "wheel"):
            motion[label].append(shown)
        total = shown - stamp
        result["total"].append(total)
        during_encode = any(start <= stamp < end for start, end in encoding)
        during_resize = any(start <= stamp < end for start, end in resized)
        if during_encode:
            result["during_encode"].append(total)
        if during_resize:
            result["during_resize"].append(total)
        if during_encode and during_resize:
            result["during_both"].append(total)
    # The native text bridge samples AppKit's actual marked and commit callbacks.
    composition_counts = {kind: sum(row["kind"] == kind for row in rows) for kind in ("composition", "text_commit")}
    return {"inputs": {name: {k: statistics(v) if isinstance(v, list) else v for k, v in sample.items()}
                       for name, sample in results.items()}, "motion": {kind: motion_intervals(displays, shown) for kind, shown in motion.items()},
            "appkit_text": composition_counts, "unmatched_drag": unmatched_drag}


def run(args, scratch, driver, depth, repeat):
    name = f"native-{depth}-r{repeat}"
    print(f"Measuring {name}", file=sys.stderr, flush=True)
    env = {"WGPU_BACKEND": "metal", "OCTARA_ASYNC_METAL": "1", "OCTARA_RENDER_WORKER": "1", "OCTARA_TRACE_INPUT": "1",
           "OCTARA_TRACE_PRESENTED": "1", "OCTARA_TRACE_FRAME_CLOCK": "0", "OCTARA_TRACE_GPU": "0", "OCTARA_TRACE_NATIVE_INPUT": "1",
           "OCTARA_METAL_FRAME_LATENCY": str(depth - 1), "OCTARA_METAL_ACQUIRE_DELAY_MS": "0"}
    with LiveWindow(name, args, scratch, scratch, ["gpu_lab", "input-pressure"], env=env) as app:
        before = app.inspect()
        require(before["viewport"] == [1440, 900], f"expected the native 1440×900 fixture: {before['viewport']}")
        window = before["window"]
        with native_driver(driver, app.process.pid) as native:
            native("latin")
            app.requests({"op": "invoke", "window": window, "command": "shader-lab.replay.start"})
            origin = time.monotonic()
            stop = Event()
            with ThreadPoolExecutor(max_workers=1) as worker:
                capture = worker.submit(capture_bursts, app, window, origin, stop)
                try:
                    sent, resizes, dismissals = probe_inputs(native, before, origin, args.pinyin)
                    capture.result()
                except BaseException as error:
                    stop.set()
                    raise SmokeFailure(f"fixture at {time.monotonic() - origin:.2f}s: {error}") from error
            wait_until(origin + 27.6)
            after = app.inspect()
        require(find(after, "pressure-count")["name"] == str(3 * CYCLES), "native clicks did not reach the test control in order")
        require(find(after, "pressure-slider")["numeric_value"] < 0.3, "native drag did not move the slider")
        require(find(after, "gpu-material-prism")["bounds"][1] < find(before, "gpu-material-prism")["bounds"][1], "native wheel did not scroll materials")
        draft = find(after, "pressure-editor").get("value", "")
        expected = "Zbbb" + "你好" * 3 if args.pinyin else "Z" + "b" * CYCLES
        require(draft == expected, f"native keyboard, selection or IME result differed: {draft!r}, expected {expected!r}")
        rows = read_trace(app.log.name, (*SCHEMAS, NATIVE, REPLAY))

    configs = [row for row in rows[SURFACE] if row["configured"]]
    require(configs and all(row["metal_drawables"] == depth for row in configs), "a configured pool used the wrong drawable count")
    outputs = [row for row in rows[OUTPUT] if row["phase"] == "painted_output" and row["saved"]]
    require(len(outputs) == CYCLES * len(CAPTURE_OFFSETS), "missing completed GPU PNG outputs")
    require([r["event"] for r in rows[REPLAY]] == ["start", "finish"], "the GPU pressure replay did not finish")
    bounds = (rows[REPLAY][0]["app_clock_ms"] + 250, rows[REPLAY][1]["app_clock_ms"])
    submissions = {frame_key(row["paint"]): row for row in rows[FRAME] if row["submitted"] and row["render_worker"]
                   and bounds[0] <= row["app_clock_ms"] < bounds[1]}
    displays = sorted((r for r in rows[PRESENTED] if frame_key(r) in submissions), key=lambda r: r["presented_us"])
    require(len(displays) >= 2 and {tuple(r["size_px"]) for r in displays} == {(1320, 820), (1440, 900)}, "missing actual display at either size")
    native = delivery_report([r for r in rows[NATIVE] if r["window"] == window], displays, sent, resizes, outputs, dismissals)
    require("drag" in native["inputs"], "no posted drag motion could be matched to an observed window event")
    require(any(sample["during_both"] for sample in native["inputs"].values()), "no matched input overlapped both resizing and PNG output")
    require(any("gpu-preview" == site.get("id") and site["outcome"] == "rendered"
                for row in submissions.values() for site in row["paint"].get("effects", ())), "the author GPU program did not render")
    require(not args.pinyin or (native["appkit_text"]["composition"] >= 3 and native["appkit_text"]["text_commit"] >= 3),
            "the system input method did not produce both AppKit marked text and commit callbacks")
    return {"drawables": depth, "round": repeat, "captures": len(outputs), "resizes": len(resizes),
            **frame_report(rows, window, bounds, submissions, displays), **native}


def main():
    repo = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--window", type=Path, default=repo / "target/release/window")
    parser.add_argument("--octara", type=Path, default=repo / "target/debug/octara")
    parser.add_argument("--rounds", type=int, default=2)
    parser.add_argument("--pinyin", action="store_true", help="also test the enabled system Simplified Pinyin input source")
    args = parser.parse_args()
    require(sys.platform == "darwin" and args.rounds > 0, "run on a visible macOS desktop with at least one paired round")
    args.repo, args.timeout = repo, 5
    args.window, args.octara = args.window.resolve(), args.octara.resolve()
    require(args.window.is_file() and args.octara.is_file(), "build the release example window and the Octara CLI first")
    with tempfile.TemporaryDirectory(prefix="octara-native-latency-", dir="/tmp") as temporary:
        scratch = Path(temporary)
        driver = scratch / "native-input"
        subprocess.run(["swiftc", "-swift-version", "6", "-warnings-as-errors", "-module-cache-path", scratch / "swift-cache",
                        repo / "examples/native_render_input.swift", "-o", driver], check=True)
        for repeat in range(1, args.rounds + 1):
            for depth in ((3, 2) if repeat % 2 else (2, 3)):
                print(json.dumps(run(args, scratch, driver, depth, repeat)), flush=True)


if __name__ == "__main__":
    try:
        main()
    except (SmokeFailure, OSError, KeyError, ValueError, subprocess.CalledProcessError, struct.error) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)

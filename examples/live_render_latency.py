#!/usr/bin/env python3
"""Compare two and three Metal drawables on a visible 1920x1080 @ 120 Hz display.

Build the release window and the Inspector CLI and put the window on that display.
Each run replays 27 seconds of Shader Lab. Inspector clicks, scrolling, and drag moves
are timed from framework dispatch to the first Metal display of the resulting revision.
The report counts changed revisions that had no attributable display separately.
Use --scenario resize to measure window resizing without concurrent PNG captures.
Use --scenario admission to check window and capture ordering at the current display mode.
"""

import argparse
from collections import Counter, defaultdict
import ctypes
import json
from pathlib import Path
from statistics import median, quantiles
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
RESIZE = "octara.resize_trace/1"
SURFACE = "octara.surface_trace/1"
DRAIN = "octara.surface_drain/1"
ACQUIRE = "octara.acquire_trace/1"
ADMISSION = "octara.device_admission/1"
OUTPUT = "octara.capture_trace/1"
OPPORTUNITY = "octara.display_opportunity/1"
SCHEMAS = (FRAME, PRESENTED, INPUT, REPLAY, RESIZE, SURFACE, DRAIN, ACQUIRE, ADMISSION, OUTPUT, OPPORTUNITY)
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
    returned = [row for row in rows[ACQUIRE] if row["window"] == window and row["phase"] == "return"]
    acquisitions = {row["acquisition"]: row for row in returned if row["route"] == "worker" and row["status"] == "ready"}
    acquired = [acquisitions[row["acquisition_request"]] for row in submissions.values()
                if row["acquisition_request"] in acquisitions]
    acquisition_status_counts = defaultdict(Counter)
    for row in returned:
        acquisition_status_counts[row["route"]][row["status"]] += 1
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
        "acquisition_status_counts": dict(acquisition_status_counts),
        "worker_acquisition_attempt_wait": statistics(row["at_us"] - row["started_us"] for row in returned
                                                      if row["route"] == "worker" and row["status"] in ("ready", "stale", "timeout")),
        "peak_matched_pending_presents": peak,
        "empty_display_ticks": len(empty_ticks),
        "empty_ticks_used_for_display": len(displayed_targets.intersection(empty_ticks)),
    }


def surface_drains(rows):
    enqueues = {}
    starts = {}
    drains = defaultdict(list)
    for row in sorted(rows, key=lambda row: row["at_us"]):
        device = row["device_generation"]
        if row["phase"] == "enqueue":
            enqueues[device] = row["at_us"]
        elif row["phase"] == "start":
            starts[device] = {"started_us": row["at_us"]}
            if device in enqueues:
                starts[device]["enqueued_us"] = enqueues.pop(device)
        elif row["phase"] == "finish" and device in starts:
            drains[device].append({**starts.pop(device), "finished_us": row["at_us"], "ready": row["ready"]})
    return drains


RESIZE_SPANS = {
    "client_round_trip": ("client_request", "client_reply"),
    "client_to_wire_forward": ("client_request", "wire_forwarded"),
    "wire_forward_to_host_handler": ("wire_forwarded", "host_handler"),
    "host_handler_to_native_call": ("host_handler", "native_call"),
    "client_to_native_call": ("client_request", "native_call"),
    "client_to_event": ("client_request", "event"),
    "native_call_elapsed": ("native_call", "native_return"),
    "native_call_to_event": ("native_call", "event"),
    "native_return_to_event": ("native_return", "event"),
    "native_call_to_observed": ("native_call", "observed"),
    "native_return_to_observed": ("native_return", "observed"),
    "event_to_observed": ("event", "observed"),
    "observed_to_viewport": ("observed", "viewport"),
    "viewport_to_eligible": ("viewport", "eligible"),
    "observed_to_eligible": ("observed", "eligible"),
    "eligible_to_drain_enqueue": ("eligible", "drain_enqueued"),
    "drain_enqueue_to_worker_start": ("drain_enqueued", "drain_started"),
    "drain_finish_to_configure": ("drain_finished", "configure_started"),
    "request_to_configure": ("client_request", "configure_finished"),
    "configure_elapsed": ("configure_started", "configure_finished"),
    "configure_to_client_reply": ("configure_finished", "client_reply"),
    "configure_to_first_accepted_frame_start": ("configure_finished", "first_accepted_frame_started"),
    "configure_to_first_accepted_render_submit": ("configure_finished", "first_accepted_render_submitted"),
    "configure_to_first_accepted_present_call": ("configure_finished", "first_accepted_present_called"),
    "configure_to_presented_frame_drawable_request": ("configure_finished", "drawable_requested"),
    "drawable_request_to_worker_start": ("drawable_requested", "drawable_acquire_started"),
    "presented_frame_drawable_wait": ("drawable_acquire_started", "drawable_acquire_finished"),
    "drawable_return_to_host": ("drawable_acquire_finished", "drawable_handled"),
    "drawable_return_to_frame_start": ("drawable_acquire_finished", "frame_started"),
    "configure_to_presented_frame_start": ("configure_finished", "frame_started"),
    "presented_frame_start_to_render_submit": ("frame_started", "render_submitted"),
    "render_submit_to_schedule": ("render_submitted", "scheduled"),
    "configure_to_schedule": ("configure_finished", "scheduled"),
    "schedule_to_display": ("scheduled", "presented"),
    "configure_to_display": ("configure_finished", "presented"),
    "client_reply_to_display": ("client_reply", "presented"),
    "display_to_client_reply": ("presented", "client_reply"),
}


def resize_milestones(events, size, request_us, reply_us, next_request_us, configure_started_us):
    timeline = {"client_request": request_us, "client_reply": reply_us, "configure_started": configure_started_us}
    phases = {
        "request": ("native_call", reply_us),
        "request_return": ("native_return", reply_us),
        "event": ("event", next_request_us),
        "observed": ("observed", configure_started_us),
        "viewport": ("viewport", configure_started_us),
        "eligible": ("eligible", configure_started_us),
    }
    for phase, (name, end_us) in phases.items():
        event = next((row for row in events if row["phase"] == phase and row["size_px"] == size
                      and request_us <= row["at_us"] <= end_us), None)
        if event is None:
            continue
        timeline[name] = event["at_us"]
        if phase == "request":
            for field, stage in (("forwarded_us", "wire_forwarded"), ("handler_us", "host_handler")):
                at = event.get(field)
                if at is not None and request_us <= at <= timeline[name]:
                    timeline[stage] = at
    return timeline


def resize_surface(row):
    return (row["size_px"], row["surface_generation"], row["device_generation"])


def resize_report(rows, window, submissions, displays, requests, *, reply_confirms_config=True):
    # The requests and Metal timestamps share the system Core Animation clock.
    epoch_us = median(row["presented_time_s"] * 1_000_000 - row["presented_us"] for row in displays)
    configurations = [row for row in rows[SURFACE] if row["window"] == window and row["reason"] == "resize" and row["configured"]]
    events = sorted((row for row in rows.get(RESIZE, ()) if row["window"] == window), key=lambda row: row["at_us"])
    drains = surface_drains(rows.get(DRAIN, ()))
    acquisitions = {(row["window"], row["acquisition"], row["phase"]): row for row in rows.get(ACQUIRE, ()) if row["route"] == "worker"}
    samples = []
    for index, (size, sent_s, replied_s) in enumerate(requests):
        size = list(size)
        request_us = sent_s * 1_000_000 - epoch_us
        reply_us = replied_s * 1_000_000 - epoch_us
        next_request_s = requests[index + 1][1] if index + 1 < len(requests) else float("inf")
        next_request_us = next_request_s * 1_000_000 - epoch_us
        configure_by_us = min(reply_us, next_request_us) if reply_confirms_config else next_request_us
        failures = []
        sample = {"size_px": size, "request_us": request_us, "reply_us": reply_us, "failures": failures}
        samples.append(sample)
        matching = [row for row in configurations if row["size_px"] == size
                    and request_us <= row["started_us"] <= row["started_us"] + row["elapsed_us"] <= configure_by_us]
        if len(matching) != 1:
            failures.append("ambiguous_configuration" if matching else "missing_configuration")
            continue

        config = matching[0]
        device, surface = config["device_generation"], config["surface_generation"]
        configured_us = config["started_us"] + config["elapsed_us"]
        timeline = resize_milestones(events, size, request_us, reply_us, next_request_us, config["started_us"])
        timeline["configure_finished"] = configured_us
        sample.update(device_generation=device, surface_generation=surface, configure_started_us=config["started_us"],
                      timeline_us=timeline)
        if "native_call" in timeline and "native_return" in timeline:
            sample["during_native_call"] = [phase for phase in ("event", "observed", "viewport") if phase in timeline
                                            and timeline["native_call"] <= timeline[phase] <= timeline["native_return"]]
        attempts = []
        for drain in drains[device]:
            if request_us <= drain["finished_us"] <= config["started_us"]:
                if attempts and attempts[-1]["ready"]:
                    attempts.clear()
                attempts.append(drain)
        sample["drain_attempts"] = attempts
        if attempts and attempts[-1]["ready"]:
            timeline.update(drain_started=attempts[0]["started_us"], drain_finished=attempts[-1]["finished_us"])
            if "enqueued_us" in attempts[0]:
                timeline["drain_enqueued"] = attempts[0]["enqueued_us"]
            sample["drain_elapsed_us"] = sum(drain["finished_us"] - drain["started_us"] for drain in attempts)
            sample["drain_retries"] = len(attempts) - 1
        else:
            failures.append("missing_ready_drain")

        target = resize_surface(config)
        first_accepted = min((row for row in submissions.values() if row["window"] == window and resize_surface(row) == target
                              and (render_submitted := row["timeline_us"].get("render_submitted")) is not None
                              and configured_us <= render_submitted < next_request_us),
                             key=lambda row: row["timeline_us"]["render_submitted"], default=None)
        if first_accepted is not None:
            frame = first_accepted["timeline_us"]
            sample.update(first_accepted_frame=first_accepted["frame"], first_accepted_revision=first_accepted["revision"])
            timeline.update(first_accepted_frame_started=frame.get("started"), first_accepted_render_submitted=frame["render_submitted"])
            present_called = frame.get("present_called")
            if present_called is not None and present_called < next_request_us:
                timeline["first_accepted_present_called"] = present_called

        shown = min((row for row in displays if resize_surface(row) == target and configured_us <= row["scheduled_us"]
                     and sent_s <= row["presented_time_s"] < next_request_s), key=lambda row: row["presented_time_s"], default=None)
        if shown is None:
            failures.append("missing_display")
        else:
            sample.update(presented_frame=shown["frame"], presented_revision=shown["revision"], presented_us=shown["presented_us"],
                          request_to_display_us=(shown["presented_time_s"] - sent_s) * 1_000_000)
            submitted = submissions[frame_key(shown)]
            frame = submitted["timeline_us"]
            timeline.update(frame_started=frame.get("started"), render_submitted=frame.get("render_submitted"),
                            scheduled=shown["scheduled_us"], presented=shown["presented_us"])
            acquisition = submitted.get("acquisition_request")
            requested = acquisitions.get((shown["window"], acquisition, "start"))
            returned = acquisitions.get((shown["window"], acquisition, "return"))
            if requested is not None:
                timeline["drawable_requested"] = requested["at_us"]
                sample["acquisition_request"] = acquisition
            if returned is not None and returned["status"] == "ready":
                timeline.update(drawable_acquire_started=returned["started_us"], drawable_acquire_finished=returned["at_us"],
                                drawable_handled=returned.get("handled_us"))
                sample["acquisition_request"] = acquisition
        for span, (start, finish) in RESIZE_SPANS.items():
            began, ended = timeline.get(start), timeline.get(finish)
            if began is not None and ended is not None and began <= ended:
                sample[f"{span}_us"] = ended - began
    metrics = ("request_to_display", "drain_elapsed", *RESIZE_SPANS)
    return {"requests": len(samples), "failures": dict(Counter(failure for sample in samples for failure in sample["failures"])),
            "drain_retries": sum(sample.get("drain_retries", 0) for sample in samples), "samples": samples,
            **{metric: statistics(sample[f"{metric}_us"] for sample in samples if f"{metric}_us" in sample) for metric in metrics}}


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
    resizes = []
    if scenario in ("capture", "resize"):
        media_time = ctypes.CDLL("/System/Library/Frameworks/QuartzCore.framework/QuartzCore").CACurrentMediaTime
        media_time.argtypes = []
        media_time.restype = ctypes.c_double

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
        if scenario in ("capture", "resize"):
            for index, (second, width, height) in enumerate(((0.9, 1320, 820), (1.8, 1440, 900))):
                at(start + second)
                sent_s = media_time()
                reply = request("size", main, width=width, height=height)
                replied_s = media_time()
                detail = reply["live"]["resize"]
                require(detail["matched"] and detail["synchronized"] and detail["physical"] == [width, height],
                        f"native resize did not converge at 1×: {detail}")
                resizes.append((detail["physical"], sent_s, replied_s))
                if scenario == "capture":
                    capture(main, cycle, index, (width, height))

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
    return windows, probes, resizes


def compare(args, temporary, scenario, depth, repeat):
    name = f"{scenario}-{depth}-r{repeat}"
    print(f"Measuring {name}", file=sys.stderr, flush=True)
    env = {"WGPU_BACKEND": "metal", "OCTARA_ASYNC_METAL": "1", "OCTARA_RENDER_WORKER": "1", "OCTARA_TRACE_INPUT": "1",
           "OCTARA_TRACE_PRESENTED": "1", "OCTARA_TRACE_FRAME_CLOCK": "0", "OCTARA_TRACE_GPU": "0",
           "OCTARA_METAL_FRAME_LATENCY": str(depth - 1), "OCTARA_METAL_ACQUIRE_DELAY_MS": "0"}
    with LiveWindow(name, args, temporary, temporary, ["gpu_lab", "replay", "120", "stress"], env=env) as app:
        with app.wire() as stream:
            windows, probes, resizes = exercise(app, stream, scenario)
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
        expected_sizes = {(800, 620)} if index else {(1440, 900), (1320, 820)} if resizes else {(1440, 900)}
        actual_sizes = {tuple(row["size_px"]) for row in displays}
        require(actual_sizes <= expected_sizes if scenario == "resize" else actual_sizes == expected_sizes,
                f"{name}: unexpected or missing display sizes")
        report = {"window": window, **frame_report(rows, window, bounds, submissions, displays)}
        if not index:
            report["inputs"] = input_report([row for row in rows[INPUT] if row["window"] == window], displays, probes)
            if resizes:
                report["resize"] = resize_report(rows, next(iter(submissions.values()))["window"], submissions, displays, resizes)
        result["windows"].append(report)
    return result


def main():
    repo = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--window", type=Path, default=repo / "target/release/window")
    parser.add_argument("--octara", type=Path, default=repo / "target/debug/octara")
    parser.add_argument("--scenario", choices=("single", "dual", "resize", "capture", "admission"), action="append",
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

#!/usr/bin/env python3
"""Summarize opt-in Shell frame traces offline; never drive a window or infer display FPS.

Use snapshot.time_ms to select a steady workload interval in the app's clock.
Durations are CPU-side elapsed time around the reported operations, including waits.
The input sample belongs to the oldest coalesced dispatch, not every individual key.
"""

import argparse
import json
import math
from pathlib import Path


def distribution(values):
    values = sorted(value for value in values
                    if isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value))
    if not values:
        return {"n": 0, "p50": None, "p95": None, "p99": None, "max": None}
    quantile = lambda p: values[max(0, math.ceil(len(values) * p) - 1)]
    return {"n": len(values), "p50": quantile(.50), "p95": quantile(.95),
            "p99": quantile(.99), "max": values[-1]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log", type=Path)
    parser.add_argument("--output", required=True, type=Path, help="new JSON report path")
    parser.add_argument("--from-ms", type=float, default=0)
    parser.add_argument("--until-ms", type=float)
    parser.add_argument("--window", help="exact window string from the frame record")
    parser.add_argument("--require-visible", action="store_true", help="also exclude unknown visibility/occlusion")
    args = parser.parse_args()
    if args.from_ms < 0 or not math.isfinite(args.from_ms):
        parser.error("--from-ms must be finite and nonnegative")
    if args.until_ms is not None and (not math.isfinite(args.until_ms) or args.until_ms <= args.from_ms):
        parser.error("--until-ms must be finite and greater than --from-ms")
    frames, excluded = [], {"outside_interval_or_window": 0, "background": 0, "unknown_visibility": 0}
    trace_records, update_records, updates = 0, 0, []
    before, after = {}, {}
    with args.log.open(encoding="utf-8") as source:
        for number, line in enumerate(source, 1):
            if "octara.frame_trace/1" not in line and "octara.update_trace/1" not in line:
                continue
            try:
                frame = json.loads(line)
                if frame.get("schema") not in ("octara.frame_trace/1", "octara.update_trace/1"):
                    raise ValueError("unexpected trace schema")
                clock = frame["app_clock_ms"]
                if not isinstance(clock, (int, float)) or not math.isfinite(clock):
                    raise ValueError("invalid app clock")
            except (ValueError, TypeError, KeyError) as error:
                raise SystemExit(f"invalid frame trace at line {number}: {error}") from error
            if frame["schema"] == "octara.update_trace/1":
                update_records += 1
                if clock >= args.from_ms and (args.until_ms is None or clock <= args.until_ms):
                    updates.append(frame)
                elif clock < args.from_ms:
                    before[frame["phase"]] = frame
                else:
                    after.setdefault(frame["phase"], frame)
                continue
            trace_records += 1
            if clock < args.from_ms or (args.until_ms is not None and clock > args.until_ms) \
                    or (args.window is not None and frame["window"] != args.window):
                excluded["outside_interval_or_window"] += 1
                continue
            if frame.get("visible") is False or frame.get("occluded") is True:
                excluded["background"] += 1
                continue
            if args.require_visible and (frame.get("visible") is not True or frame.get("occluded") is not False):
                excluded["unknown_visibility"] += 1
                continue
            frames.append(frame)
    if not frames:
        raise SystemExit(f"no matching frames ({trace_records} trace records; excluded {excluded})")
    stages = sorted({stage for frame in frames for stage in frame.get("cpu_us", {})})
    inputs = [frame["input"] for frame in frames if frame.get("input") is not None]
    fields = ["handler_us", "queue_us", "wait_redraw_us", "to_present_return_us"]
    kinds = sorted({sample["kind"] for sample in inputs})
    slowest = sorted(frames, key=lambda frame: frame["cpu_us"].get("total", 0), reverse=True)[:10]
    report = {
        "schema": "octara.chat.stream_trace_summary/1", "source": str(args.log.resolve()),
        "interval_app_ms": [args.from_ms, args.until_ms], "unit": "microseconds",
        "trace_records": trace_records, "frames": len(frames), "excluded": excluded,
        "update_records": update_records, "updates_in_interval": len(updates),
        "app_update_elapsed": {
            phase: distribution(sample.get("elapsed_us") for sample in updates if sample["phase"] == phase)
            for phase in sorted({sample["phase"] for sample in updates})
        },
        "app_update_boundary": "App-wide advance_to/poll includes task, timer, reactive and layout work. Selected only by app clock, without window/visibility attribution; do not add to input.handler_us or frame totals.",
        "adjacent_updates": {"before_interval": before, "after_interval": after},
        "adjacent_update_boundary": "Nearest records outside each boundary are retained for review, excluded from quantiles. Logical app clocks are labels, not wall-clock start/end timestamps; elapsed_us cannot establish precise overlap with the interval.",
        "require_visible": args.require_visible, "surface_failures": sum(not f["surface_acquired"] for f in frames),
        "unknown_visibility_frames": sum(f.get("visible") is None or f.get("occluded") is None for f in frames),
        "cpu_side_elapsed": {stage: distribution(frame["cpu_us"].get(stage) for frame in frames) for stage in stages},
        "oldest_input": {field: distribution(sample.get(field) for sample in inputs) for field in fields},
        "input_frames": len(inputs), "dispatches": sum(sample.get("dispatches", 0) for sample in inputs),
        "coalesced": sum(sample.get("coalesced", 0) for sample in inputs),
        "native_compositions": sum(sample.get("native_compositions", 0) for sample in inputs),
        "native_commits": sum(sample.get("native_commits", 0) for sample in inputs),
        "by_oldest_input_kind": {
            kind: {field: distribution(sample.get(field) for sample in inputs if sample["kind"] == kind) for field in fields}
            for kind in kinds
        },
        "slowest_frames": slowest,
        "boundary": "CPU-side elapsed (including blocking waits); input starts at Shell dispatch or owned native text callback. Coalesced samples describe the oldest input until present returns. Trace-on JSON/log writing can affect later samples. No OS hardware queue, GPU execution, physical presentation or display FPS measurement.",
        "gpu_execution_us": None, "physical_display_us": None,
    }
    with args.output.open("x", encoding="utf-8") as output:
        json.dump(report, output, ensure_ascii=False, indent=2)
        output.write("\n")
    print(json.dumps({"report": str(args.output), "frames": len(frames), "input_frames": len(inputs),
                      "excluded": excluded}, ensure_ascii=False))


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Measure one Octara window from Game Performance's xctrace XML exports."""

import argparse
from bisect import bisect_right
from collections import defaultdict
import json
import math
from pathlib import Path
from statistics import median, quantiles
from xml.etree import ElementTree as ET


def rows(directory, schema):
    for node in ET.parse(directory / f"{schema}.xml").getroot().findall("node"):
        columns = [col.findtext("mnemonic") for col in node.findall("schema/col")]
        ids = {element.get("id"): element for element in node.iter() if "id" in element.attrib}

        def resolve(element, ids=ids):
            if element is not None and "ref" in element.attrib:
                return ids[element.get("ref")]
            return element

        for row in node.findall("row"):
            yield dict(zip(columns, map(resolve, row), strict=True)), resolve


def value(element):
    return (element.text or element.get("fmt") or "").strip()


def process_id(element, resolve):
    pid = resolve(element.find("pid"))
    return int(value(pid)) if pid is not None else None


def app_frames(label, resolve, pid):
    # A desktop compositor swap can hold several clients. The top-level Surface ID identifies
    # the compositor buffer. The narrative's process, frame and surface identify the client.
    tokens = list(map(resolve, label))
    for i, token in enumerate(tokens[:-4]):
        if token.tag == "process" and process_id(token, resolve) == pid:
            frame_marker, frame, surface_marker, surface = tokens[i + 1 : i + 5]
            if value(frame_marker) == ":Frame" and value(surface_marker) == ":Surface":
                yield int(value(frame)), int(value(surface))


def intervals(samples):
    return [(later - earlier) / 1e6 for earlier, later in zip(samples, samples[1:])]


def report(args):
    def in_bounds(ns):
        ms = ns / 1e6
        return (args.start_ms is None or args.start_ms <= ms) and (args.end_ms is None or ms < args.end_ms)

    all_frames = {}
    display_rows = app_rows = 0
    for row, resolve in rows(args.exports, "displayed-surfaces-interval"):
        if value(row["display-name"]) != args.display:
            continue
        at = int(value(row["start"]))
        bounded = in_bounds(at)
        display_rows += bounded
        owners = list(app_frames(row["event-label"], resolve, args.pid))
        app_rows += bounded and bool(owners)
        for frame, surface in owners:
            previous = all_frames.get(frame)
            if previous is not None and previous[1] != surface:
                raise ValueError(f"frame {frame} has several client surfaces for PID {args.pid}; trace a dedicated window")
            if previous is None or at < previous[0]:
                all_frames[frame] = (at, surface)

    # Filter after first-swap selection so a repeated frame cannot reappear at a bounds edge.
    frames = sorted((at, surface) for at, surface in all_frames.values() if in_bounds(at))
    if len(frames) < 2:
        raise ValueError(f"need two attributed frames for PID {args.pid} on {args.display}; found {len(frames)} in {display_rows} display rows")
    if len({at for at, _ in frames}) != len(frames):
        raise ValueError(f"several frames for PID {args.pid} share a display swap; trace a dedicated window")

    vsync = set()
    for row, _ in rows(args.exports, "display-vsyncs-interval"):
        if value(row["display-name"]) == args.display and value(row["event"]) == "VSYNC":
            at = int(value(row["timestamp"]))
            if in_bounds(at):
                vsync.add(at)
    if len(vsync) < 2:
        raise ValueError(f"need two observed display refreshes on {args.display}; found {len(vsync)}")
    refresh_ms = median(intervals(sorted(vsync)))

    requests = defaultdict(set)
    for row, resolve in rows(args.exports, "ca-client-present-request"):
        if process_id(row["process"], resolve) == args.pid:
            requests[int(value(row["surface-id"]))].add(int(value(row["timestamp"])))
    sorted_requests = {surface: sorted(times) for surface, times in requests.items()}
    matched = set()
    for at, surface in frames:
        candidates = sorted_requests.get(surface, [])
        index = bisect_right(candidates, at)
        if index:
            matched.add((surface, candidates[index - 1]))

    frame_times = [at for at, _ in frames]
    gaps = intervals(frame_times)
    percentiles = quantiles(gaps, n=100, method="inclusive") if len(gaps) > 1 else gaps * 99
    return {
        "pid": args.pid,
        "display": args.display,
        "bounds_ms": [args.start_ms, args.end_ms],
        "observed_span_ms": [at / 1e6 for at in (frame_times[0], frame_times[-1])],
        "coverage": {
            "display_rows": display_rows,
            "pid_attributed_rows": app_rows,
            "distinct_app_frames": len(frames),
            "client_present_requests_in_bounds": sum(in_bounds(t) for times in requests.values() for t in times),
            "distinct_prior_requests_on_attributed_surfaces": len(matched),
        },
        "display_refresh": {"vsync_samples": len(vsync), "median_interval_ms": round(refresh_ms, 4), "estimated_hz": round(1000 / refresh_ms, 3)},
        "app_display_intervals": {
            "count": len(gaps),
            "observed_hz": round(len(gaps) * 1e9 / (frame_times[-1] - frame_times[0]), 3),
            "p50_ms": round(median(gaps), 4),
            "p95_ms": round(percentiles[94], 4),
            "p99_ms": round(percentiles[98], 4),
            "max_ms": round(max(gaps), 4),
            "over_1_5_refresh": sum(gap > 1.5 * refresh_ms for gap in gaps),
        },
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("exports", type=Path, help="directory with displayed-surfaces-interval.xml, display-vsyncs-interval.xml and ca-client-present-request.xml")
    parser.add_argument("--pid", type=int, required=True, help="the Octara process with one measured window")
    parser.add_argument("--display", required=True, help="exact display name from the trace")
    parser.add_argument("--start-ms", type=float, help="include swaps at and after this trace-relative time")
    parser.add_argument("--end-ms", type=float, help="include swaps before this trace-relative time")
    args = parser.parse_args()
    if args.pid <= 0:
        parser.error("--pid must be positive")
    if any(t is not None and (not math.isfinite(t) or t < 0) for t in (args.start_ms, args.end_ms)):
        parser.error("time bounds must be finite and nonnegative")
    if args.start_ms is not None and args.end_ms is not None and args.start_ms >= args.end_ms:
        parser.error("--end-ms must follow --start-ms")
    try:
        print(json.dumps(report(args), indent=2))
    except (ET.ParseError, KeyError, OSError, ValueError) as error:
        parser.error(str(error))


if __name__ == "__main__":
    main()

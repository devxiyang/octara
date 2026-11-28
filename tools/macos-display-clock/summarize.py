#!/usr/bin/env python3
"""Summarize the probe CSV files in a results directory or a tar.gz archive."""

import argparse
import csv
import io
import statistics
import tarfile
from pathlib import Path


def load_runs(path):
    if path.is_dir():
        for file in sorted(path.glob("*.csv")):
            with file.open(newline="") as stream:
                yield file.stem, list(csv.DictReader(stream))
    else:
        with tarfile.open(path, "r:gz") as archive:
            for member in sorted(archive.getmembers(), key=lambda member: member.name):
                if member.isfile() and member.name.endswith(".csv"):
                    with io.TextIOWrapper(archive.extractfile(member), newline="") as stream:
                        yield Path(member.name).stem, list(csv.DictReader(stream))


def cadence(times):
    if len(times) < 2:
        return "— / —"
    gaps = [second - first for first, second in zip(times, times[1:])]
    rate = (len(times) - 1) / (times[-1] - times[0])
    return f"{rate:.2f} / {statistics.median(gaps) * 1000:.3f}"


def summarize(rows, stage):
    start = float(stage["callback"])
    ticks = [
        row for row in rows
        if row["stage"] == stage["stage"] and row["kind"] == "tick"
        and start + 0.75 <= float(row["callback"]) < start + 3.90
    ]
    if any(row["display"] != stage["display"] for row in ticks):
        raise ValueError(f"window left the expected display during {stage['stage']}")
    serials = {row["sequence"] for row in ticks}
    times = [
        float(row["presented"]) for row in rows
        if row["kind"] == "present" and row["sequence"] in serials
    ]
    positive = sorted(time for time in times if time > 0)
    callbacks = sorted(float(row["callback"]) for row in ticks)
    return (
        cadence(callbacks), cadence(positive),
        sum(row["acquisition"] != "drawable" for row in ticks),
        times.count(0), len(ticks) - len(times),
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("results", type=Path)
    args = parser.parse_args()

    print("| Run | Stage | Callback Hz / median ms | Presented Hz / median ms | "
          "No drawable | Zero presents | No present handler |")
    print("| --- | --- | ---: | ---: | ---: | ---: | ---: |")
    for name, rows in load_runs(args.results):
        for stage in (row for row in rows if row["kind"] == "stage"):
            stats = summarize(rows, stage)
            print("| " + " | ".join(map(str, (name, stage["stage"], *stats))) + " |")


if __name__ == "__main__":
    main()

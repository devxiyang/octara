#!/usr/bin/env python3
"""Check production workspace edges, including optional and target-specific dependencies."""

import json
from pathlib import Path
import subprocess
import sys


# Explicit edges make a new package or an upward dependency a reviewable decision.
# Dev dependencies can host integration tests without entering the shipped library graph.
ALLOWED = {
    "octara-primitives": set(),
    "octara-macros": set(),
    "octara": {"octara-primitives", "octara-macros"},
    "octara-render": {"octara-primitives"},
    "octara-icons": {"octara-primitives"},
    "octara-fonts": {"octara"},
    "octara-kit": {"octara", "octara-primitives", "octara-icons"},
    "octara-shell": {"octara", "octara-primitives", "octara-render"},
    "octara-cli": {"octara", "octara-primitives"},
    "octara-chat": {"octara", "octara-primitives", "octara-icons", "octara-kit", "octara-shell"},
    "octara-examples": {
        "octara", "octara-primitives", "octara-fonts", "octara-icons",
        "octara-kit", "octara-render", "octara-shell",
    },
}


def violations(metadata):
    members = set(metadata["workspace_members"])
    packages = [p for p in metadata["packages"] if p["id"] in members]
    names = {p["name"] for p in packages}
    errors = []
    for package in packages:
        name = package["name"]
        if name not in ALLOWED:
            errors.append(f"{name}: declare its layer in tools/check_layers.py")
            continue
        for dependency in package["dependencies"]:
            target = dependency["name"]
            if dependency["kind"] != "dev" and target in names and target not in ALLOWED[name]:
                errors.append(f"forbidden production dependency: {name} -> {target}")
    return errors


def main():
    root = Path(__file__).resolve().parent.parent
    result = subprocess.run(
        ["cargo", "metadata", "--offline", "--no-deps", "--format-version", "1"],
        cwd=root, capture_output=True, text=True,
    )
    if result.returncode:
        sys.stderr.write(result.stderr)
        return result.returncode
    errors = violations(json.loads(result.stdout))
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print("Workspace layers: all production dependencies point to permitted lower packages.")
    return 0


if __name__ == "__main__":
    sys.exit(main())

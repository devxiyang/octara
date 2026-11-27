#!/usr/bin/env python3
"""Save the v2 focused editor snapshot while driving the input method manually."""

import argparse
from datetime import datetime, timezone
import json
from pathlib import Path

from live_inspect import request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--socket", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    result = request(args.socket, {"op": "observe"})
    if not result["ok"]:
        raise RuntimeError(result.get("message", str(result)))
    result["observed_at"] = datetime.now(timezone.utc).isoformat()
    with args.output.open("x", encoding="utf-8") as output:
        output.write(json.dumps(result, ensure_ascii=False, indent=2) + "\n")


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Run a built Octara fixture on a booted iOS simulator."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
from uuid import uuid4


def main():
    scenes = {
        "panel": ("panel", "Octara Panel", "panel_scroll"),
        "controlled": ("ime", "Octara Controlled IME", "ime"),
        "album": ("album", "Octara Album Back", "album_back"),
    }
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--device", required=True, help="UDID of the booted simulator")
    parser.add_argument("--scene", choices=scenes, default="panel", help="fixture to launch (default: panel)")
    parser.add_argument("--binary", type=Path, required=True, help="fixture executable built for aarch64-apple-ios-sim")
    parser.add_argument("--output", type=Path, required=True, help="new directory for the app, socket, receipt and logs")
    args = parser.parse_args()
    if not args.binary.is_file():
        parser.error(f"executable was not found: {args.binary}")
    fixture, name, executable_name = scenes[args.scene]
    output = args.output.resolve()
    endpoint = output / f"{fixture}.sock"
    if len(os.fsencode(endpoint)) >= 104:
        parser.error("choose a shorter output path for the Unix socket")
    output.mkdir()

    app = output / f"{name}.app"
    app.mkdir()
    executable = app / executable_name
    shutil.copy2(args.binary, executable)
    digest = hashlib.sha256()
    with executable.open("rb") as stream:
        for block in iter(lambda: stream.read(1_048_576), b""):
            digest.update(block)
    build = digest.hexdigest()
    bundle = f"dev.octara.{fixture}.{uuid4().hex}"
    with (app / "Info.plist").open("wb") as stream:
        plistlib.dump({
            "CFBundleExecutable": executable.name,
            "CFBundleIdentifier": bundle,
            "CFBundleName": name,
            "CFBundlePackageType": "APPL",
            "CFBundleSupportedPlatforms": ["iPhoneSimulator"],
            "CFBundleShortVersionString": "1.0",
            "CFBundleVersion": "1",
            "LSRequiresIPhoneOS": True,
            "UIDeviceFamily": [1, 2],
            "UILaunchScreen": {},
            "UIApplicationSceneManifest": {
                "UIApplicationSupportsMultipleScenes": False,
                "UISceneConfigurations": {
                    "UIWindowSceneSessionRoleApplication": [{
                        "UISceneConfigurationName": "Octara",
                        "UISceneClassName": "UIWindowScene",
                        "UISceneDelegateClassName": "OctaraWindowSceneDelegate",
                    }],
                },
            },
        }, stream)
    receipt = output / "receipt.json"
    receipt.write_text(json.dumps({"build": build, "sources": None}) + "\n")

    simctl = ["xcrun", "simctl"]
    env = {
        **os.environ,
        "SIMCTL_CHILD_OCTARA_INSPECT": str(endpoint),
        "SIMCTL_CHILD_OCTARA_INSPECT_BUILD": build,
        "SIMCTL_CHILD_OCTARA_TRACE_INPUT": "1",
    }
    if args.scene == "controlled":
        env["SIMCTL_CHILD_OCTARA_IME_SCENE"] = args.scene
    trace = output / "console.log"
    run = {
        "device": args.device,
        "bundle": bundle,
        "socket": str(endpoint),
        "receipt": str(receipt),
        "trace": str(trace),
    }
    subprocess.run([*simctl, "install", args.device, app], check=True)
    try:
        report = json.dumps(run, indent=2)
        (output / "run.json").write_text(report + "\n")
        print(report, flush=True)
        with trace.open("w") as log:
            subprocess.run(
                [*simctl, "launch", "--console", args.device, bundle],
                env=env, stdout=log, stderr=subprocess.STDOUT, check=True,
            )
    finally:
        subprocess.run([*simctl, "uninstall", args.device, bundle], check=False)
        if endpoint.is_socket():
            endpoint.unlink()


if __name__ == "__main__":
    main()

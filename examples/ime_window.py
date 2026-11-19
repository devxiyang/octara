#!/usr/bin/env python3
"""Prepare a macOS app bundle for manual OS-input/IME validation; never launch or control it.

Build first: cargo build -p octara-examples --bin ime -p octara-cli --bin octara
The output directory owns one copied executable, Info.plist, and manifest.json. No input source,
keyboard, pointer, process, user preference or Accessibility permission is changed by this tool.
"""

import argparse
import json
import os
from pathlib import Path
import plistlib
import shutil
import tempfile


REPO = Path(__file__).resolve().parent.parent
BUNDLE_ID = "dev.octara.ime-validation"
SCENES = {
    "input": ("Octara IME.app", BUNDLE_ID, "Octara IME Validation", ["ime-single", "ime-multi", "ime-password"]),
    "rename": ("Octara Rename.app", "dev.octara.rename-validation", "Octara Rename Validation", ["rename-editor"]),
    "controlled": ("Octara Controlled IME.app", "dev.octara.controlled-ime-validation", "Octara Controlled IME Validation",
                   ["ime-controlled", "ime-controlled-other"]),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--binary", type=Path, default=REPO / "target/debug/ime", help="built IME fixture executable")
    parser.add_argument("--output", type=Path, help="empty directory; default: a new /tmp/octara-ime-* directory")
    parser.add_argument("--socket", type=Path, help="unused absolute Inspector socket; default: OUTPUT/ime.sock")
    parser.add_argument("--scene", choices=SCENES, default="input", help="validation scene to package")
    args = parser.parse_args()
    binary = args.binary.resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        parser.error(f"build the executable first: {binary}")

    output = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="octara-ime-", dir="/tmp"))
    if args.output:
        output.mkdir(parents=True, exist_ok=True)
        if any(output.iterdir()):
            parser.error(f"output must be empty: {output}")
    socket = args.socket if args.socket else output / "ime.sock"
    if not socket.is_absolute() or len(os.fsencode(socket)) > 100:
        parser.error("socket must be an absolute Unix socket path of at most 100 bytes")
    if socket.exists() or socket.is_symlink():
        parser.error(f"socket path is already in use: {socket}")
    if not socket.parent.is_dir():
        parser.error(f"socket directory does not exist: {socket.parent}")

    name, bundle_id, title, fields = SCENES[args.scene]
    bundle = output / name
    contents = bundle / "Contents"
    executable = contents / "MacOS/ime"
    executable.parent.mkdir(parents=True)
    shutil.copy2(binary, executable)
    info = {
        "CFBundleExecutable": "ime",
        "CFBundleIdentifier": bundle_id,
        "CFBundleName": title,
        "CFBundleDisplayName": title,
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": "1.0",
        "CFBundleVersion": "1",
        "NSHighResolutionCapable": True,
        "LSEnvironment": {"OCTARA_INSPECT": str(socket), "OCTARA_IME_SCENE": args.scene},
    }
    with (contents / "Info.plist").open("wb") as stream:
        plistlib.dump(info, stream)
    manifest = {
        "bundle": str(bundle),
        "bundle_identifier": bundle_id,
        "executable": str(executable),
        "source_executable": str(binary),
        "socket": str(socket),
        "window_title": title,
        "scene": args.scene,
        "input_sources": {"latin": "com.apple.keylayout.ABC", "pinyin": "com.apple.inputmethod.SCIM.ITABC"},
        "fields": fields,
        "single_seed": "A😀Z",
    }
    if args.scene == "input":
        manifest["multi_seed"] = "alpha\nbeta"
    elif args.scene == "rename":
        manifest["native_control"] = str(socket.with_suffix(".native-control.json"))
    else:
        manifest["other_seed"] = "B😀Y"
    encoded = json.dumps(manifest, ensure_ascii=False, indent=2) + "\n"
    (output / "manifest.json").write_text(encoded, encoding="utf-8")
    print(encoded, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

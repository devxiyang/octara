#!/usr/bin/env python3
"""Package the signed review executable as a macOS application."""

import argparse
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile

def bundle(binary, destination, *, identity="-", version="0.1.0", build_number="1"):
    destination = destination.absolute()
    if destination.suffix != ".app" or destination.exists() or destination.is_symlink():
        raise ValueError("the destination must be a new .app bundle")
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".octara-app-", dir=destination.parent) as stage:
        app = Path(stage) / destination.name
        executable = app / "Contents" / "MacOS" / "signed_review"
        executable.parent.mkdir(parents=True)
        shutil.copyfile(binary, executable)
        executable.chmod(0o755)
        info = executable.parent.parent / "Info.plist"
        info.write_bytes(plistlib.dumps({
            "CFBundleDisplayName": "Octara Release Review",
            "CFBundleName": "Octara Release Review",
            "CFBundleExecutable": executable.name,
            "CFBundleIdentifier": "dev.octara.signed-review",
            "CFBundlePackageType": "APPL",
            "CFBundleInfoDictionaryVersion": "6.0",
            "CFBundleShortVersionString": version,
            "CFBundleVersion": build_number,
        }))
        subprocess.run(["/usr/bin/plutil", "-lint", info], check=True, timeout=15)
        timestamp = "--timestamp=none" if identity == "-" else "--timestamp"
        subprocess.run(
            ["/usr/bin/codesign", "--force", "--sign", identity, "--options", "runtime", timestamp, app],
            check=True, timeout=120,
        )
        verify(app)
        app.rename(destination)
    return destination


def verify(app):
    subprocess.run(["/usr/bin/codesign", "--verify", "--deep", "--strict", app], check=True, timeout=30)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("app", type=Path)
    parser.add_argument("--identity", default="-", help="Developer ID Application identity, or '-' for a local ad hoc signature")
    parser.add_argument("--version", default="0.1.0")
    parser.add_argument("--build-number", default="1")
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("this packaging command requires macOS")
    print(bundle(args.binary, args.app, identity=args.identity, version=args.version, build_number=args.build_number))


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Check canonical workspace members and their permitted dependency edges."""

import json
import re
from pathlib import Path
import subprocess
import sys


# Explicit edges make a new package or an upward dependency a reviewable decision.
# Dev dependencies can host integration tests without entering the shipped library graph.
# Applications and examples may opt into optional modules.
ALLOWED = {
    "octara-model": set(),
    "octara-macros": set(),
    "octara-fonts": set(),
    "octara-release": set(),
    "octara-icons": {"octara-model"},
    "octara-svg": {"octara-model"},
    "octara-tree": {"octara-model"},
    "octara-runtime": {"octara-model", "octara-tree"},
    "octara": {"octara-model", "octara-tree", "octara-runtime", "octara-macros"},
    "octara-headless": {"octara"},
    "octara-kit": {"octara", "octara-headless", "octara-icons"},
    "octara-markdown": {"octara", "octara-headless", "octara-icons"},
    "octara-pipeline": {"octara-model", "octara-tree", "octara-runtime"},
    "octara-render": {"octara-model"},
    "octara-render-metal": {"octara-model", "octara-render"},
    "octara-gpu": {"octara-model", "octara-render", "octara-render-metal"},
    "octara-host-native": {
        "octara", "octara-model", "octara-pipeline", "octara-render", "octara-gpu",
    },
    "octara-host-web": {"octara", "octara-model", "octara-render"},
    "octara-inspect": {"octara", "octara-model", "octara-pipeline", "octara-render"},
    "octara-cli": {"octara", "octara-model", "octara-inspect", "octara-release"},
    "octara-chat": {"octara", "octara-icons", "octara-kit", "octara-markdown"},
    "octara-chat-desktop": {"octara", "octara-chat", "octara-host-native"},
    "octara-examples": {
        "octara", "octara-fonts", "octara-icons",
        "octara-kit", "octara-markdown", "octara-pipeline", "octara-render", "octara-inspect",
    },
    "octara-examples-desktop": {
        "octara", "octara-examples", "octara-host-native", "octara-icons",
        "octara-inspect", "octara-kit", "octara-pipeline", "octara-render", "octara-release",
    },
}

APP_DIRS = {
    "octara-chat": "apps/chat",
    "octara-chat-desktop": "apps/chat/desktop",
    "octara-examples": "examples",
    "octara-examples-desktop": "examples/desktop",
}

MODULE_ALLOWED = {"octara", "octara-icons", "octara-kit"}
MODULE_CROSS_PLATFORM_DEPS = {"serde", "serde_json", "url"}


AUTHORING_PACKAGES = ("octara-headless", "octara-kit", "octara-markdown", *APP_DIRS)
INTERNAL_IMPORT = re.compile(
    r"\boctara_(?:model|runtime|tree)\b"
    r"|\boctara\s*::\s*(?:model|runtime|tree)\b"
    r"|\boctara\s*::\s*\{[^;]*?\b(?:model|runtime|tree)\s*(?=::|as\b|[,}])"
)


def authoring_import_violations(root, directories, authoring_packages):
    for package in authoring_packages:
        for source in sorted((directories[package] / "src").rglob("*.rs")):
            content = source.read_text()
            for match in INTERNAL_IMPORT.finditer(content):
                line = content.count("\n", 0, match.start()) + 1
                yield f"{source.relative_to(root)}:{line}: use the Octara authoring facade"


def violations(metadata):
    root = Path(metadata["workspace_root"]).resolve()
    modules_root = root / "modules"
    members = set(metadata["workspace_members"])
    packages = [p for p in metadata["packages"] if p["id"] in members]
    modules = {}
    for package in packages:
        name = package["name"]
        directory = Path(package["manifest_path"]).resolve().parent
        if name not in ALLOWED and directory.parent == modules_root:
            modules[name] = directory

    directories = {name: root / APP_DIRS.get(name, f"crates/{name}") for name in ALLOWED}
    directories.update(modules)
    names = {p["name"] for p in packages}
    errors = [f"missing workspace member: {name}" for name in sorted(ALLOWED.keys() - names)]
    for package in packages:
        name = package["name"]
        if name not in directories:
            errors.append(f"unexpected workspace member: {name}")
            continue
        expected_manifest = directories[name] / "Cargo.toml"
        if Path(package["manifest_path"]).resolve() != expected_manifest:
            errors.append(f"workspace member {name}: expected {expected_manifest.relative_to(root)}")

        is_module = name in modules
        is_app = name in APP_DIRS
        allowed = MODULE_ALLOWED if is_module else ALLOWED[name]
        if is_app:
            allowed = allowed | modules.keys()

        # Cargo's declared dependencies include optional and target-specific edges.
        for dependency in package["dependencies"]:
            target = dependency["name"]
            path = Path(dependency["path"]) if dependency.get("path") else None
            resolved_path = path.resolve() if path else None
            if dependency["kind"] == "dev":
                if not is_app and not is_module and (
                    target in modules or any(p is not None and p.is_relative_to(modules_root) for p in (path, resolved_path))
                ):
                    errors.append(f"forbidden development dependency: {name} -> {target}")
                continue
            if name == "octara-headless" and target != "octara":
                errors.append(f"forbidden production dependency: {name} -> {target}")
                continue
            in_repo = path is not None and (
                path.is_relative_to(root) or resolved_path.is_relative_to(root)
            )
            if target not in directories and not in_repo:
                if is_module and dependency["target"] is None and target not in MODULE_CROSS_PLATFORM_DEPS:
                    errors.append(f"production dependency {name} -> {target}: expected a target-specific dependency")
                continue
            if target not in allowed:
                errors.append(f"forbidden production dependency: {name} -> {target}")
            elif resolved_path != directories[target]:
                expected = directories[target].relative_to(root)
                errors.append(f"production dependency {name} -> {target}: expected path {expected}")
    errors.extend(authoring_import_violations(root, directories, (*AUTHORING_PACKAGES, *sorted(modules))))
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

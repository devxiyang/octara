#!/usr/bin/env python3
"""Verify generation settings in a real Chat window and on loopback HTTP.

Build the model_selection example and octara CLI first. Uses Inspector framework input,
native window resize and GPU captures; does not claim OS keyboard or IME verification.
All app data and evidence are isolated under the printed output directory.
"""

import argparse
import json
import os
from pathlib import Path
import sys
import tempfile

sys.dont_write_bytecode = True
from live_model_selection import (HTTPFixture, LABELS, ModelWindow, REPO, SmokeFailure,
                                  find, has, last_answer, require, save)
from live_inspect import resize


def edit(app, field, value):
    app.guarded("tap", id=field)
    app.guarded("key", chord="super+a")
    app.guarded("type", id=field, text=value)


def configure(app, temperature, tokens, model):
    app.guarded("tap", id="settings-open")
    edit(app, "settings-temperature", temperature)
    edit(app, "settings-tokens", tokens)
    app.guarded("tap", id="settings-model")
    app.guarded("type", id="settings-model-search", text=LABELS[model])
    app.guarded("key", chord="enter")
    require(find(app.inspect(), "settings-model").get("value") == LABELS[model], "nested model selection failed")
    app.guarded("tap", id="settings-save")
    require(not has(app.inspect(), "settings-save"), "valid settings were not saved")


def preferences(store):
    return json.loads((store / "settings.json").read_text())


def run(args, output, sockets, store, server, summary):
    first = output / "first"
    first.mkdir()
    with ModelWindow(args, first, sockets, store, server) as app:
        app.guarded("tap", id="settings-open")
        edit(app, "settings-temperature", "oops")
        invalid = app.observation("01-invalid")
        require("number" in find(invalid, "settings-error").get("name", ""), "invalid input has no actionable explanation")
        require("disabled" in find(invalid, "settings-save").get("states", []), "invalid draft permits Save")
        app.gpu("01-invalid.png")
        app.guarded("tap", id="settings-cancel")
        require(not (store / "settings.json").exists(), "cancel wrote preferences")

        configure(app, "0.4", "512", "local-b")
        app.say("idle")
        require(server.request(1)["generation"] == {"temperature": 0.4, "max_completion_tokens": 512}, "first request settings differ")
        configure(app, "0.8", "1024", "local-a")
        app.say("queued")
        app.guarded("tap", id="settings-open")
        app.guarded("tap", id="settings-reset")
        defaults = app.observation("02-defaults")
        require("invalid" not in find(defaults, "settings-temperature").get("states", []), "blank override is marked invalid")
        app.gpu("02-defaults.png")
        app.guarded("tap", id="settings-save")
        require(len(server.status()["requests"]) == 1, "settings changes interrupted the active request or sent queued work early")
        server.release(1)
        server.request(2, ended="completed")
        complete = app.until(lambda s: not has(s, "stop"), "queued completion")
        require(server.request(2)["generation"] == {"temperature": 0.8, "max_completion_tokens": 1024}, "queued parameters were overwritten")
        app.guarded("tap", id=f"regenerate-{last_answer(complete)}")
        require(server.request(3, ended="completed")["generation"] == {}, "regeneration did not use current defaults")
        app.until(lambda s: not has(s, "stop"), "regenerated completion")
        configure(app, "0.6", "2048", "local-b")
        saved = preferences(store)
        app.guarded("tap", id="settings-open")
        app.clean()
        app.gpu("03-saved-desktop.png")
        resize(app, 390, 720)
        app.clean()
        app.observation("04-compact")
        app.gpu("04-compact.png")
        app.guarded("tap", id="settings-reset")
        app.guarded("tap", id="settings-cancel")
        require(preferences(store) == saved, "cancelling Restore defaults changed saved settings")
        save(first / "routes.json", app.routes)

    second = output / "restart"
    second.mkdir()
    with ModelWindow(args, second, sockets, store, server) as app:
        require(find(app.inspect(), "model").get("value") == LABELS["local-b"], "restart lost the saved model ID")
        app.guarded("tap", id="settings-open")
        restored = app.observation("05-restored")
        require(find(restored, "settings-temperature").get("value") == "0.6", "restart lost temperature")
        require(find(restored, "settings-tokens").get("value") == "2048", "restart lost token limit")
        app.clean()
        app.gpu("05-restored.png")
        app.guarded("tap", id="settings-reset")
        app.guarded("tap", id="settings-save")
        require(preferences(store) == {"model_id": "local-a", "generation": {"temperature": None, "max_completion_tokens": None}},
                "Restore defaults did not persist optional overrides as empty")
        save(second / "routes.json", app.routes)
    facts = server.status()
    require([facts["requests"][str(n)]["model"] for n in (1, 2, 3)] == ["wire-b", "wire-a", "wire-a"], "request model snapshots differ")
    require(len(facts["requests"]) == 3, "unexpected HTTP requests")
    save(output / "http-final.json", facts)
    summary.update(ok=True, cancel=True, invalid_input=True, queued_snapshot=True, regeneration_defaults=True,
                   restart=True, restore_defaults=True, native_resize=True, gpu_captures=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--window", type=Path, default=REPO / "target/debug/examples/model_selection")
    parser.add_argument("--cli", type=Path, default=REPO / "target/debug/octara")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--timeout", type=float, default=15)
    args = parser.parse_args()
    require(0 < args.timeout <= 30, "timeout must be within (0, 30] seconds")
    args.repo, args.octara, args.window = REPO, args.cli.resolve(), args.window.resolve()
    for path in (args.octara, args.window):
        require(path.is_file() and os.access(path, os.X_OK), f"build the executable first: {path}")
    output = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="octara-settings-", dir="/tmp"))
    output.mkdir(parents=True, exist_ok=True)
    require(not any(output.iterdir()), f"evidence directory must be empty: {output}")
    store = output / "store"
    store.mkdir()
    sockets = Path(tempfile.mkdtemp(prefix="settings-sock-", dir="/tmp"))
    summary = {"ok": False, "input_route": "framework", "os_input_verified": False}
    print(f"Settings evidence: {output}", flush=True)
    try:
        with HTTPFixture(args, output) as server:
            run(args, output, sockets, store, server, summary)
    except BaseException as error:
        summary["error"] = str(error)
        raise
    finally:
        save(output / "summary.json", summary)
        try:
            sockets.rmdir()
        except OSError:
            pass
    print("PASS real Chat settings: validation, cancel, model/parameter snapshots, resize, restart, defaults and GPU captures")


if __name__ == "__main__":
    try:
        main()
    except (SmokeFailure, OSError, ValueError, KeyError, StopIteration) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)

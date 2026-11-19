#!/usr/bin/env python3
"""Verify UI primitives in actual desktop windows through the live Inspector.

Build first, then run (the runner never invokes Cargo):
    python3 examples/live_primitives.py --window target/release/window --octara target/release/octara
    python3 examples/live_primitives.py --display --output /tmp/primitives-evidence
    python3 examples/live_primitives.py --scene controls --window target/release/window --octara target/release/octara

Every input uses a freshly observed Inspector identity target, except deliberate stale-target
refusals. Input is framework/semantic input, with no OS automation or native IME verification.
Captures identify window GPU pixels or window-server pixels and any display fallback.
Each window has a 160-request limit; waits, processes, sockets and evidence are bounded.
"""

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import socket
import struct
import sys
import tempfile
import zlib

sys.dont_write_bytecode = True

from live_forms import EDITOR_FIELDS, FormsWindow, save_json, token
from live_inspect import SmokeFailure, find, nodes, require


EDITOR = "suggest-editor"
DRAFT_TAB = "editor-tabs-tab-draft"
PREVIEW_TAB = "editor-tabs-tab-preview"
DRAFT_PANEL = "editor-tabs-panel-draft"
PREVIEW_PANEL = "editor-tabs-panel-preview"
REVIEW_TEXT = "请审阅当前改动。"
SUMMARY_TEXT = "请概括当前内容。"


def present(snapshot, identifier):
    return next((node for node in nodes(snapshot) if node.get("id") == identifier), None)


def selected(snapshot, identifier):
    return "selected" in find(snapshot, identifier).get("states", [])


def tab_link(snapshot, tab, panel):
    control, page = find(snapshot, tab), find(snapshot, panel)
    require(control.get("controls") == panel, f"{tab} has no relation to its active panel")
    require(page.get("role") == "tab_panel" and page.get("labelled_by") == tab and page.get("name") == control.get("name"),
            f"{panel} did not take its name from its tab")


def editor_state(snapshot):
    require(snapshot.get("focus") == EDITOR and snapshot.get("text_input"), "suggestion editor lost keyboard ownership")
    return {key: snapshot["text_input"].get(key) for key in EDITOR_FIELDS}


def pending(snapshot, option):
    require(snapshot.get("focus") == EDITOR, "suggestions took focus away from the editor")
    owner = find(snapshot, EDITOR)
    require(owner.get("expanded") is bool(option), "editor expanded state differs from the popup")
    expected = f"{EDITOR}-option-{option}" if option else None
    require(owner.get("active_descendant") == expected, f"active suggestion differs: {owner}")
    if option:
        require(find(snapshot, f"{EDITOR}-list").get("role") == "list_box", "popup omitted listbox semantics")
        require(selected(snapshot, expected), "active suggestion omitted selected semantics")
        require("disabled" in find(snapshot, f"{EDITOR}-option-deploy").get("states", []), "disabled suggestion became available")
    else:
        require(present(snapshot, f"{EDITOR}-list") is None, "closed popup remained visible")


def submission_count(snapshot, count):
    require(find(snapshot, "interaction-submissions").get("name") == f"提交次数：{count}", "suggestion choice unexpectedly submitted")


class PrimitivesWindow(FormsWindow):
    def action(self, name, op, **fields):
        if op not in ("set_value", "drag"):
            return super().action(name, op, **fields)
        before = self.observe(f"{name}-before")
        owner = find(before, fields["id"])
        route = "framework" if op == "drag" else "semantic"
        reply = self.request(name, {"op": op, "window": self.window, "target": token(owner), **fields}, route, "delivered")
        return self.observe(f"{name}-after"), reply

    def reject(self, name, op, code, *, target=None, delivery="refused", **fields):
        """Observe afresh and send one expected refusal without retrying delivered input."""
        before = self.observe(f"{name}-before")
        owner = find(before, fields["id"]) if fields.get("id") else before
        request = {"op": op, "window": self.window, "target": target or token(owner), **fields}
        self.steps += 1
        require(self.steps <= 160, "live verification exceeded its request budget")
        with self.wire() as stream:
            reply = self.wire_request(stream, request)
        require(self.transcript.tell() <= 16 * 1024 * 1024, "CLI evidence exceeded 16 MiB")
        route = "framework" if op == "tap" else "semantic"
        require(reply.get("ok") is False and reply.get("code") == code, f"{name}: wrong refusal: {reply}")
        require(reply.get("delivery") == delivery and reply.get("live", {}).get("delivery") == delivery,
                f"{name}: the failed operation reported unexpected delivery: {reply}")
        require(reply.get("live", {}).get("route") == route, f"{name}: unexpected route: {reply}")
        self.routes.append({"name": name, "op": op, "route": route, "delivery": delivery, "target": request["target"], "code": code})
        save_json(self.output / f"{self.fixture}-{name}.json", {"request": request, "reply": reply})
        return self.observe(f"{name}-after"), reply


def native_size(app):
    snapshot, reply = app.action("native-390x640", "size", width=390, height=640)
    resize = reply["live"].get("resize", {})
    require(resize.get("requested") == [390, 640] and resize.get("matched") is True
            and resize.get("synchronized") is True, f"native resize did not synchronize: {reply}")
    actual, physical, scale = resize.get("actual", []), resize.get("physical", []), resize.get("scale", 0)
    require(len(actual) == len(physical) == 2 and scale > 0
            and all(abs(a - b) < 0.2 for a, b in zip(actual, snapshot["viewport"]))
            and all(abs(a - b) < 1 for a, b in zip(actual, [390, 640]))
            and all(abs(a * scale - p) < 0.2 for a, p in zip(actual, physical)), "native/framework/physical sizes disagree")
    _, settled = app.action("settle-narrow-layout", "settle", limit_ms=5000)
    require(settled.get("live", {}).get("settled") is True and not settled["live"].get("pending"), "narrow layout did not settle")
    return snapshot


def replace_editor(app, text):
    app.action("select-editor", "key", chord="mod+a")
    return app.action("replace-editor", "type", text=text)[0]


def interaction(args, output, sockets):
    with PrimitivesWindow("interaction", args, output, sockets, ["showcase", "interaction"]) as app:
        first = app.observe("initial", evidence=True)
        require(present(first, "editor-dialog") is None, "dialog opened before its model requested it")
        submission_count(first, 0)
        app.picture("initial-gpu")

        opened, _ = app.action("open-dialog", "tap", id="interaction-open")
        dialog = find(opened, "editor-dialog")
        require(dialog.get("role") == "dialog" and dialog.get("name") == "编辑消息", "dialog lost its accessible name or role")
        editor_state(opened)
        old_editor = token(find(opened, EDITOR))
        old_dialog = token(dialog)
        require(selected(opened, DRAFT_TAB) and not selected(opened, PREVIEW_TAB), "initial tab selection differs")
        tab_link(opened, DRAFT_TAB, DRAFT_PANEL)
        require(find(opened, PREVIEW_TAB).get("controls") is None, "hidden Preview was published as a relation target")
        app.observe("dialog-open", evidence=True)
        app.picture("dialog-gpu")
        inert, _ = app.reject("outside-is-inert", "tap", "STEP_FAILED", id="interaction-open")
        require(token(find(inert, "editor-dialog")) == old_dialog and inert.get("focus") == EDITOR,
                "an inert outside action moved focus or reopened the dialog")

        suggestions, _ = app.action("open-suggestions", "type", text="/")
        pending(suggestions, "review")
        rows = {key: token(find(suggestions, f"{EDITOR}-option-{key}")) for key in ("review", "deploy", "summary")}
        highlighted, _ = app.action("select-query", "key", chord="mod+a")
        before_update = editor_state(highlighted)
        updated, _ = app.action("reorder-and-rename", "invoke", command="interaction.options")
        require(editor_state(updated) == before_update, "option metadata changed editor text, selection or composition")
        require(token(find(updated, EDITOR)) == old_editor, "option updates replaced the editor")
        require({key: token(find(updated, f"{EDITOR}-option-{key}")) for key in rows} == rows, "option updates replaced stable rows")
        require(find(updated, f"{EDITOR}-option-review").get("name") == "/review · 审阅", "updated label did not reach the retained row")
        pending(updated, "review")
        moved, _ = app.action("skip-disabled-suggestion", "key", chord="up")
        pending(moved, "summary")
        chosen, _ = app.action("enter-chooses-suggestion", "key", chord="enter")
        pending(chosen, None)
        require(find(chosen, EDITOR).get("value") == SUMMARY_TEXT, "Enter chose the wrong stable suggestion ID")
        require(find(chosen, "interaction-chosen").get("name") == "已选择：summary", "choice status differs")
        submission_count(chosen, 0)
        app.action("select-retained-editor", "key", chord="mod+a")
        retained = app.observe("before-tab-navigation")
        retained_selection = editor_state(retained)["selection"]

        app.action("focus-draft-tab", "focus", id=DRAFT_TAB)
        moved, _ = app.action("tab-arrow-is-manual", "key", chord="right")
        require(moved.get("focus") == PREVIEW_TAB and selected(moved, DRAFT_TAB) and not selected(moved, PREVIEW_TAB),
                "tab arrow activated the panel")
        require(token(find(moved, EDITOR)) == old_editor, "manual tab focus replaced the editor")
        wrapped, _ = app.action("tabs-skip-disabled", "key", chord="right")
        require(wrapped.get("focus") == DRAFT_TAB and selected(wrapped, DRAFT_TAB), "tab navigation failed to skip disabled Advanced")
        app.action("return-preview-focus", "key", chord="right")
        preview, _ = app.action("activate-preview", "key", chord="enter")
        require(selected(preview, PREVIEW_TAB) and not selected(preview, DRAFT_TAB), "Enter did not select Preview")
        require(present(preview, EDITOR) is None and find(preview, DRAFT_TAB).get("controls") is None,
                "hidden retained draft or its relation remained exposed")
        tab_link(preview, PREVIEW_TAB, PREVIEW_PANEL)
        require(find(preview, "editor-preview-text").get("name") == f"预览：{SUMMARY_TEXT}", "preview lost the draft")
        app.action("focus-draft-without-activation", "key", chord="left")
        draft, _ = app.action("activate-draft", "key", chord="space")
        require(selected(draft, DRAFT_TAB) and token(find(draft, EDITOR)) == old_editor, "panel switching replaced the retained editor")
        tab_link(draft, DRAFT_TAB, DRAFT_PANEL)
        restored, _ = app.action("restore-editor-focus", "focus", id=EDITOR)
        require(editor_state(restored)["selection"] == retained_selection, "retained panel lost the editor selection")

        pointer_list = replace_editor(app, "/")
        pending(pointer_list, "summary")
        pending_review, _ = app.action("pointer-list-disabled-skip", "key", chord="down")
        pending(pending_review, "review")
        picked, _ = app.action("pointer-chooses-owned-popup", "tap", id=f"{EDITOR}-option-review")
        pending(picked, None)
        require(find(picked, EDITOR).get("value") == REVIEW_TEXT and token(find(picked, EDITOR)) == old_editor,
                "pointer choice replaced the editor or chose the wrong ID")
        require(find(picked, "interaction-chosen").get("name") == "已选择：review", "pointer choice status differs")
        submission_count(picked, 0)

        app.action("tab-enters-segments", "key", chord="tab")
        segment, _ = app.action("segment-keyboard-choice", "key", chord="right")
        require(segment.get("focus") == "editor-format-option-full" and find(segment, "editor-format-option-full").get("checked") is True,
                "compact radio choice did not follow keyboard focus")
        require("selected" not in find(segment, "editor-format-option-full").get("states", []), "segments acquired tab semantics")
        app.action("return-to-editor", "focus", id=EDITOR)
        escaped_list = replace_editor(app, "/")
        pending(escaped_list, "summary")
        escaped, _ = app.action("escape-only-popup", "key", chord="escape")
        pending(escaped, None)
        require(present(escaped, "editor-dialog"), "suggestion Escape also dismissed the dialog")
        closed, _ = app.action("escape-dialog", "key", chord="escape")
        require(present(closed, "editor-dialog") is None and closed.get("focus") == "interaction-open", "dialog did not restore its opener")
        reopened, _ = app.action("reopen-dialog", "tap", id="interaction-open")
        require(token(find(reopened, EDITOR)) != old_editor and token(find(reopened, "editor-dialog")) != old_dialog,
                "reopening reused retired allocations")
        require(find(reopened, EDITOR).get("value") == "/", "the application-owned draft did not survive dialog unmount")
        refused, _ = app.reject("retired-editor-refused", "focus", "STALE_TARGET", id=EDITOR, target=old_editor)
        require(refused.get("focus") == EDITOR and find(refused, EDITOR).get("value") == "/", "stale input changed the new editor")
        app.observe("reopened-identities", evidence=True)

        native_size(app)
        narrow = app.observe("narrow-dialog", evidence=True)
        require(find(narrow, "editor-dialog")["bounds"][2] <= 390, "dialog did not fit the narrow window")
        app.picture("narrow-gpu")
        if args.display:
            app.picture("narrow-display", display=True)
        app.action("clean", "expect", clean=True)
        return {"checks": ["dialog-focus-and-inert-outside", "dialog-restore-and-retired-target-refusal",
                           "tabs-manual-activation-and-disabled-skip", "retained-panel-editor-selection",
                           "popup-stable-ids-reorder", "popup-keyboard-and-pointer-owned-overlay",
                           "suggestion-enter-without-submit", "segmented-radio-semantics", "native-390x640", "clean"],
                "requests": app.steps, "routes": app.routes, "captures": app.captures}


def numeric_state(snapshot, value, raw=None):
    slider, progress = find(snapshot, "controls-slider"), find(snapshot, "controls-progress")
    require(slider.get("role") == "slider" and slider.get("min_numeric_value") == 0
            and slider.get("max_numeric_value") == 1 and slider.get("numeric_value_step") == 0.05,
            f"slider omitted its numeric range: {slider}")
    require(abs(slider.get("numeric_value", -10) - value) < 1e-9, f"slider value differs from {value}: {slider}")
    require(abs(progress.get("numeric_value", -1000) - value * 100) < 1e-7, f"progress differs from the shared value: {progress}")
    require(progress.get("max_numeric_value") == 100 and "set_value" not in progress.get("actions", []), "progress became editable")
    if raw is not None:
        require(find(snapshot, "controls-number").get("value") == raw, f"numeric draft differs from {raw!r}")


def replace_number(app, text):
    app.action("select-number", "key", chord="mod+a")
    return app.action("edit-number-draft", "type", text=text)[0]


def controls(args, output, sockets):
    with PrimitivesWindow("controls", args, output, sockets, ["showcase", "controls"]) as app:
        first = app.observe("initial", evidence=True)
        numeric_state(first, 0.35, "0.35")
        require(find(first, "controls-number").get("required") is True, "number field omitted required semantics")
        identities = {identifier: token(find(first, identifier)) for identifier in ("controls-slider", "controls-number", "controls-progress")}
        app.picture("initial-gpu")
        app.action("focus-number", "focus", id="controls-number")
        invalid = replace_number(app, "-")
        require(find(invalid, "controls-number").get("invalid") is True, "incomplete numeric draft was accepted")
        require(find(invalid, "controls-number").get("numeric_value") is None, "invalid draft pretended to be numeric")
        require("disabled" in find(invalid, "controls-commit").get("states", []), "invalid draft enabled submit")
        refused_submit, _ = app.action("invalid-enter-keeps-draft", "key", chord="enter")
        numeric_state(refused_submit, 0.35, "-")
        require(find(refused_submit, "controls-status").get("name") == "尚未提交。", "invalid Enter committed a numeric draft")
        raw = replace_number(app, "0.2600")
        numeric_state(raw, 0.35, "0.2600")
        committed, _ = app.action("commit-numeric-draft", "key", chord="enter")
        numeric_state(committed, 0.25, "0.25")
        require(find(committed, "controls-status").get("name") == "已提交：0.25", "complete numeric draft did not normalize on submission")
        semantic, _ = app.action("semantic-slider-snaps", "set_value", id="controls-slider", value=0.76)
        numeric_state(semantic, 0.75, "0.75")
        require({identifier: token(find(semantic, identifier)) for identifier in identities} == identities,
                "numeric model changes replaced retained controls")
        app.action("focus-slider", "focus", id="controls-slider")
        for chord, expected in (("home", 0.0), ("right", 0.05), ("pageup", 0.55), ("pagedown", 0.05), ("end", 1.0), ("left", 0.95)):
            moved, _ = app.action(f"slider-{chord}", "key", chord=chord)
            numeric_state(moved, expected)
        centered, _ = app.action("pointer-track-center", "tap", id="controls-slider")
        numeric_state(centered, 0.5)
        half, _ = app.action("prepare-centered-drag", "set_value", id="controls-slider", value=0.5)
        width = find(half, "controls-slider")["bounds"][2]
        require(width >= 80, "slider has insufficient travel for the live drag")
        dragged, _ = app.action("pointer-drag-slider", "drag", id="controls-slider", dx=width / 4, dy=0)
        dragged_value = find(dragged, "controls-slider").get("numeric_value", -1)
        require(0.65 <= dragged_value <= 0.85 and abs(dragged_value / 0.05 - round(dragged_value / 0.05)) < 1e-7,
                f"drag did not move and snap near three quarters: {dragged_value}")
        numeric_state(dragged, dragged_value)
        app.observe("numeric-input-and-drag", evidence=True)

        app.action("focus-menu-trigger", "focus", id="controls-menu")
        menu, _ = app.action("open-command-menu", "key", chord="down")
        require(menu.get("focus") == "controls-menu-action-controls.reset", "Down did not open on the first menu action")
        require(find(menu, "controls-menu-menu").get("role") == "menu", "command popup omitted menu semantics")
        boundary, _ = app.action("menu-last-item", "key", chord="end")
        require(boundary.get("focus") == "controls-menu-submenu-3", "End did not focus the submenu trigger")
        app.action("open-boundary-submenu", "key", chord="right")
        maximum, _ = app.action("submenu-skips-unavailable", "key", chord="end")
        require(maximum.get("focus") == "controls-menu-submenu-3-action-controls.maximum", "submenu End did not skip the disabled action")
        chosen, _ = app.action("choose-maximum-command", "key", chord="enter")
        numeric_state(chosen, 1.0, "1")
        require(present(chosen, "controls-menu-menu") is None and chosen.get("focus") == "controls-menu", "command did not close the menu and restore its trigger")
        app.action("reopen-command-menu", "key", chord="down")
        toggle, _ = app.action("focus-disable-command", "key", chord="down")
        require(toggle.get("focus") == "controls-menu-action-controls.toggle_disabled", "Down did not reach the disable action")
        disabled, _ = app.action("disable-numeric-controls", "key", chord="enter")
        require(all("disabled" in find(disabled, identifier).get("states", []) for identifier in ("controls-number", "controls-slider")),
                "disable command did not disable both numeric inputs")
        unchanged, _ = app.reject("disabled-slider-rejects-numeric-handler", "set_value", "STEP_FAILED",
                                  id="controls-slider", value=0.2, delivery="refused")
        numeric_state(unchanged, 1.0, "1")
        app.picture("disabled-gpu")
        app.action("reenable-controls", "invoke", command="controls.toggle_disabled")
        app.action("open-menu-for-pointer-command", "tap", id="controls-menu")
        reset, _ = app.action("pointer-reset-command", "tap", id="controls-menu-action-controls.reset")
        numeric_state(reset, 0.35, "0.35")
        require(find(reset, "controls-status").get("name") == "已重置。", "pointer menu command did not run")

        unknown, _ = app.action("indeterminate-progress", "tap", id="controls-progress-mode")
        progress = find(unknown, "controls-progress")
        require(progress.get("numeric_value") is None and progress.get("max_numeric_value") == 100,
                "indeterminate progress pretended to be zero")
        require("set_value" not in progress.get("actions", []), "indeterminate progress became editable")
        app.observe("indeterminate-progress", evidence=True)
        # Continuous indeterminate animation is deliberately not settled or captured.
        determinate, _ = app.action("restore-determinate-progress", "tap", id="controls-progress-mode")
        numeric_state(determinate, 0.35, "0.35")
        native_size(app)
        app.action("narrow-command-menu", "tap", id="controls-menu")
        app.observe("narrow-menu", evidence=True)
        app.picture("narrow-gpu")
        if args.display:
            app.picture("narrow-display", display=True)
        app.action("dismiss-narrow-menu", "key", chord="escape")
        app.action("clean", "expect", clean=True)
        return {"checks": ["numeric-raw-draft-and-invalid-submit", "numeric-explicit-normalization", "semantic-set-value-snapping",
                           "slider-keyboard-and-bounds", "slider-track-tap-and-pointer-drag", "menu-submenu-and-disabled-skip",
                           "menu-command-and-focus-restoration", "disabled-numeric-input", "indeterminate-progress-not-zero",
                           "native-390x640", "clean"],
                "requests": app.steps, "routes": app.routes, "captures": app.captures}


def artifact_budget(output):
    files = [path for path in output.rglob("*") if path.is_file()]
    require(len(files) <= 48, "evidence exceeded 48 files")
    require(all(path.stat().st_size <= 16 * 1024 * 1024 for path in files), "an evidence artifact exceeded 16 MiB")
    require(sum(path.stat().st_size for path in files) <= 128 * 1024 * 1024, "evidence exceeded 128 MiB")


def main():
    repo = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--window", type=Path, default=repo / "target/debug/window")
    parser.add_argument("--octara", type=Path, default=repo / "target/debug/octara")
    parser.add_argument("--display", action="store_true", help="request a window-server screenshot and record any GPU fallback")
    parser.add_argument("--scene", choices=("all", "interaction", "controls"), default="all", help="scene to verify; default: both scenes in separate windows")
    parser.add_argument("--output", type=Path, help="new or empty evidence directory; default: a new /tmp directory")
    parser.add_argument("--timeout", type=float, default=30)
    args = parser.parse_args()
    require(os.name == "posix" and hasattr(socket, "AF_UNIX"), "live verification requires Unix sockets and an interactive desktop")
    require(0 < args.timeout <= 30, "--timeout must be greater than zero and at most 30 seconds")
    args.repo = repo
    for name in ("window", "octara"):
        path = getattr(args, name).resolve()
        require(path.is_file() and os.access(path, os.X_OK), f"build the executable first: {path}")
        setattr(args, name, path)
    output = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="octara-primitives-evidence-", dir="/tmp"))
    output.mkdir(parents=True, exist_ok=True)
    require(not any(output.iterdir()), f"--output must be empty: {output}")
    sockets = Path(tempfile.mkdtemp(prefix="octara-primitives-", dir="/tmp"))
    summary = {"started": datetime.now(timezone.utc).isoformat(), "passed": False,
               "scope": "actual desktop windows driven by live Inspector framework and semantic requests",
               "input_route": "framework-and-semantic", "os_input_verified": False, "native_ime_verified": False,
               "scene": args.scene, "request_limit_per_window": 160,
               "output": str(output), "window": str(args.window), "octara": str(args.octara)}
    print(f"Live primitives evidence: {output}", flush=True)
    try:
        for scene in ("interaction", "controls") if args.scene == "all" else (args.scene,):
            summary[scene] = {"interaction": interaction, "controls": controls}[scene](args, output, sockets)
        artifact_budget(output)
        summary["passed"] = True
    except Exception as error:
        summary["failure"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        summary["finished"] = datetime.now(timezone.utc).isoformat()
        save_json(output / "summary.json", summary)
        if not any(sockets.iterdir()):
            sockets.rmdir()
    print(f"PASS live primitives ({args.scene}): framework/semantic input, native resize and GPU capture; OS input unverified.")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (SmokeFailure, OSError, ValueError, KeyError, struct.error, zlib.error) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)

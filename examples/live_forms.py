#!/usr/bin/env python3
"""Verify fields and radios in a real desktop window through the live Inspector.

Build first, then run:
    python3 examples/live_forms.py --window target/debug/window --octara target/debug/octara
    # Optional normal, offline Chat and window-server capture report:
    python3 examples/live_forms.py --chat target/debug/octara-chat --display --output /tmp/forms-evidence

Input uses framework events and semantic operations, not OS automation or a native IME.
Each action observes fresh node/window identity targets. Inputs are never retried.
Only this runner's processes and unique sockets are closed; evidence is retained.
"""

import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import json
import os
from pathlib import Path
from types import SimpleNamespace
import socket
import struct
import sys
import tempfile
import zlib

sys.dont_write_bytecode = True

from live_inspect import LiveWindow, SmokeFailure, find, png_pixels, require


SHORT_HELP = "支持中文和表情。"
FULL_HELP = "支持中文和表情；名称用于显示，保存时去掉两端空格。"
RADIOS = [f"reply-style-option-{style}" for style in ("fast", "balanced", "deep")]
EDITOR_FIELDS = ("text", "selection", "selection_utf16", "reversed", "composition",
                 "composition_selection", "composition_selection_utf16", "display_text",
                 "display_selection", "display_selection_utf16", "marked", "marked_utf16")


def token(value):
    target = value.get("target")
    require(isinstance(target, str) and target, "Inspector omitted an identity target")
    return target


def save_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def ordered_radios(node):
    if node.get("role") == "radio_button":
        yield node["id"]
    for child in node.get("children", []):
        yield from ordered_radios(child)


class FormsWindow(LiveWindow):
    def __init__(self, fixture, args, output, sockets, window_args):
        super().__init__(fixture, args, output, sockets, window_args=window_args)
        self.window = None
        self.steps = 0
        self.routes = []
        self.captures = []

    def __exit__(self, *error):
        super().__exit__(*error)
        if self.endpoint.is_socket():
            self.endpoint.unlink()  # The socket directory was created exclusively for this run.

    def request(self, name, request, route="query", delivery=None):
        self.steps += 1
        require(self.steps <= 160, "live verification exceeded its request budget")
        reply = self.requests(request)[0]
        require(self.transcript.tell() <= 16 * 1024 * 1024, "CLI evidence exceeded 16 MiB")
        live = reply.get("live", {})
        require(live.get("route") == route, f"{name}: expected route {route!r}: {reply}")
        if delivery:
            require(reply.get("delivery") == delivery and live.get("delivery") == delivery,
                    f"{name}: input did not report {delivery}: {reply}")
        self.routes.append({"name": name, "op": request["op"], "route": live["route"],
                            "delivery": live.get("delivery"), "target": request.get("target")})
        return reply

    def observe(self, name, evidence=False):
        request = {"op": "inspect"}
        if self.window is not None:
            request["window"] = self.window
        reply = self.request(name, request)
        snapshot = reply["snapshot"]
        require(snapshot.get("schema") == "octara.inspect/2", "wrong Inspector schema")
        require(reply["live"].get("target_validation") == "identity", "live host lacks identity validation")
        token(snapshot)
        if self.window is None:
            self.window = snapshot["window"]
        require(snapshot["window"] == self.window, "Inspector routed to a different window")
        if evidence:
            save_json(self.output / f"{self.fixture}-{name}.json", reply)
        return snapshot

    def action(self, name, op, **fields):
        before = self.observe(f"{name}-before")
        if op in ("tap", "focus", "type") and fields.get("id"):
            owner = find(before, fields["id"])
        elif op in ("key", "type"):
            require(before.get("focus"), f"{name}: no focused target")
            owner = find(before, before["focus"])
        else:
            owner = before
        route = ("framework" if op in ("tap", "key", "type") else "native-window" if op == "size" else
                 "wait" if op == "settle" else "query" if op in ("issues", "expect") else "semantic")
        reply = self.request(name, {"op": op, "window": self.window, "target": token(owner), **fields},
                             route, None if route in ("query", "wait") else "delivered")
        return self.observe(f"{name}-after"), reply

    def picture(self, name, display=False):
        snapshot = self.observe(f"{name}-before")
        path = self.output / f"{self.fixture}-{name}.png"
        report = self.request(name, {"op": "capture", "window": self.window, "target": token(snapshot),
                                    "path": str(path), **({} if display else {"scale": 1}),
                                    "pixels": "display" if display else "painted",
                                    "settle_ms": 5000}, "capture")["capture"]
        require(path.stat().st_size <= 16 * 1024 * 1024, "capture exceeded 16 MiB")
        # Display PNG encoders can use other formats; the header still gives exact dimensions.
        header = path.read_bytes()[:24]
        require(header[:8] == b"\x89PNG\r\n\x1a\n" and len(header) == 24, "capture is not a PNG")
        dimensions = struct.unpack(">II", header[16:24])
        require(dimensions == (report["width"], report["height"]), "capture/report dimensions disagree")
        require(report.get("settled") is True and not report.get("pending"), f"capture did not settle: {report}")
        if display:
            actual_display = report.get("pixels") == "display" and report.get("renderer") == "display"
            require(actual_display or (report.get("pixels") == "painted" and report.get("renderer") == "gpu"
                                       and report.get("fallback")), f"unreported display fallback: {report}")
        else:
            require(report.get("renderer") == "gpu" and report.get("pixels") == "painted", f"not a window GPU capture: {report}")
            width, height, digest = png_pixels(path)
            require((width, height) == tuple(round(v) for v in snapshot["viewport"]), "GPU capture differs from viewport")
            report = {**report, "pixel_sha256": digest}
        self.captures.append(report)
        save_json(self.output / f"{self.fixture}-{name}-report.json", report)
        return report


def field_state(snapshot, value, error=None, help_text=FULL_HELP):
    control, save = find(snapshot, "profile-name"), find(snapshot, "forms-save")
    require(control.get("role") == "text_input" and control.get("name") == "显示名称", "field lost its label/role")
    label = find(snapshot, "profile-name-label")
    require(control.get("labelled_by") == label["id"] and control.get("name") == label.get("name"),
            "field name did not come from its visible label")
    require(control.get("value") == value, f"field value differs: {control}")
    require(control.get("required") is True and control.get("invalid") is bool(error), "field required/invalid semantics differ")
    require(control.get("description") == help_text + (f"\n{error}" if error else ""), "field description differs")
    require(("disabled" in save.get("states", [])) is bool(error), "save disabled state differs from validation")
    require(save.get("description") == (error or "保存当前名称和回复风格。"), "save omitted its current disabled reason")


def radio_state(snapshot, style):
    identifier = f"reply-style-option-{style}"
    require(snapshot.get("focus") == identifier, f"radio focus differs: {snapshot.get('focus')}")
    require([id for id in RADIOS if find(snapshot, id).get("checked")] == [identifier], "radio selection differs from focus")
    require("disabled" in find(snapshot, RADIOS[2]).get("states", []), "deep radio became enabled")


def replace_name(app, text):
    app.action("focus-name", "focus", id="profile-name")
    app.action("select-name", "key", chord="mod+a")
    return app.action("replace-name", "type", text=text)[0] if text else app.action("clear-name", "key", chord="backspace")[0]


def forms(args, output, sockets):
    with FormsWindow("forms", args, output, sockets, ["showcase", "forms"]) as app:
        first = app.observe("initial", evidence=True)
        field_state(first, "小明", help_text=SHORT_HELP)
        require(find(first, "reply-style")["role"] == "radio_group", "missing radio group semantics")
        require(list(ordered_radios(find(first, "reply-style"))) == RADIOS, "unexpected initial radio order")
        require(find(first, RADIOS[1]).get("checked") is True, "balanced was not selected initially")
        identities = {id: token(find(first, id)) for id in ["profile-name", *RADIOS]}
        app.picture("initial-gpu")
        app.action("focus-editor", "focus", id="profile-name")
        selected, _ = app.action("select-editor", "key", chord="mod+a")
        original_editor = {key: selected["text_input"].get(key) for key in EDITOR_FIELDS}
        require(original_editor["selection"] == [0, len("小明".encode())], "did not select the committed Chinese name")
        for command in ("forms.help", "forms.reorder"):
            changed, _ = app.action(command, "invoke", command=command)
            require(changed.get("focus") == "profile-name", f"{command}: editor lost focus")
            require({key: changed["text_input"].get(key) for key in EDITOR_FIELDS} == original_editor,
                    f"{command}: editor text, selection or composition changed")
            require({id: token(find(changed, id)) for id in identities} == identities, f"{command}: retained controls were replaced")
        field_state(changed, "小明")
        require(list(ordered_radios(find(changed, "reply-style"))) == list(reversed(RADIOS)), "radios did not reorder")
        require(find(changed, RADIOS[1]).get("name") == "均衡 · 推荐" and find(changed, RADIOS[1]).get("checked") is True,
                "radio rename/reorder changed the stable selection")
        app.observe("retained-editor", evidence=True)
        entered, _ = app.action("tab-enters-radio", "key", chord="tab")
        radio_state(entered, "balanced")
        left, _ = app.action("tab-leaves-radio", "key", chord="tab")
        require(left.get("focus") == "forms-help", "radio group took more than one Tab stop")
        returned, _ = app.action("backtab-radio", "key", chord="shift+tab")
        radio_state(returned, "balanced")
        for chord, style in (("right", "fast"), ("right", "balanced"), ("left", "fast"),
                             ("home", "balanced"), ("end", "fast"), ("left", "balanced")):
            moved, _ = app.action(f"radio-{chord}-{style}", "key", chord=chord)
            radio_state(moved, style)
        app.observe("keyboard-radios", evidence=True)
        unchanged = find(first, "forms-status").get("name")
        for value, error in (("", "请输入显示名称。"), ("admin", "此名称不可用，请换一个名称。")):
            invalid = replace_name(app, value)
            field_state(invalid, value, error)
            invalid, _ = app.action("invalid-enter", "key", chord="enter")
            field_state(invalid, value, error)
            require(find(invalid, "forms-status").get("name") == unchanged, "invalid Enter saved the form")
            app.observe("blank" if not value else "reserved-name", evidence=True)
            if not value:
                app.picture("invalid-gpu")
        valid = replace_name(app, "  王小明🙂  ")
        field_state(valid, "  王小明🙂  ")
        require(valid["text_input"]["composition"] == "", "Chinese text was not committed")
        saved, _ = app.action("chinese-enter-save", "key", chord="enter")
        require(find(saved, "forms-status").get("name") == "已保存：王小明🙂 · balanced", "committed Chinese Enter did not save")
        app.observe("saved-chinese", evidence=True)
        narrow, reply = app.action("native-390x640", "size", width=390, height=640)
        resize = reply["live"].get("resize", {})
        require(resize.get("requested") == [390, 640] and resize.get("matched") is True
                and resize.get("synchronized") is True, f"native resize was not matched/synchronized: {reply}")
        actual, physical, scale = resize.get("actual", []), resize.get("physical", []), resize.get("scale", 0)
        require(len(actual) == len(physical) == 2 and scale > 0
                and all(abs(a - b) < 0.2 for a, b in zip(actual, narrow["viewport"]))
                and all(abs(a - b) < 1 for a, b in zip(actual, [390, 640]))
                and all(abs(a * scale - p) < 0.2 for a, p in zip(actual, physical)), "native/framework/physical sizes disagree")
        _, settled = app.action("settle-narrow-layout", "settle", limit_ms=5000)
        require(settled.get("live", {}).get("settled") is True and not settled["live"].get("pending"), "narrow layout did not settle")
        app.observe("narrow", evidence=True)
        app.picture("narrow-gpu")
        if args.display:
            app.picture("narrow-display", display=True)
        app.action("clean", "expect", clean=True)
        return {"checks": ["field-semantics", "invalid-enter", "chinese-committed-save", "retained-editor-selection",
                           "radio-stable-identity", "radio-one-tab-stop", "radio-arrows-disabled-skip", "native-390x640", "clean"],
                "requests": app.steps, "routes": app.routes, "captures": app.captures}


@contextmanager
def offline_chat_environment(store):
    previous = os.environ.copy()
    os.environ.clear()
    os.environ.update({key: value for key, value in previous.items() if not key.startswith("OCTARA_")})
    os.environ["OCTARA_CHAT_DIR"] = str(store)
    try:
        yield
    finally:
        os.environ.clear()
        os.environ.update(previous)


def chat(args, output, sockets):
    store = output / "chat-store"
    store.mkdir()
    chat_args = SimpleNamespace(**{**vars(args), "window": args.chat})
    with offline_chat_environment(store), FormsWindow("chat", chat_args, output, sockets, []) as app:
        snapshot = app.observe("offline-model-context", evidence=True)
        description = find(snapshot, "model").get("description", "")
        require("Scripted" in description and "Applies to the next" in description, "Chat omitted its current model/next-message context")
        app.picture("offline-model-context-gpu")
        return {"noNetwork": True, "network_configuration": "all inherited OCTARA_* settings cleared; only isolated OCTARA_CHAT_DIR and Inspector set",
                "model_description": description, "requests": app.steps, "routes": app.routes, "captures": app.captures}


def main():
    repo = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--window", type=Path, default=repo / "target/debug/window")
    parser.add_argument("--octara", type=Path, default=repo / "target/debug/octara")
    parser.add_argument("--chat", type=Path, help="optional built normal octara-chat executable")
    parser.add_argument("--display", action="store_true", help="also request a window-server capture and report any fallback")
    parser.add_argument("--output", type=Path, help="new or empty evidence directory; default: new /tmp directory")
    parser.add_argument("--timeout", type=float, default=30)
    args = parser.parse_args()
    require(os.name == "posix" and hasattr(socket, "AF_UNIX"), "live verification requires Unix sockets and an interactive desktop")
    require(0 < args.timeout <= 30, "--timeout must be greater than zero and at most 30 seconds")
    args.repo = repo
    for name in ("window", "octara", "chat"):
        path = getattr(args, name)
        if path is not None:
            path = path.resolve()
            require(path.is_file() and os.access(path, os.X_OK), f"build the executable first: {path}")
            setattr(args, name, path)
    output = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="octara-forms-evidence-", dir="/tmp"))
    output.mkdir(parents=True, exist_ok=True)
    require(not any(output.iterdir()), f"--output must be empty: {output}")
    sockets = Path(tempfile.mkdtemp(prefix="octara-forms-", dir="/tmp"))
    summary = {"started": datetime.now(timezone.utc).isoformat(), "passed": False,
               "input_route": "framework", "os_input_verified": False, "native_ime_verified": False,
               "output": str(output), "window": str(args.window), "octara": str(args.octara)}
    print(f"Live forms evidence: {output}", flush=True)
    try:
        summary["forms"] = forms(args, output, sockets)
        if args.chat:
            summary["chat"] = chat(args, output, sockets)
        summary["passed"] = True
    except Exception as error:
        summary["failure"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        summary["finished"] = datetime.now(timezone.utc).isoformat()
        save_json(output / "summary.json", summary)
        if not any(sockets.iterdir()):
            sockets.rmdir()
    print("PASS live forms: validation, retained editor, radio keyboard behavior, native resize and GPU capture; OS input unverified.")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (SmokeFailure, OSError, ValueError, KeyError, struct.error, zlib.error) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)

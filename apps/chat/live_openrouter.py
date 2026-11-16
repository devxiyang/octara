#!/usr/bin/env python3
"""Opt-in real OpenRouter smoke check (uses credits and OPENROUTER_API_KEY).

Runs the ordinary Chat binary with an isolated store. Sends only the synthetic prompts
below, never repository contents. The key stays in the child environment and HTTP header.
Captures the real GPU UI; all input is through Inspector, not OS keyboard injection.
"""

import argparse
import json
import os
from pathlib import Path
import sys
import tempfile

sys.dont_write_bytecode = True
from live_model_selection import ModelWindow, REPO, SmokeFailure, find, has, last_answer, nodes, require, save, wait
from live_inspect import resize
from live_settings import edit


class ProductWindow(ModelWindow):
    def __init__(self, args, output, sockets, store):
        super().__init__(args, output, sockets, store, None)
        self.fixture = "openrouter-chat"
        self.endpoint = sockets / "chat.sock"

    def environment(self):
        env = {k: v for k, v in os.environ.items() if not k.startswith("OCTARA_")}
        env.update(OCTARA_INSPECT=str(self.endpoint), OCTARA_CHAT_DIR=str(self.store))
        return env

    def finished(self):
        def ready(snapshot):
            failures = [n["id"] for n in nodes(snapshot) if n.get("id", "").startswith("failed-")]
            require(not failures, f"model reply failed: inspect {failures} in the retained transcript")
            return not has(snapshot, "stop")
        return wait(120, self.inspect, ready, "the real model's completed answer")

    def clean(self):
        issues = self.guarded("issues").get("issues", [])
        # Conversation titles deliberately ellipsize; full names remain in the semantic tree.
        unexpected = [issue for issue in issues if not (
            issue.get("code") == "TEXT_TRUNCATED"
            and (issue.get("id") in {"title", "model"} or issue.get("id", "").startswith("thread-title-")))]
        require(not unexpected, f"window layout or diagnostics: {unexpected}")


def run(args, output, sockets, store, summary):
    with ProductWindow(args, output, sockets, store) as app:
        require(find(app.inspect(), "model").get("value") == "DeepSeek V4.1 Flash", "OpenRouter preset was not selected")
        app.clean()
        app.observation("01-welcome")
        app.gpu("01-welcome.png")
        print("Captured welcome screen", flush=True)
        app.guarded("tap", id="settings-open")
        edit(app, "settings-temperature", "0.4")
        edit(app, "settings-tokens", "4096")
        app.guarded("tap", id="settings-save")
        app.say("请用中文解释 Rust 的所有权：一个三级标题、三个简短要点，最后给出一个 5 行以内的代码示例。总计不超过 180 字。")
        app.finished()
        app.observation("02-answer")
        app.clean()
        app.gpu("02-answer.png")
        print("Completed real Chinese reply", flush=True)
        app.say("把上面的解释用一个日常比喻讲清楚，80 字以内。")
        complete = app.finished()
        app.guarded("tap", id=f"regenerate-{last_answer(complete)}")
        app.finished()
        app.observation("03-followup")
        app.clean()
        app.gpu("03-followup.png")
        print("Completed follow-up and regeneration", flush=True)
        app.say("请列出 80 个 Rust 学习练习，每个练习包含题目和详细解答。")
        app.until(lambda s: has(s, "stop"), "a cancellable model request")
        app.guarded("tap", id="stop")
        app.until(lambda s: not has(s, "stop"), "stopped generation")
        app.observation("04-stopped")
        app.guarded("tap", id="new-chat")
        app.guarded("tap", id="settings-open")
        app.clean()
        app.gpu("05-settings.png")
        app.guarded("tap", id="settings-cancel")
        resize(app, 390, 780)
        app.clean()
        app.observation("06-compact")
        app.gpu("06-compact.png")
        save(output / "routes.json", app.routes)
        saved = wait(15, lambda: [json.loads(p.read_text()) for p in store.glob("*.v2.json")],
                     lambda rows: bool(rows) and any(m["status"] == "Stopped" for row in rows for m in row["messages"]),
                     "the stopped conversation to persist")
        answers = [m for row in saved for m in row["messages"] if m["author"] == "Assistant" and m["status"] == "Done"]
        require(len(answers) == 2, "expected two completed answers after regeneration")
        require(all(any("Text" in part and part["Text"].strip() for _, part in m["parts"]) for m in answers), "completed answer has no text")
        summary.update(ok=True, model="deepseek/deepseek-v4.1-flash", completed_answers=2, regenerated=True,
                       stopped=True, persisted=True, native_resize=True, gpu_captures=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--window", type=Path, default=REPO / "target/debug/octara-chat")
    parser.add_argument("--cli", type=Path, default=REPO / "target/debug/octara")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    require(bool(os.environ.get("OPENROUTER_API_KEY")), "OPENROUTER_API_KEY must be set; its value is never printed")
    args.repo, args.octara, args.window, args.timeout = REPO, args.cli.resolve(), args.window.resolve(), 30
    output = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="octara-openrouter-", dir="/tmp"))
    output.mkdir(parents=True, exist_ok=True)
    require(not any(output.iterdir()), f"evidence directory must be empty: {output}")
    store = output / "store"
    store.mkdir()
    sockets = Path(tempfile.mkdtemp(prefix="router-sock-", dir="/tmp"))
    summary = {"ok": False, "input_route": "framework", "os_input_verified": False}
    print(f"OpenRouter evidence: {output}", flush=True)
    try:
        run(args, output, sockets, store, summary)
    except BaseException as error:
        summary["error"] = str(error)
        raise
    finally:
        save(output / "summary.json", summary)
        try:
            sockets.rmdir()
        except OSError:
            pass
    print("PASS real OpenRouter Chat: Chinese reply, follow-up, regeneration, stop, save, resize and GPU captures")


if __name__ == "__main__":
    try:
        main()
    except (SmokeFailure, OSError, ValueError, KeyError, StopIteration) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)

#!/usr/bin/env python3
"""Verify Chat's async ownership through a real window and the local Inspector.

Build `cargo build -p octara-chat --example lifecycle -p octara-cli --bin octara`, then run
`python3 apps/chat/live_lifecycle.py`. Uses only Python's stdlib. The fixture's model keeps old
senders after cancellation; fixture-only commands release data while all Chat actions use its
actual UI. This verifies framework input in a desktop window, not OS input injection.
"""

import argparse
import os
from pathlib import Path
import re
import socket
import subprocess
import sys
import tempfile
import threading
import time

REPO = Path(__file__).resolve().parents[2]
sys.dont_write_bytecode = True
sys.path.insert(0, str(REPO / "examples"))
from live_inspect import LiveWindow, SmokeFailure, find, nodes, require  # noqa: E402


class ChatWindow(LiveWindow):
    def __init__(self, name, args, output, sockets, store):
        super().__init__(name, args, output, sockets)
        self.store = store

    def __enter__(self):
        self.log = (self.output / f"{self.fixture}-window.log").open("w", encoding="utf-8")
        self.transcript = (self.output / f"{self.fixture}-cli.jsonl").open("w", encoding="utf-8")
        env = os.environ.copy()
        for name in ("OCTARA_CAPTURE", "OCTARA_CONFORMANCE", "OCTARA_EXIT_AFTER_FRAMES", "OCTARA_PICK"):
            env.pop(name, None)
        env["OCTARA_INSPECT"] = str(self.endpoint)
        env["OCTARA_CHAT_DIR"] = str(self.store)
        self.process = subprocess.Popen([str(self.args.window)], cwd=self.args.repo, env=env,
                                        stdout=self.log, stderr=subprocess.STDOUT)
        try:
            deadline = time.monotonic() + self.args.timeout
            while not self.endpoint.exists():
                self.alive()
                require(time.monotonic() < deadline, "Chat Inspector socket did not appear")
                threading.Event().wait(0.05)
            self.inspect()
            return self
        except BaseException:
            self.__exit__(*sys.exc_info())
            raise

    def control(self, command):
        self.requests({"op": "invoke", "command": f"lifecycle.{command}"})

    def emit(self, ordinal, verb):
        self.control(f"reply.{ordinal}.{verb}")

    def count(self, count):
        self.control(f"replies.{count}")

    def wait_for(self, predicate, reason):
        deadline = time.monotonic() + self.args.timeout
        while True:
            snapshot = self.inspect()
            if predicate(snapshot):
                return snapshot
            require(time.monotonic() < deadline, f"Chat did not reach {reason}")
            threading.Event().wait(0.05)

    def say(self, text):
        self.requests({"op": "type", "id": "composer", "text": text}, {"op": "key", "chord": "enter"})

    def settle(self):
        self.requests({"op": "settle", "limit_ms": 5000})


def has(snapshot, target):
    return any(node.get("id") == target for node in nodes(snapshot))


def under(snapshot, target):
    root = find(snapshot, target)
    pending = [root]
    names = []
    while pending:
        node = pending.pop()
        if node.get("name"):
            names.append(node["name"])
        pending.extend(reversed(node.get("children", [])))
    return " ".join(names)


def answer_id(snapshot):
    ids = [int(node["id"][4:]) for node in nodes(snapshot) if re.fullmatch(r"msg-\d+", node.get("id", ""))]
    require(ids, "Chat has no visible message")
    return max(ids)


def queued(snapshot):
    return {node["id"] for node in nodes(snapshot) if re.fullmatch(r"queued-\d+", node.get("id", ""))}


def threads(snapshot):
    return {node["id"] for node in nodes(snapshot) if re.fullmatch(r"thread-\d+", node.get("id", ""))}


def reply_visible(app, ordinal):
    marker = f"reply-{ordinal} prefix"
    return app.wait_for(lambda s: any(marker in n.get("name", "") for n in nodes(s)), marker)


def release_late(app, ordinal):
    for verb in ("late", "done", "failed", "done"):
        app.emit(ordinal, verb)
    app.settle()


def delete_conversation(app, row):
    app.requests({"op": "secondary_tap", "id": row})
    snapshot = app.wait_for(lambda s: bool(s.get("asks")), "conversation menu")
    menu = next((a for a in snapshot["asks"] if "menu" in a["ask"]), None)
    require(menu, f"Chat did not ask for a menu: {snapshot.get('asks')}")
    app.requests({"op": "answer", "id": menu["id"], "answer": {"menu": "delete"}})
    snapshot = app.wait_for(lambda s: any("prompt" in a["ask"] for a in s.get("asks", [])), "delete confirmation")
    prompt = next(a for a in snapshot["asks"] if "prompt" in a["ask"])
    app.requests({"op": "answer", "id": prompt["id"], "answer": {"prompt": 1}})
    app.wait_for(lambda s: not has(s, row), "deleted conversation")


def composer_contracts(args, output, sockets):
    store = output / "composer-store"
    store.mkdir()
    attachments = []
    for folder in ("first", "second"):
        path = output / folder / "notes.txt"
        path.parent.mkdir()
        path.write_text(f"{folder} attachment", encoding="utf-8")
        attachments.append(str(path))

    with ChatWindow("chat-composer", args, output, sockets, store) as app:
        starter = "starter-write"
        for _ in range(16):
            if app.inspect().get("focus") == starter:
                break
            app.requests({"op": "key", "chord": "tab"})
        focused = find(app.inspect(), starter)
        require(focused["role"] == "button" and focused["name"] == "Find the right words"
                and "press" in focused.get("actions", []) and "focus_visible" in focused.get("states", []),
                f"welcome card did not expose a keyboard-focused action: {focused}")
        app.capture("chat-starter-focused.png")
        app.requests({"op": "key", "chord": "enter"})
        after_starter = app.inspect()
        require(after_starter.get("focus") == "composer"
                and find(after_starter, "composer").get("value", "").startswith("Help me write"),
                "keyboard activation did not fill and focus the composer")
        app.requests({"op": "key", "chord": "mod+a"}, {"op": "type", "id": "composer", "text": "  "})
        require("disabled" in find(app.inspect(), "send").get("states", []),
                "a whitespace-only draft enabled Send")

        app.requests({"op": "tap", "id": "attach"})
        choosing = app.wait_for(lambda s: any("paths" in a["ask"] for a in s.get("asks", [])), "attachment picker")
        picker = next(a for a in choosing["asks"] if "paths" in a["ask"])
        app.requests({"op": "answer", "id": picker["id"], "answer": {"paths": attachments}})
        attached = app.inspect()
        remove = [n for n in nodes(attached) if re.fullmatch(r"remove-file-\d+", n.get("id", ""))]
        require(len(remove) == 2 and all(n["role"] == "button" and n["name"] == "Remove notes.txt"
                                         and "press" in n.get("actions", []) for n in remove),
                f"same-name attachments lack distinct removable actions: {remove}")
        app.requests({"op": "tap", "id": remove[0]["id"], "target": remove[0]["target"]})
        remaining = app.inspect()
        require(not has(remaining, remove[0]["id"]) and find(remaining, remove[1]["id"])["target"] == remove[1]["target"],
                "removing one attachment retired the other action")
        require("disabled" not in find(remaining, "send").get("states", []), "the attachment-only draft cannot be sent")
        app.requests({"op": "tap", "id": "send"})
        app.count(1)
        app.say("queued in A")
        waiting_a = app.inspect()
        thread_a, = threads(waiting_a)
        queue_a, = queued(waiting_a)
        editor = find(waiting_a, "composer")["target"]
        remove_a = "unqueue-" + queue_a.removeprefix("queued-")
        old_action = find(waiting_a, remove_a)["target"]

        app.requests({"op": "tap", "id": "new-chat"})
        app.say("start B")
        app.count(2)
        app.say("queued in B")
        waiting_b = app.inspect()
        queue_b, = queued(waiting_b)
        require(queue_b != queue_a and not has(waiting_b, remove_a), "conversation B exposed an action from A")
        require(find(waiting_b, "composer")["target"] == editor, "switching to B replaced the composer")
        with app.wire() as stream:
            refusal = app.wire_request(stream, {"op": "tap", "id": remove_a, "target": old_action})
            require(refusal.get("code") == "STALE_TARGET" and refusal.get("delivery") == "refused",
                    f"old queue action was not refused: {refusal}")
        app.requests({"op": "tap", "id": thread_a})
        returned = app.inspect()
        require(queued(returned) == {queue_a} and find(returned, remove_a)["target"] != old_action,
                "A did not restore its queue with a fresh action target")
        require(find(returned, "composer")["target"] == editor, "returning to A replaced the composer")


def run(args, output, sockets, store):
    with ChatWindow("chat-live", args, output, sockets, store) as app:
        app.say("frozen question")
        app.count(1)
        app.control("titles")
        app.emit(0, "text")
        first = reply_visible(app, 0)
        first_answer = answer_id(first)
        kept_rows = threads(first)
        require(len(kept_rows) == 1, "first question should create one conversation")
        kept_row = next(iter(kept_rows))
        app.capture("chat-streaming.png")

        app.requests({"op": "tap", "id": "stop"})
        stopped = app.wait_for(lambda s: has(s, f"regenerate-{first_answer}") and not has(s, "stop"), "stopped answer")
        frozen = under(stopped, f"msg-{first_answer}")
        require("Stopped" in frozen and "LATE" not in frozen, f"cancel-time event changed stopped answer: {frozen}")
        app.control("cancelled.0")
        app.capture("chat-stopped.png")
        release_late(app, 0)
        require(under(app.inspect(), f"msg-{first_answer}") == frozen, "late events changed the stopped answer")
        app.count(1)

        app.say("replacement question")
        app.count(2)
        app.emit(1, "text")
        replacement = reply_visible(app, 1)
        require(has(replacement, "stop"), "the replacement answer is not streaming")
        app.capture("chat-new-answer.png")
        app.say("queued second")
        app.say("queued third")
        waiting = app.wait_for(lambda s: len(queued(s)) == 2, "two queued questions")
        waiting_ids = queued(waiting)
        release_late(app, 0)
        after_late = app.inspect()
        require(has(after_late, "stop") and queued(after_late) == waiting_ids,
                "old completion ended the new answer or consumed its queue")
        require(under(after_late, f"msg-{first_answer}") == frozen, "old text changed after the new answer started")
        app.count(2)

        app.emit(1, "done")
        app.wait_for(lambda s: len(queued(s)) == 1 and has(s, "stop"), "first queued question starting")
        app.count(3)
        release_late(app, 1)
        app.count(3)
        require(has(app.inspect(), "stop"), "duplicate terminal event ended the queued answer")
        app.emit(2, "text")
        reply_visible(app, 2)
        app.emit(2, "done")
        app.wait_for(lambda s: not queued(s) and has(s, "stop"), "last queued question starting")
        app.count(4)
        release_late(app, 2)
        app.count(4)
        app.emit(3, "text")
        last = reply_visible(app, 3)
        old_answer = answer_id(last)
        app.emit(3, "done")
        app.wait_for(lambda s: has(s, f"regenerate-{old_answer}") and not has(s, "stop"), "queue completed")

        app.requests({"op": "tap", "id": f"regenerate-{old_answer}"})
        app.count(5)
        app.emit(4, "text")
        regenerated = reply_visible(app, 4)
        require(has(regenerated, "stop") and not has(regenerated, f"msg-{old_answer}"), "regeneration retained the old answer")
        release_late(app, 3)
        app.count(5)
        require(has(app.inspect(), "stop"), "old completion ended the regenerated answer")
        app.emit(4, "done")
        app.wait_for(lambda s: not has(s, "stop"), "regeneration completed")

        app.requests({"op": "tap", "id": "new-chat"})
        app.say("delete this conversation")
        app.count(6)
        app.emit(5, "text")
        disposable = reply_visible(app, 5)
        new_rows = threads(disposable) - kept_rows
        require(len(new_rows) == 1, "second question should create one disposable conversation")
        deleted_row = next(iter(new_rows))
        stopped_answer = answer_id(disposable)
        app.requests({"op": "tap", "id": "stop"})
        app.wait_for(lambda s: has(s, f"regenerate-{stopped_answer}") and not has(s, "stop"), "stopped answer ready to regenerate")
        app.control("cancelled.5")
        app.requests({"op": "tap", "id": f"regenerate-{stopped_answer}"})
        app.count(7)
        app.emit(6, "text")
        regenerated = reply_visible(app, 6)
        require(has(regenerated, "stop") and not has(regenerated, f"msg-{stopped_answer}"),
                "regenerating a stopped answer did not replace the old message")
        release_late(app, 5)
        after_late = app.inspect()
        require(has(after_late, "stop") and not has(after_late, f"msg-{stopped_answer}"),
                "late events from the stopped answer changed its regenerated replacement")
        app.count(7)
        delete_conversation(app, deleted_row)
        app.control("cancelled.6")
        release_late(app, 5)
        release_late(app, 6)
        require(not has(app.inspect(), deleted_row), "late events resurrected the deleted conversation")
        app.count(7)
        app.requests({"op": "tap", "id": kept_row})
        saved = app.wait_for(lambda s: any("reply-4 prefix" in n.get("name", "") for n in nodes(s)), "kept answer")
        require("LATE" not in under(saved, "transcript"), "late data reached the persisted conversation")
        app.settle()

    # Launch a fresh process with the same isolated store and verify actual saved product data.
    with ChatWindow("chat-restart", args, output, sockets, store) as app:
        loaded = app.inspect()
        require(threads(loaded) == {kept_row}, "restart restored a deleted conversation or lost the kept one")
        app.requests({"op": "tap", "id": kept_row})
        snapshot = app.wait_for(lambda s: any("reply-4 prefix" in n.get("name", "") for n in nodes(s)), "restored answer")
        require("LATE" not in under(snapshot, "transcript"), "restart restored stale data")
        require(not has(snapshot, "stop"), "restart restored a completed request as streaming")
        app.count(0)
        app.capture("chat-restored.png")

    log = (output / "chat-live-window.log").read_text(encoding="utf-8")
    for marker in ("cancel text attempted", "cancel done attempted", "cancel failed attempted",
                   "reply 0 late attempted", "reply 0 done attempted", "reply 0 failed attempted", "reply 3 late attempted",
                   "reply 5 late attempted", "reply 6 late attempted"):
        require(marker in log, f"fixture did not actually attempt {marker}")
    for path in store.glob("*.json"):
        require("LATE" not in path.read_text(encoding="utf-8"), f"stale content was persisted in {path.name}")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--chat", type=Path, default=REPO / "target/debug/examples/lifecycle", help="built lifecycle window fixture")
    parser.add_argument("--octara", type=Path, default=REPO / "target/debug/octara", help="built CLI")
    parser.add_argument("--output", type=Path, help="empty output directory; default: a new /tmp directory")
    parser.add_argument("--timeout", type=float, default=15, help="per-operation timeout, at most 30 seconds")
    args = parser.parse_args()
    if os.name != "posix" or not hasattr(socket, "AF_UNIX"):
        print("SKIP: Chat live lifecycle verification uses Unix-domain sockets")
        return 0
    require(0 < args.timeout <= 30, "timeout must be greater than zero and at most 30 seconds")
    args.repo = REPO
    for name in ("chat", "octara"):
        path = getattr(args, name).resolve()
        require(path.is_file() and os.access(path, os.X_OK), f"build the executable first: {path}")
        setattr(args, name, path)
    args.window = args.chat  # Reuse the CLI and capture helpers from the general live check.
    output = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="octara-chat-live-", dir="/tmp"))
    if args.output:
        output.mkdir(parents=True, exist_ok=True)
        require(not any(output.iterdir()), f"output directory must be empty: {output}")
    store = output / "store"
    store.mkdir()
    print(f"Chat lifecycle artifacts: {output}", flush=True)
    with tempfile.TemporaryDirectory(prefix="octara-chat-sock-", dir="/tmp") as temporary:
        run(args, output, Path(temporary), store)
        composer_contracts(args, output, Path(temporary))
    print("PASS Chat live lifecycle: cancellation, regeneration, persistence, session queues, attachment actions and keyboard focus")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (SmokeFailure, OSError, ValueError, KeyError, StopIteration) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)

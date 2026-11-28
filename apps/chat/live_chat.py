#!/usr/bin/env python3
"""Drive an isolated Chat window over the Octara Inspect v2 Unix socket."""

import argparse
import json
import socket
import time


class Client:
    def __init__(self, stream, window):
        self.stream = stream
        self.window = window

    def request(self, op, **arguments):
        request = {"protocol": "octara.inspect/2", "op": op, **arguments}
        if self.window is not None:
            request["window"] = self.window
        self.stream.write(json.dumps(request).encode() + b"\n")
        self.stream.flush()
        line = self.stream.readline(32 * 1024 * 1024 + 1)
        if not line.endswith(b"\n"):
            raise RuntimeError("Incomplete Inspector receipt")
        reply = json.loads(line)
        if reply.get("protocol") != request["protocol"] or not reply.get("ok") or reply.get("diagnostics"):
            raise RuntimeError(f"Inspector request failed: {reply.get('code')} {reply.get('message')}")
        return reply

    def observe(self):
        return self.request("observe")["snapshot"]

    def act(self, target, kind, **arguments):
        receipt = self.request("act", target=target, action={"kind": kind, **arguments})
        if not receipt.get("handled"):
            raise RuntimeError(f"The Chat window did not handle {kind}")

    def send(self, text):
        snapshot = self.observe()
        self.act(snapshot["target"], "menu_command", name="chat.focus_composer")
        snapshot = self.observe()
        editor = snapshot["editor"]
        session = {"session": editor["session"], "expected_rev": editor["document_rev"]}
        self.act(snapshot["focus"], "editor", event={"edit": {
            **session, "edit": {"insert": {"text": text, "replacement": None}},
        }})
        snapshot = self.observe()
        editor = snapshot["editor"]
        self.act(snapshot["focus"], "editor", event={"key_intercept": {
            "session": editor["session"],
            "expected_rev": editor["document_rev"],
            "key": {"key": "enter", "code": None, "modifiers": "", "repeat": False},
        }})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--connect", required=True, help="Socket of a newly opened isolated Chat window")
    parser.add_argument("--window", type=int)
    parser.add_argument("--question", default="What is the weather?")
    parser.add_argument("--contains", default="cloudy in Oslo", help="Expected answer fragment")
    parser.add_argument("--timeout", type=float, default=12)
    args = parser.parse_args()
    with socket.socket(socket.AF_UNIX) as sock:
        sock.settimeout(args.timeout)
        sock.connect(args.connect)
        with sock.makefile("rwb") as stream:
            chat = Client(stream, args.window)
            before = {node["test_id"] for node in chat.observe()["nodes"] if (node.get("test_id") or "").startswith("regenerate-")}
            chat.send(args.question)
            deadline = time.monotonic() + args.timeout
            while time.monotonic() < deadline:
                nodes = chat.observe()["nodes"]
                if any((node.get("test_id") or "").startswith("failure-") for node in nodes):
                    raise RuntimeError("Chat reported a failed reply")
                if any((node.get("test_id") or "").startswith("regenerate-") and node["test_id"] not in before for node in nodes):
                    text = " ".join(node["text"] for node in nodes if node.get("text"))
                    if args.contains not in text:
                        raise RuntimeError("The completed reply lacks the expected text")
                    print("Chat reply completed and matched the expected text.")
                    return
                time.sleep(0.05)
    raise RuntimeError("Timed out waiting for Chat to finish its reply")


if __name__ == "__main__":
    main()

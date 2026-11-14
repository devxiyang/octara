# Chat reference app

Run the normal app with `cargo run -p octara-chat`. It uses the offline scripted model and saves
conversations in its application data directory (`OCTARA_CHAT_DIR` overrides that location).

## Request lifecycle verification

The headless lifecycle tests and real-window fixture use a controllable model which retains its
old senders after cancellation. This reproduces in-flight text and terminal events without a
network service or timing-dependent sleeps. The fixture also attempts late events from inside
the transport's cancel callback.

Build the fixture and CLI, then run the desktop check:

```sh
cargo build -p octara-chat --example lifecycle -p octara-cli --bin octara
python3 apps/chat/live_lifecycle.py
```

The script opens the actual Chat UI, uses the live Inspector for input and observation, and
checks stopping, a replacement answer, queued questions, regeneration after stopping and after
completion, deletion of a streaming conversation, and restart.
Streaming, stopped, replacement and restored states are captured with the window's GPU renderer.
Its isolated store, screenshots, window log and JSONL transcripts stay in the printed temporary
directory. An explicit `--output DIRECTORY` must name an empty directory. The script only stops
the child windows it started.

The `lifecycle.*` commands belong to this verification example. They release model data and
assert request counts; they are not part of the normal Chat executable. The log records each
late-event delivery attempt, including those rejected because the receiving request has retired.
Chat interactions use the product's normal controls and the framework input route.

To open the fixture manually, supply an isolated store and Inspector socket:

```sh
OCTARA_CHAT_DIR=/tmp/octara-chat-fixture OCTARA_INSPECT=/tmp/octara-chat.sock \
  cargo run -p octara-chat --example lifecycle
```

After sending a question, a live `invoke` of `lifecycle.reply.0.text` releases its first text;
`lifecycle.reply.0.done` ends that reply. The ordinal counts model reply requests from zero in
that process. `late` and `failed` release stale data, even after cancellation. All of these
commands require the corresponding reply to have started.

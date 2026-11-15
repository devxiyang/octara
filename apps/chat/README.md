# Chat reference app

Run the normal app with `cargo run -p octara-chat`. It uses the offline scripted model and saves
conversations in its application data directory (`OCTARA_CHAT_DIR` overrides that location).

To use an HTTP model, configure `OCTARA_CHAT_ENDPOINT` with a complete Chat Completions
endpoint and `OCTARA_CHAT_MODEL` with its model name. `OCTARA_CHAT_API_KEY` optionally
supplies the bearer token. The configured model is selected first; Scripted remains
available in the picker. Incomplete configuration fails at startup instead of silently
falling back. See [NETWORK.md](NETWORK.md) for local HTTP verification, configuration,
supported streaming events, cancellation and Agent observations.

## Model selection

The header uses the kit's searchable Combobox. Search supports ordinary text editing and IME;
Up/Down highlight a result, Enter chooses it, and Escape closes the picker. The composer stays
mounted, preserving its draft and selection. Long model names are truncated in the header and
shown in full by its tooltip. `model-search`, `model-option-<stable ID>` and Inspector's
`active_descendant` expose the current search and highlight.

Applications configure models with an explicit ID:

```rust
use octara_chat::{ModelEntry, Options};
use octara_chat::engine::Scripted;
use std::rc::Rc;

let options = Options {
    models: vec![ModelEntry::new("offline", Rc::new(Scripted::default()))],
    ..Options::default()
};
```

`ModelEntry.id` identifies the configured entry, independently of its display name, list
position or provider's HTTP `model` string. IDs must be nonempty and unique; duplicate display
names are allowed. IDs appear in control identifiers, so use application keys without endpoints
or credentials. The normal executable registers `network` and `scripted`.

`Chat.model` holds `Option<String>`. Reordering or renaming configuration across a reload keeps
the selected ID. Removing it makes the selection unavailable instead of selecting another entry
at its previous index. With no available selection, Send/Retry/Regenerate are disabled, keyboard
submission preserves the draft and attachments, and an existing request's Stop still works.

| Action | Model used |
| --- | --- |
| Select a model while an answer streams | The active request continues with its captured model |
| Send, edit and resend, Retry, or Regenerate | Capture the current selection before changing the conversation |
| Generate the first answer and its title | Both use the same captured model, even if a callback changes selection |
| Send while already answering | The queued message stores the model selected when Send was pressed |
| Automatic queue release or Send now | Use that queued model; later picker changes do not retarget it |
| Edit a queued message | Restore its draft and model choice; choose another model before resending if desired |
| Stop | Cancel the active request's transport, regardless of the current picker selection |

Each queued row shows its bound model. A queued entry whose model is no longer configured stays
queued; Send now is disabled until it is edited with an available model or removed. There is no
silent fallback. See [MODEL_SELECTION.md](MODEL_SELECTION.md) for HTTP and real-window evidence.

## Long-session verification

The `long_session` example opens the unchanged Chat UI with an isolated 1,000- or
10,000-message history and a controllable streaming Model. The headless regression,
real-window recording commands, Release CPU benchmark, source references and observed
results are documented in [LONG_SESSION.md](LONG_SESSION.md).

High-rate input, selection, scrolling and cancellation workloads, measurement boundaries
and the Exact/Rune design references are in [RESPONSIVENESS.md](RESPONSIVENESS.md).

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

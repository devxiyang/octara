# Examples

Example apps for Octara, their shared scenarios and a benchmark.

```sh
cargo test                                              # every test in the workspace
cargo run --release -p octara-examples --bin bench      # benchmark tables
```

- `src/apps.rs`: the apps below.
- `src/scenarios.rs`: acceptance, hot-reload and benchmark scenarios, written against a headless `octara::app::Window`.
- `tests/`: acceptance (`acceptance`), from-scratch consistency (`consistency`), diagnostics regression (`diagnostics`),
  list rendering, sorting and grouping, virtualization and streamed text.

## Inspect a real window

On macOS/Linux, enable a private local socket when launching the normal window binary:

```sh
OCTARA_INSPECT=/tmp/octara-example.sock cargo run -p octara-examples --bin window -- counter
```

In another terminal, attach to that process; use an unused socket path for each app:

```sh
cargo run -q -p octara-cli -- inspect --connect /tmp/octara-example.sock --json
cargo run -q -p octara-cli -- run --connect /tmp/octara-example.sock --json
```

The second command accepts JSON lines such as `{"op":"tap","id":"inc"}`,
`{"op":"expect","id":"count","text":"Count: 1"}` and
`{"op":"capture","path":"/tmp/octara-counter.png"}`. Capturing keeps the window open.
Live replies identify framework input and the real-time host; GPU/native capture details remain
in the capture report. The headless commands remain available for virtual-time scripts.

For timed gestures, launch `window -- gestures` instead of `window -- counter`, then send
`{"op":"double_tap","id":"double-target"}` or `{"op":"long_press","id":"hold-target"}`.
These use the recognizers on real time. `{"op":"size","width":500,"height":560}` requests
the native window's client-area size in logical pixels. Its reply confirms the actual window,
GPU surface and layout agree; `live.resize` includes the requested and actual dimensions.

After building the CLI and window binaries, run the desktop smoke check with
`python3 examples/live_inspect.py`. It starts its own windows and checks input, timed gestures,
native resize, captures, errors, window routing and reconnects. PNGs and transcripts stay in the
printed temporary directory.

## Specs

Apps are driven by test id (`role#id` below). Components come from `octara-kit`; colors are theme tokens
(`Theme::light()`), so checks compare `visual_of(..)` with `Theme::light().get(Token::..)`.

**Counter** (`counter`): `text#count` = `Count: {n}`, `text#double` = `Double: {2n}`; `button#inc` adds one, `button#reset` sets 0.

**Todo** (`todo(n)`):

- `text_field#new-todo` holds the draft; `button#add` appends `{id: next, title: trimmed draft, done: false}` if the trimmed draft is not empty, then clears it;
- `text#left` = `{k} left`; `button#filter-all|active|done` select the filter (the current one `selected`);
- `group#todos` holds one `group#todo-{id}` per visible item with `checkbox#toggle-{id}`, `text#title-{id}` (muted text color when done) and `button#remove-{id}`;
- `button#clear-done` exists while some item is done and removes them;
- `todo(n)` starts with ids `1..=n`, titles `Item {i}`.

**Chat** (`chat(n)`, `chat_virtual(n)`, `chat_stream(n)`):

- `text#header` = `{m} messages`, `text#status` = `streaming` / `idle`;
- `group#messages` holds `group#msg-{id}` with `text#msg-role-{id}` and `text#msg-body-{id}`;
- `Send("user", text)` adds a user message and an empty streaming assistant message; `Send("token", s)` appends to the last message;
  `Send("done", "")` ends streaming; `text_field#composer` + `button#send` send the composer text;
- `chat_virtual` virtualizes the list (scroll with `Event::ScrollTo("messages", 0, y)`); `chat_stream` also streams bodies as `TextBuffer`.

**Settings** (`settings`):

- `switch#notifications` (off) shows `group#notification-options` with `switch#sound` (on) and `text_field#volume` (`50`); their values persist while hidden;
- `button#theme-light|dark|system` (system `selected`);
- two keyed Disclosures after the options: `button#account-toggle` (`expanded` while open) → `group#account-body` with `text#account-name`,
  `button#advanced-toggle` → `group#advanced-body` with `checkbox#telemetry`; their open state survives sibling changes;
- `text#summary` = `notifications={on|off} sound={on|off} volume={v} theme={t} telemetry={on|off}`.

**Contacts** (`contacts(n)`, `contacts_virtual(n)`): contacts sorted by name and grouped by team, each group with a
`text#team-{team}` header `{team} ({size})`; rows `group#contact-{id}` with `text#name-{id}`, `checkbox#star-{id}`,
`button#open-{id}` (row-local detail). Events: `Send("rename", "id:name")`, `Send("team", "id:team")`,
`Send("add", "name:team")`; `contacts_virtual` flattens and virtualizes the groups (`Event::ScrollTo("contacts", 0, y)`).

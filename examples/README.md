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
native resize, captures, errors, window routing, reconnects and style declaration order (including
patch removal and reload). The style fixture can also be opened with `window styles`.
PNGs and transcripts stay in the
printed temporary directory.

## Specs

### Dialogs, navigation, suggestions and numeric controls

`cargo run -p octara-examples --bin window -- showcase interaction` combines a Dialog,
Tabs/Segmented and editor-owned PopupList suggestions. Type `/` in the dialog; Up/Down and
Enter choose suggestions without submitting. Tab panels retain their editor, selection and
undo. Cmd/Ctrl+Shift+U renames and reorders suggestions while the editor remains mounted.

`cargo run -p octara-examples --bin window -- showcase controls` combines NumberField drafts,
a Slider, determined/unknown Progress and command menus with submenus. Inspector exposes
numeric bounds and supports guarded `set_value` operations. See [PRIMITIVES.md](PRIMITIVES.md)
for contracts, source references and the real-window runner.

### Fields and radio groups

`cargo run -p octara-examples --bin window -- showcase forms` opens a small settings form.
It uses `field` for `profile-name`, controlled `radio_group` choices in `reply-style`, and an
ordinary Rust submit handler. Inspector reports the control's description, required/invalid
state and the disabled Save action's reason. Cmd/Ctrl+Shift+H changes supporting text while
the editor stays mounted; Cmd/Ctrl+Shift+R reorders and renames options by stable ID.

The headless regressions and real-window runner are documented in [FORMS.md](FORMS.md).

### Popovers and searchable choices

`cargo run -p octara-examples --bin window -- showcase choices` opens the component scene.
It uses public kit components: a controlled popover, a searchable city picker, and a second
picker inside a modal dialog. The selected value is shared between both pickers.

- `city`: the main picker; `city-search`, `city-list`, `city-option-<stable ID>` and
  `city-empty` describe its open contents. Inspector's `active_descendant` identifies the
  keyboard-highlighted option while focus stays in the search field. Selection is separate.
- `details`: a popover with the `choice-note` input and `close-details` action.
- `open-choices-dialog`: opens `choices-dialog`, containing the `dialog-city` picker and
  `close-choices-dialog` action. Closing an inner popup leaves its dialog open.
- `update-cities` (Cmd/Ctrl+Shift+U) changes labels, removes Beijing and reverses the options;
  pressing it again restores them. The command also works while a popup owns focus.
- `toggle-city` (Cmd/Ctrl+Shift+X) removes or restores the main picker, including an open popup.

The matching integration tests exercise query preservation, option changes, unmount, nested
focus restoration across hot reload, and phone/desktop layouts in both appearances. Native
verification steps and evidence are recorded in [CHOICES.md](CHOICES.md).

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

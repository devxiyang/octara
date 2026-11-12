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
- `chat_virtual` virtualizes the list (scroll with `Event::ScrollTo("messages", 0, y)`); `chat_stream` also streams bodies as `LiveText`.

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

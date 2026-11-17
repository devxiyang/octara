---
name: octara
description: "Build and change native desktop and mobile UI with the Octara Rust framework: elements, styles, state, lists, input, motion, async work, headless tests and diagnostics. Use whenever code uses the `octara` crates (`use octara::prelude::*`), the task touches Octara views or tests, or an Octara diagnostic code (such as OVERFLOW or STRICT_READ_UNTRACKED) appears."
license: MIT OR Apache-2.0
metadata:
  octara-version: "0.0.0"
---

# Octara

Octara is a native UI framework in Rust for desktop and mobile. It is not a web framework: no DOM,
no CSS cascade, no HTML semantics. Style methods use GPUI's names, which are Tailwind's vocabulary
(`p_4`, `gap_2`, `rounded_md`, `text_sm`).

Read [references/start-here.md](references/start-here.md) once before writing Octara code; it is
short and every example in it is tested. [references/api/index.md](references/api/index.md) lists
the crates and modules, one file each: every item with its signature, summary and the line that
defines it (written by `octara skill install` for the version this project uses; run it if the
directory is missing). Look anything up instead of guessing:

- `octara vocab <name>`: signature, docs, members and related entries of any function, method,
  type, token, spring, command or diagnostic code (`octara vocab bg`, `octara vocab Styled::p_4`,
  `octara vocab Token`, `octara vocab focus.next`). `--json` for structured output.
- `octara explain <CODE>`: what a diagnostic means and why it happens. It also finds
  the code inside a pasted message or panic.

If `octara` is not installed: `cargo install --git https://github.com/octoii-works/octara octara-cli`.

## Rules

- Keep ordinary UI imports small. Drawing/native constructors live in `octara::view`; their
  specialized types live in `octara_primitives::canvas`, `path` and `native`. Import JSON via
  `octara::serde_json::json`. These are not default prelude exports; use vocab for canonical paths.

- A view function runs **once** and builds a retained tree. What changes is read in closures:
  `text(move || format!("{}", n.get()))`, `.opacity(move || ..)`. Reading `.get()` in the body
  bakes the value in (`STRICT_READ_UNTRACKED`).
- Mark every function that builds part of the view `#[ui]`: elements then record which
  component made them and where it was called, for the inspector and picking.
- Ordinary functions do not create owners. Pass state handles into reusable functions; use
  `scope(key, || content)` only for an independent lifetime/context range. Duplicate scope keys
  refuse the duplicate content (`DUPLICATE_SCOPE_KEY`). Structural keys use `Eq + Hash` and their
  type for identity; `Display` is only a label. `scope` and `keyed` require keys with a `'static` lifetime.
  Named state belongs to the active owner: `state("key", || init)`, `state_list("key", init, key_fn)`,
  `text_buffer(..)`. Use `App::current().run(|| state("key", || init))` for shared App state;
  same-key/type App lookups reuse the value until shutdown.
- Register `on_cleanup(..)` during construction to retire external requests and cancel retained
  external work with the component. Input handlers restore their mounted scope; tasks they spawn
  are cancelled with it. Create App work inside `App::current().run(|| spawn(..))`; it has no
  current window. Keep prompt/focus tasks mounted. Hosts call `app.shutdown()` on exit; closing
  the last window alone does not dispose the App root, and dropping handles is not shutdown.
- Write state only in handlers (`on_press`, ..), `watch` callbacks and tasks
  (`WRITE_IN_BUILD`, `WRITE_IN_DERIVE`). A value computed from others is a `memo`, not a second
  state kept in sync.
- `memo` returns `ReadSignal`; `Signal::read_only` exposes the same read capability. Pass handles
  directly to matching properties (`text(name)`, `.checked(checked)`) or use closures to format.
  Sample explicitly with `untrack(|| value.get())`; untrack does not grant write permission.
  `watch` returns a stop handle; ignoring it keeps the watch owned by its scope.
- Lists: key items by a stable unique id; an `each` row is one element; `virtual_each` sits
  directly in a scroll container with a bounded height.
  Use `read_only()` to share a list as `ReadList` (live, not a copied snapshot); its items are
  `ReadSignal`s. `StateList` and `Groups` use the same spelling; list operators already return `ReadList`.
- Give the app root `size_full()`; a scroll area is `flex_1().overflow_y_scroll()`.
- Component authors register built-in callbacks with `.behavior(|b| b.on_input(..).on_key(..))`.
  Layers run in registration order before the replaceable application `on_*` callback; setting
  an event twice replaces it only within its own layer. A consumed key or disposed scope stops
  the chain. Kit bindings and number commits survive added application callbacks. Read the model
  for the accepted value; input/numeric arguments remain proposed values, and setters can reject
  or transform them. Submit/blur can notify invalid drafts: validate before saving.
- Input `.placeholder(..)` is a display-only hint for an empty field without composition;
  it never supplies a label, value or automatic height. Keep `.label(..)` explicit. Hint
  updates preserve selection/composition and target identity; an empty hint clears it.
- Each element has one content kind. Use input settings on `text_input()`, native settings on
  `native_view()` / `web_view()`, and image fit after `.img(..)`. Only containers (`el`, `row`,
  `col`) accept `.child(..)` / `.children(..)`; `.img(..)` requires an empty box or an existing
  image. Put captions, badges and input affixes in an external container around atomic content.
  Incompatible calls panic with `ELEMENT_CONTENT` before mounting.
- Rich-text links belong to their text. `Span::id` names a link for lookup; it is not a reuse
  key. Unchanged ranges with the same destination retain identity across style/position changes;
  replaced words or destinations retire old Agent targets even with the same id. Observe again
  after `STALE_TARGET`; do not redirect a pending press to the replacement.
- Give interactive and checked elements a test id (`.id("save")`); tests and agents drive and
  read the app by it (`app.tap("save")`, `app.kernel().text_of("total")`).
- Configure the initial theme, fonts and target conventions with `AppConfig` before
  `App::with_config`. Desktop shell options own this as `Options.app` and default to system
  fonts. Set `options.app.theme` before `run_with`, not in a deferred startup task. Text-engine
  replacement is initialization-only; runtime theme/font/environment changes use the App's APIs
  in handlers, watch callbacks or host operations. Construction, initializers and pure sources
  cannot mutate resources, even through another App handle. Shutdown rejects resource writes.
- Colors are semantic tokens (`Token::Surface`, `Token::Text`, `Token::Accent`); app colors are
  `Token::custom("brand")` defined in the theme.
- Motion is declared, never state: `.motion(Spring::SNAP)` on an element or ancestor makes changes
  there move. Tests run on a virtual clock (`advance`, `run_until_idle`).

## Done means

```rust,ignore
let mut app = Window::headless(view);
// drive it: app.tap("id"), app.press_key("mod+s"), app.type_text("hi"), app.dispatch(Event::..)
assert!(app.layout_issues().is_empty(), "{:?}", app.layout_issues());
assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
```

Tests pass, layout issues and diagnostics are empty, and `octara check` is clean. For any code,
run `octara explain <CODE>`.

## Checking and driving the running app

Name the root view once in the app's `Cargo.toml`:

```toml
[package.metadata.octara]
view = "my_app::app"   # a `fn() -> View` in the library
```

- `octara check`: the app at light and dark, phone and desktop sizes — layout issues,
  diagnostics, work that never settles, unnamed controls, small touch targets, low contrast.
  Exit 0 means clean. A finding that is fine is accepted with a reason:
  `accept = [{ code = "SMALL_TARGET", id = "dot-*", why = "..." }]` in the same table.
- `octara inspect [--size 390x844] [--theme dark]`: the semantic tree — test ids, roles, names,
  states, bounds, actions, source locations.
- `octara run --script steps.jsonl`: one JSON request per line, stopping at the first failure;
  without `--script`, requests from stdin. Requests: `{"op":"tap","id":"save"}`,
  `{"op":"type","id":"title","text":"Milk"}`, `{"op":"key","chord":"mod+s"}`,
  `{"op":"advance","ms":300}`, `{"op":"settle"}`, `{"op":"inspect"}`, `{"op":"issues"}`,
  `{"op":"expect","id":"total","text":"3 items"}`, `{"op":"expect","clean":true}`,
  `{"op":"capture","path":"shot.png"}` (a picture after settling, with a report: whether it
  settled, what each native view shows), `{"op":"pick","x":120,"y":40}` (the element at a
  point, with its source and component) (`octara vocab octara::inspect::Request` lists them
  all). In a
  running debug window, `mod+shift+c` picks with the mouse and copies the element's line. A window captures with `OCTARA_CAPTURE=shot.png` (the window's GPU renderer, native
  views from their own snapshots).

To inspect an existing desktop window on macOS/Linux, start its usual shell command with
`OCTARA_INSPECT=/absolute/path/app.sock`, then use `octara inspect --connect /absolute/path/app.sock`
or `octara run --connect /absolute/path/app.sock --script steps.jsonl --json`. These commands attach
to that running process without building a headless harness. The socket is private to its owner;
choose an unused path. One connection runs requests in order. Tap, key and typing use Octara's
framework events; focus and invoke use semantic operations. Captures use the window's GPU renderer
and keep it running. Replies report the actual route; a `run` request `{"op":"inspect"}` also
lists `live.operations`. `double_tap` and `long_press` dispatch timed framework pointer events
while the native event loop continues, using real time. `size` requests logical client-area pixels
from the native window and waits for its actual size, GPU surface and framework viewport to agree;
`live.resize` reports the requested and actual sizes, including a window-system constraint or
timeout. Live settling also waits within a wall-clock budget. Virtual `advance` remains headless.
If a connection fails after an action, inspect the app before retrying: it may already have run.

Use `inspect --json` or JSON `inspect`/`pick` requests to retain the returned opaque `target`,
then send it with the operation, for example
`{"op":"tap","id":"save","target":"<node.target>"}`. Node operations require that
node's target; `key` or `type` without an id require the focused node's target. Window operations
use `snapshot.target` or the corresponding `windows` entry's target. A target checks object
identity: changing a value or moving a control keeps it valid; replacing a node, rebuilding a
tree or reopening its window retires it. `STALE_TARGET` means observe again. `delivery: "refused"`
means no input was sent; `"delivered"` on a failure means an earlier phase ran and later input
was stopped. Never retry that input automatically. Targets are not authorization credentials or
state-version locks. Requests without a target retain the original test-id behavior.

`{"op":"reload","target":"<snapshot.target>"}` rebuilds the app's view trees through the real
hot-reload path, preserving named state and retiring their observed targets. Live inspection
advertises `live.target_validation: "identity"` and the `reload` operation when supported.

# Octara

Octara is a native UI framework in Rust for desktop and mobile. It is not a web framework: no DOM,
no CSS cascade, no HTML semantics. Style methods use GPUI's names, which are Tailwind's vocabulary
(`p_4`, `gap_2`, `rounded_md`, `text_sm`).

Read [references/start-here.md](references/start-here.md) once before writing Octara code; it is
short and every example in it is tested. [references/index.md](references/index.md) lists every
name you can use. Look anything up instead of guessing:

- `octara vocab <name>`: signature, docs, members and related entries of any function, method,
  type, token, spring, command or diagnostic code (`octara vocab bg`, `octara vocab Styled::p_4`,
  `octara vocab Token`, `octara vocab focus.next`). `--json` for structured output.
- `octara explain <CODE>`: what a diagnostic means and why it happens. It also finds
  the code inside a pasted message or panic.

If `octara` is not installed: `cargo install --git https://github.com/octoii-works/octara octara-cli`.

## Rules

- A view function runs **once** and builds a retained tree. What changes is read in closures:
  `text(move || format!("{}", n.get()))`, `.opacity(move || ..)`. Reading `.get()` in the body
  bakes the value in (`STRICT_READ_UNTRACKED`).
- Mark every function that builds part of the view `#[component]`: elements then record which
  component made them and where it was called, for the inspector and picking.
- State in components is named: `state("key", || init)`, `list("key", init, key_fn)`,
  `live_text(..)`. Never `Signal::new` in a component (`UNKEYED_STATE`).
- Write state only in handlers (`on_press`, ..), `watch` callbacks and tasks
  (`WRITE_IN_BUILD`, `WRITE_IN_DERIVE`). A value computed from others is a `memo`, not a second
  state kept in sync.
- Lists: key items by a stable unique id; an `each` row is one element; `virtual_each` sits
  directly in a scroll container with a bounded height.
- Give the app root `size_full()`; a scroll area is `flex_1().overflow_y_scroll()`.
- Give interactive and checked elements a test id (`.id("save")`); tests and agents drive and
  read the app by it (`app.tap("save")`, `app.kernel().text_of("total")`).
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

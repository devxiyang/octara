# Start here

The one page to read before writing an Octara app. Everything else is for looking things up:
`octara vocab <name>` gives any function, method, type, token, spring, command or diagnostic code
with its signature, docs and where it is defined; `octara explain <CODE>` explains a diagnostic;
the skill's `references/api/` lists every item by module. The code blocks on this page are
compiled and run as tests.

## What Octara is

A native UI framework in Rust for desktop and mobile. It is not a web framework: there is no DOM,
no CSS cascade and no HTML semantics. The design follows Rune and GPUI; style methods use
GPUI's names, which are Tailwind's vocabulary (`p_4`, `gap_2`, `rounded_md`, `text_sm`).

- A view function runs **once** and builds a retained tree of elements.
- What changes is read inside **closures** (bindings). When the state a closure read changes, only
  that closure re-runs and only that property updates. There is no re-render and no diffing.
- Elements are found by their **test id** (`.id("save")`): tests, agents and the accessibility
  tree all address them that way.

## A first app

```rust,no_run
use octara::prelude::*;

#[component]
fn counter() -> View {
    let count = state("count", || 0);
    col()
        .size_full()
        .p_4()
        .gap_2()
        .child(text(move || format!("Count: {}", count.get())).id("count"))
        .child(
            el().id("inc")
                .px_3()
                .py_1()
                .rounded_md()
                .bg(Token::Accent)
                .text_color(Token::AccentText)
                .hover(|p| p.bg(Token::AccentHover))
                .child("+1")
                .on_press(move || count.update(|n| *n += 1)),
        )
        .into()
}

fn main() {
    octara_shell::run("Counter", counter);
}
```

- `state(key, init)` is named state: the key identifies it within the component, so it survives
  hot reload. Use it for every piece of state a component owns.
- `text(move || ..)` is a binding; `text("fixed")` is not.
- `#[component]` marks a function that builds part of the view: every element it builds records
  it and where it was called, so the inspector and picking can say "made by
  `counter()`, called at `src/main.rs:30`". Put it on every such function.
- The app root gets `size_full()`: without it the root is only as big as its content.

The same app, tested headlessly — no window, no GPU, deterministic:

```rust
# use octara::prelude::*;
# fn counter() -> View {
#     let count = state("count", || 0);
#     col().size_full().p_4().gap_2()
#         .child(text(move || format!("Count: {}", count.get())).id("count"))
#         .child(el().id("inc").child("+1").on_press(move || count.update(|n| *n += 1)))
#         .into()
# }
let mut app = Window::headless(counter);
app.tap("inc").unwrap();
assert_eq!(app.kernel().text_of("count").as_deref(), Some("Count: 1"));
// Done means clean: no layout problems and no diagnostics.
assert!(app.layout_issues().is_empty(), "{:?}", app.layout_issues());
assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
```

## Elements and style

- Primitives: `el()` (a box, a row-direction flex container), `row()`, `col()`, `text(..)`,
  `text_input()`, `spacer()`. `.child(..)` takes an element, a `View` or a string.
- Rich text is one `text` with styled runs:
  `text(rich().text("Run ").code("cargo test").text(" or read ").link("the guide", url))`, with
  `bold`, `italic`, `styled(s, |s| s.strike().color(Token::Danger))`. A link is an element of
  its own (focusable, pressed by its words: `tap("the guide")`); `.on_link(|target| ..)` on the
  text or an ancestor gets it; with none, the system opens it (`open_url`).
- `.selectable()` on a container (a transcript, a message) makes its texts select as one
  document: drag, double press a word, triple press a text, Shift to extend, `mod+a`, `mod+c`.
  Rows of its lists keep their part while they scroll away. Tests: `select(("a", 0), ("b", 4))`,
  `selected_text()`; `.select_none()` leaves texts out (markers, control labels). Assistive
  technology reads it as a document, line by line: mark headings `.role(Role::Heading).level(1)`;
  `.live(Live::Polite)` on an element announces its changes.
- Layout is flexbox (and grid): `flex_1`, `gap_2`, `items_center`, `justify_between`,
  `w_full`, `max_w_md`, `overflow_y_scroll`. Lengths: a number is logical pixels; `relative(0.5)`
  is half the parent; `sp(n)` and the `_N` methods are theme spacing steps (`p_4` = 16 px).
- Colors are semantic tokens: `Token::Surface`, `Token::Text`, `Token::TextMuted`,
  `Token::Accent`, `Token::Border`, `Token::Danger` (`octara vocab Token` lists them with their
  light and dark values). App colors: `Token::custom("brand")`, defined in the theme.
- State styles take a `Patch`: `.hover(|p| ..)`, `.active(..)`, `.focus(..)`, `.disabled(..)`,
  `.selected(..)`, and `.group("row")` + `.group_hover("row", |p| ..)` for a child that reacts to
  its row being hovered.
- Components with styling built in come from `octara_kit`: `button`, `primary_button`,
  `ghost_button`, `toggle_button`, `icon_button`, `checkbox`, `switch`, `text_field`, `select`,
  `spinner`, `chip`, `removable_chip`, `callout`, `collapsible`, `list` / `list_item` /
  `list_header`, `split`.
- `octara_kit::markdown::markdown(source)` renders Markdown as a selectable document (headings,
  lists, tasks, code blocks with a copy button, quotes, tables). Pass a `&TextBuffer` to stream an
  answer in: each token re-parses only the block it lands in; `paced(&reply)` shows a stream
  arriving in bursts over a few frames. `mod+c` copies the text,
  `mod+alt+c` the Markdown source of the blocks the selection touches.

## State and the four layers

| Layer | What | May write state? |
| --- | --- | --- |
| Inputs | `state`, `state_list`, `text_buffer`, events | — |
| Derived | `memo(|| ..)`, `list.filter(..)`, `sort_by`, `group_by` | no |
| Observers | bindings (`text(move \|\| ..)`), `show`, `each`, `watch`'s source | no |
| Effects | handlers (`on_press`), `watch`'s callback, tasks | yes |

Writes anywhere else are rejected and reported (`WRITE_IN_BUILD`, `WRITE_IN_DERIVE`). Derive
instead of copying: a value computed from others is a `memo`, not a second `state` kept in sync.

Reading a value in the view body bakes it in, and the runtime says so:

```rust
use octara::prelude::*;

let app = Window::headless(|| {
    let n = state("n", || 1);
    text(format!("{}", n.get())).into() // read once: this text never updates
});
assert!(app.app().diagnostics().iter().any(|d| d.code == "STRICT_READ_UNTRACKED"));
// Fix: text(move || format!("{}", n.get())), or n.get_untracked() if one read is meant.
```

- `Signal`: `get`, `set`, `update(|v| ..)`, `with(|v| ..)` (borrow without cloning).
- `watch(source, callback)`: run an effect when a value changes (only `source` is tracked).
- `provide(value)` / `use_context::<T>()`: pass a value (often a `Signal`) down the tree.

## Lists

```rust
use octara::prelude::*;

#[derive(Clone)]
struct Todo {
    id: u32,
    title: String,
}

fn todos() -> View {
    let items = state_list("todos", || vec![Todo { id: 1, title: "Write docs".into() }], |t: &Todo| t.id);
    let next = state("next", || 2u32);
    col()
        .size_full()
        .child(el().id("add").child("Add").on_press(move || {
            let id = next.get();
            next.set(id + 1);
            items.push(Todo { id, title: format!("Item {id}") });
        }))
        .child(col().id("list").flex_1().overflow_y_scroll().child(each(
            move || items.source(),
            move |id, todo| {
                let id = *id;
                row()
                    .id(format!("todo-{id}"))
                    .gap_2()
                    .child(text(move || todo.with(|t| t.title.clone())))
                    .child(el().id(format!("remove-{id}")).child("Remove").on_press(move || items.remove(&id)))
                    .into()
            },
        )))
        .into()
}

let mut app = Window::headless(todos);
app.tap("add").unwrap();
assert!(app.kernel().exists("todo-2"));
app.tap("remove-1").unwrap();
assert!(!app.kernel().exists("todo-1"));
```

- Key items by a stable id (`DUPLICATE_KEY` otherwise). A row is **one** element.
- `each` applies changes row by row: an insert creates one row, a remove disposes one, and the
  state inside other rows stays.
- Views of a list: `items.filter(|t| ..)`, `sort_by`, `group_by(..).flatten()`; render with
  `each(move || view.source(), ..)`.
- Long lists: `virtual_each`, directly inside a scroll container with a bounded height
  (`VIRTUAL_LIST_NOT_SCROLLABLE` otherwise).
- A transcript or log: `.follow_tail()` on its scroll container keeps the end in view until the
  person scrolls away; `following_tail(id)` says whether it follows, `scroll_to_end(id)` goes back.

## Structure

- `show(cond, then)` / `show_else(cond, then, otherwise)`: the branch is a scope; its state
  lives while it is shown.
- `component_keyed("Card", id, || ..)`: a named scope with its own state. Prefer it over
  `component` whenever siblings share a name (`AMBIGUOUS_COMPONENT`). The attribute
  `#[component]` only names who built what; state scopes are these functions'.
- `.overlay(open, Placement::BELOW, || ..)`: popovers and menus, placed next to the element and
  kept inside the window.

## Input

- Pointer and touch: `on_press`, `on_double_press`, `on_long_press`, `on_secondary_press`,
  `on_drag(|d| ..)`. Presses come from mouse, touch, keyboard and assistive technology alike.
- Keyboard: focus moves with Tab (`tab_index`, `focus_group`, `autofocus`, `modal`); chords map to
  commands, and commands to handlers:

```rust
use octara::prelude::*;

let mut app = Window::headless(|| {
    let saved = state("saved", || 0);
    on_command("save", move || saved.update(|n| *n += 1));
    col().size_full().child(text(move || format!("saved {}", saved.get())).id("saved")).into()
});
app.bind("mod+s", Some("save"), None).unwrap(); // `mod` is cmd on Apple platforms, ctrl elsewhere
app.press_key("mod+s").unwrap();
assert_eq!(app.kernel().text_of("saved").as_deref(), Some("saved 1"));
```

- Text: `octara_kit::text_field(signal)` binds a `Signal<String>`; `.on_submit(..)` runs on Enter.
  Selection, undo, clipboard and input methods are the runtime's.
- `octara vocab focus.next` (or any command) shows the built-in commands and their keys.

## Motion

Animation is never state: the view says where things are, and `.motion(spring)` on an element (or
any ancestor) makes changes there move instead of jump — colors, opacity, shadows, positions and
sizes, including hover and theme changes. Springs: `Spring::SNAP` (hover, press), `SETTLE`,
`REFLOW` (layout), `GLIDE`, `POP`, `MORPH`. Inside a motion subtree elements fade in and out.

```rust
use octara::prelude::*;
use std::time::Duration;

let mut app = Window::headless(|| {
    let open = state("open", || false);
    col()
        .size_full()
        .motion(Spring::SNAP)
        .child(el().id("toggle").size_8().on_press(move || open.update(|o| *o = !*o)))
        .child(el().id("panel").size_8().bg(Token::Surface).opacity(move || if open.get() { 1.0 } else { 0.0 }))
        .into()
});
app.tap("toggle").unwrap();
// The style is the target at once; the drawn value moves toward it frame by frame.
assert_eq!(app.kernel().visual_of("panel").unwrap().opacity, 1.0);
app.app().advance(Duration::from_millis(50));
assert!(app.app().next_deadline().is_some(), "still moving: another frame is due");
app.app().run_until_idle(Duration::from_secs(1)).unwrap();
assert!(app.app().next_deadline().is_none(), "at rest");
```

`instant(|| ..)` makes one change land at once; `App::set_reduced_motion(true)` turns movement into
short fades. Tests run on a virtual clock: `advance`, `run_until_idle`.

## Async

Tasks run on the UI thread, owned by the scope that spawned them, and are cancelled with it. They
use the app's clock, so tests are deterministic.

```rust
use octara::prelude::*;
use std::time::Duration;

let mut app = Window::headless(|| {
    let status = state("status", || "loading".to_string());
    spawn(async move {
        sleep(Duration::from_millis(300)).await;
        status.set("ready".into());
    });
    text(move || status.get()).id("status").into()
});
assert_eq!(app.kernel().text_of("status").as_deref(), Some("loading"));
app.app().run_until_idle(Duration::from_secs(1)).unwrap();
assert_eq!(app.kernel().text_of("status").as_deref(), Some("ready"));
```

- `resource(key, fetch)`: a load that re-runs when its key changes; read `.loading()`,
  `.value()`, `.error()`. A newer load cancels the older one.
- `background(|| ..)`: blocking work on a worker thread, awaited in a task.
- `channel()`: a `Sender` for other threads (a socket, an LLM's tokens) and a `Receiver` on the UI
  thread. `interval(period, f)` ticks while its scope lives.
- The system: `open_window(options, view)` / `close_window(id)`; `open_url(url)`;
  `write_to_clipboard(text or image)` / `read_from_clipboard()`; `prompt_for_paths(options)` and
  `prompt_for_new_path(dir, name)`, awaited in a task (`None`: cancelled); files dropped from the
  system reach `.on_drop(|paths| ..)`, styled meanwhile by `.drag_over(|p| ..)`; the pointer's
  look is `.cursor(CursorStyle::..)` (`cursor_pointer()`), else the platform's.

## Testing and checking

- `Window::headless(view)` is `view` in a window of a headless app. Drive the window by test id:
  `tap`, `double_tap`, `long_press`, `drag`, `swipe`; by keys: `press_key("tab")`,
  `type_text("hello")`; or with events: `dispatch(Event::Press("save".into()))`.
- Read back: `app.kernel().text_of(id)`, `state_of(id)` (hovered, focused, pressed, ..),
  `visual_of(id)` (colors, opacity), `exists(id)`, `app.accessibility_tree()`.
- What is the app's, not the window's, is `app.app()`'s: the clock (`advance(d)`,
  `run_until_idle(limit)`, `settle()`), `diagnostics()`, the theme and fonts, other windows
  (`open_window`, `windows`).
- Check: `layout_issues()` (overflow, collapsed elements, truncated text, clamped overlays,
  missing tokens) and `app.app().diagnostics()` should be empty; `at_sizes(..)` runs a check at
  phone and desktop sizes. `app.app().set_check_consistency(true)` rebuilds every window from
  scratch after each operation and panics where the incremental tree differs.

## Checking the running app

Name the root view in `Cargo.toml` (`[package.metadata.octara] view = "my_app::app"`), then:
`octara check` judges the app at light and dark, phone and desktop sizes (layout, diagnostics,
work that never settles, accessibility); `octara inspect` prints the semantic tree; and
`octara run --script steps.jsonl` drives it with one JSON request per line
(`{"op":"tap","id":"save"}`, `{"op":"expect","id":"total","text":"3 items"}`). The same requests
work in a test: `octara::inspect::run_script(&mut app, script)`.

What a person points at reaches you as a point or a picked line: `{"op":"pick","x":120,"y":40}`
names the element at a point. In a running debug window, `mod+shift+c` picks with the mouse and
copies a line such as `button "Save" (id save) at src/app.rs:52:9; made by toolbar() called at
src/app.rs:30:5`.

## When something is wrong

- A diagnostic or layout issue names its code: `octara explain OVERFLOW` gives its cause and the
  rule behind it; it also finds the code inside a pasted panic or message.
- Unsure what a method takes: `octara vocab hover`, `octara vocab Styled::p_4`,
  `octara vocab Spring::SNAP`. A type lists its members.

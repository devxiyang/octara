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

`octara` provides the runtime and UI construction. `octara-primitives` owns window-independent
geometry, drawing, theme values and presentation data; the renderers consume that package without
depending on the UI runtime. Most views use `octara::prelude::*`, which also imports the common
primitive types. Specialized drawing and native interop use explicit imports:
`octara::view::{canvas, native_view, web_view}`, `octara_primitives::canvas::Canvas`,
`octara_primitives::path::{Path, Stroke, point}` and `octara_primitives::native::{NativeEvent, Slot}`.
Import JSON explicitly from `octara::serde_json::json`. These capabilities remain available;
they are not default prelude names. For primitive module APIs add `octara-primitives` as a dependency. `octara-kit` is optional:
applications and custom component libraries use the same public foundation.
Text shaping, fonts and layout are owned by `octara-text`; add it as a direct dependency when
using a custom `TextEngine` or the deterministic `MonoEngine`. The runtime uses it internally;
renderers only consume primitive glyph data.

## Initial resources

Configure resources before mounting, so the first build and frame use the intended theme,
fonts and input conventions:

```rust
use octara::prelude::*;
use octara::platform::HeadlessPlatform;

let app = App::with_config(HeadlessPlatform::default(), AppConfig {
    theme: Theme::dark(),
    ..Default::default()
});
let window = app.open_window(WindowOptions::new("Configured"), || text("Ready").into()).unwrap();
assert_eq!(window.inspect().theme, "dark");
app.shutdown();
```

`AppConfig::default()` uses bundled fonts. With the `system-fonts` feature,
`AppConfig::default().with_system_fonts()` selects installed fonts before mounting. For a custom
engine, set `text_engine`; register initial fonts on that engine before passing the config to
`App::with_config`. The engine is owned by the App and cannot be replaced at runtime.

Runtime resource changes use the App's methods (or `set_theme` for the current App). They obey
write permissions: handlers, watch callbacks and host operations may change them; construction,
state initializers, memos and binding/watch sources may not, including through another App's
handle. Shutdown rejects resource writes; a failed App must be shut down. A live App's window
cleanup may still update its resources, but shutdown cleanup may not.

Custom text engines and their returned layouts run without dependency collection, UI write
permission or an ambient window/scope, including native prediction outside an App operation.
They must perform text-provider work, not reenter UI driving/layout. Native queries can decline
while the engine is busy; empty geometry needs no engine lease. Queries decline after shutdown
or for failed/dropped owners, including empty text. An escaping provider panic fails the App; it does not
publish a partially updated frame or roll back arbitrary provider work.

Desktop `octara_shell::Options::default()` already uses system fonts. Customize its `app.theme`
before `run_with`; assign `app: AppConfig::default()` to use bundled fonts. Do not spawn a task
just to set the initial theme. `Window::headless` remains the short default setup.

## A first app

```rust
use octara::prelude::*;

#[ui]
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
- `#[ui]` marks a function that builds part of the view: every element it builds records
  it and where it was called, so the inspector and picking can say "made by
  `counter()`, called at `src/main.rs:30`". Put it on every such function.
- The app root gets `size_full()`: without it the root is only as big as its content.

The same app, tested headlessly — no window, no GPU, deterministic:

```rust
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
- Each element has one content kind. Input settings require `text_input()`; native settings
  require `native_view(..)` or `web_view(..)`. For images, write
  `el().img(&image).object_fit(ObjectFit::Contain)` in that order. Compose different content
  inside an `el()` / `row()` / `col()` container. Text, inputs, images, icons, Canvas and native
  views reject `.child(..)` / `.children(..)`; `.img(..)` also rejects a box with declared children.
  Use an external container for captions, badges and input affixes. Incompatible calls panic
  with `ELEMENT_CONTENT` before mounting.
- Rich text is one `text` with styled runs:
  `text(rich().text("Run ").code("cargo test").text(" or read ").link("the guide", url))`, with
  `bold`, `italic`, `styled(s, |s| s.strike().color(Token::Danger))`. A link is an element of
  its own (focusable, pressed by its words: `tap("the guide")`); `.on_link(|target| ..)` on the
  text or an ancestor gets it; with none, the system opens it (`open_url`).
  Links are owned by their text, not application children. Restyling or shifting an unchanged
  range preserves its target identity; replacing its words or destination retires it even with
  the same `Span::id`. That id is a lookup name, not a reuse key.
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
- Base styles follow declaration order: `.opacity(alpha).opacity(0.5)` stays at `0.5`, even
  when the Signal `alpha` changes. `.style_with(move |p| ..)` replaces its patch at that same
  position each run; omitted properties fall back to earlier declarations or defaults.
  State and screen overrides apply after the base styles.
- Components with styling built in come from `octara_kit`: `button`, `primary_button`,
  `ghost_button`, `toggle_button`, `icon_button`, `checkbox`, `switch`, `text_field`, `select`,
  `combobox`, `popover`, `field`, `radio_group`, `dialog`, `tabs`, `segmented`, `popup_list`,
  `slider`, `number_field`, `progress`, `menu_button`,
  `spinner`, `chip`, `removable_chip`, `callout`, `collapsible`, `list` / `list_item` /
  `list_header`, `split`, `splitter`.
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
// Fix: text(move || format!("{}", n.get())), or untrack(|| n.get()) for an intentional snapshot.
```

- `Signal`: `get`, `set`, `update(|v| ..)`, `with(|v| ..)` (borrow without cloning).
- `read_only()` and `memo(..)` return `ReadSignal`, with the same `get` / `with` reads.
  Pass matching handles directly to properties: `text(name)`, `.checked(checked.read_only())`,
  `.label(title)`. A closure is still useful for formatting and combining values.
- Ordinary structs and collections need not implement `Clone`: `with` borrows and `update` mutates
  in place. Only `get` requires `Clone`. Finish a borrow before writing the same value; a rejected
  `update` never calls its mutation closure.
- `untrack(|| ..)` samples without subscribing the surrounding computation. It does not permit
  writes or effect registration inside a memo or other pure expression.
- `watch(source, callback) -> Watch`: run an effect when a value changes (only `source` is tracked).
  Source results need `PartialEq`, not `Clone`; callbacks borrow the new and previous result.
  `handle.stop()` stops early. Ignoring the handle keeps the watch until its scope is disposed.
- `provide(value)` / `use_context::<T>()`: pass a value (often a `Signal`) down the tree.

An ordinary struct can own complex data without implementing `Clone`:

```rust
use octara::prelude::*;

struct Draft { title: String, tags: Vec<String> }
let mut app = Window::headless(|| {
    let draft = state("draft", || Draft { title: "Notes".into(), tags: Vec::new() });
    let title: ReadSignal<String> = memo(move || draft.with(|d| format!("{} ({})", d.title, d.tags.len())));
    col().size_full()
        .child(text(title).id("title"))
        .child(el().id("tag").role(Role::Button).label("Tag").size_8()
            .on_press(move || draft.update(|d| d.tags.push("Rust".into()))))
        .into()
});
app.tap("tag").unwrap();
assert_eq!(app.kernel().text_of("title").as_deref(), Some("Notes (1)"));
assert!(app.app().diagnostics().is_empty());
```

## Lists

For ordinary application data, return a keyed `Vec<(K, T)>` directly. No list state or
synchronizing watch is needed:

```rust
use octara::prelude::*;

let mut window = Window::headless(|| {
    let items = state("items", || vec![(1u32, "First".to_owned())]);
    col().size_full()
        .child(el().id("rename").size_8().child("Rename")
            .on_press(move || items.update(|items| items[0].1 = "Renamed".into())))
        .child(each(move || items.get(), |key, item| text(item).id(format!("item-{key}")).into()))
        .into()
});
window.tap("rename").unwrap();
assert_eq!(window.kernel().text_of("item-1").as_deref(), Some("Renamed"));
assert!(window.app().diagnostics().is_empty());
```

Use business IDs for keys, not positions when rows can move. The row's `ReadSignal` follows
its current key and source, including a read immediately after a write in the same handler.
Changing a value or switching sources with the same logical key retains the row's local state
and target. Removal retires them. A duplicate snapshot retires the region's rows until a valid
snapshot arrives; a later recovery mounts fresh targets. Reading an absent key fails instead
of returning its previous value (`LIST_ITEM_UNAVAILABLE`).

For large, independently edited collections, use `StateList` and its incremental operators:

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
            move || items.read_only(),
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
- `items.read_only()` returns a `ReadList`: a shared, live handle without list write methods.
  It does not copy items or create a snapshot; each item is exposed as a `ReadSignal`.
- Derive lists in their creating App; both sides of `join` must belong to that App.
  Cross-App composition is rejected before callbacks run, even for empty lists (`WRONG_RUNTIME`).
- For an incremental source, `each` applies changes row by row: an insert creates one row, a remove disposes one, and the
  state inside other rows stays.
- Views of a list: `items.filter(|t| ..)`, `sort_by`, `group_by(..).flatten()`; render with
  `each(move || view.clone(), ..)`.
- Long lists: `virtual_each`, directly inside a scroll container with a bounded height
  (`VIRTUAL_LIST_NOT_SCROLLABLE` otherwise).
- A transcript or log: `.follow_tail()` on its scroll container keeps the end in view until the
  person scrolls away; `following_tail(id)` says whether it follows, `scroll_to_end(id)` goes back.

## Structure

- `show(cond, then)` / `show_else(cond, then, otherwise)`: the branch is a scope; its state
  lives while it is shown.
- `scope(key, || content)`: an explicit lifetime/context range, built when mounted without
  an extra layout node. Its key is unique within its owner; duplicate keys report
  `DUPLICATE_SCOPE_KEY` and the duplicate body is not run. State follows the key on reload,
  not sibling position. Keys implement `Eq + Hash + Display + 'static`: equality and type identify
  them; display text is only a diagnostic label. Different keys may share a label; hash collisions
  do not merge identities. Keep equality/hash stable while a key is in use; key operations
  should be pure and must not depend on reactive reads or perform UI actions.
  `keyed` uses the same contract, and lists retain their actual item keys.
  `#[ui]` records provenance only; ordinary functions do not create scopes.
- Prefer passing Signal handles into reusable functions. If a helper creates named state or
  provides local context, build it inside an explicit scope (or an existing branch/list owner).
  Prebuilt content has already run its state/context reads; inserting it does not re-run them.
- `.overlay(open, Placement::BELOW, || ..)`: popovers and menus, placed next to the element and
  kept inside the window.

## Input

Imperative focus, scroll, motion and command helpers use the same write permissions as state.
Call them from handlers/tasks; pure sources cannot bypass this through `untrack` or a captured
`Window::invoke` handle. `instant(f)` still evaluates `f` under the original permissions when
its motion request is rejected. `bind_keys` is registration, so construction can declare bindings.


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
  `.placeholder("Write here…")` shows a hint only while empty and not composing; it is not
  editable text and never replaces `.label(..)`. Hint changes do not resize the field.
  Selection, undo, clipboard and input methods are the runtime's.
  Adding `.on_input(..)` preserves the binding and observes the model after its setter runs.
- Component authors use `.behavior(|b| b.on_input(..).on_submit(..))` to register built-in
  behavior. Layers run in registration order before application `on_*` callbacks. Setting
  the same event twice replaces it within that layer; setting an application callback never
  replaces a component layer. A consumed key or disposed scope stops the chain; IME runs first.
- Bound `checkbox` and `switch` update their models before `on_toggle`. Slider press, drag and
  numeric callbacks also run after its model setter; `on_key` receives unconsumed keys.
  Input/numeric callback arguments are the proposed values. Read the model for the accepted
  value: a custom setter may transform or reject a request. These callbacks are per-event,
  not a subscription to every model change.
- `field(id, label, control, info)` surrounds an ordinary control with its label and supporting
  text. The tracked `info` closure returns `FieldInfo { description, error, required }`; these
  become the control's description, invalid and required semantics in Inspector and AccessKit.
  Validation stays in ordinary Rust, and changing it keeps the editor mounted. Validate again
  in the submit handler; `required` and `invalid` describe the value rather than blocking edits.
- `radio_group(id, label, model, options)` binds an `Option<String>` stable option ID, with
  tracked `Vec<RadioOption>` choices. Tab enters once, arrows choose among enabled options,
  and focus alone does not choose. Reordering keeps each option's identity; removing the
  chosen option leaves the model unchanged until the application or person chooses again.
- `popover(id, open, placement, trigger, content)` preserves the trigger's application
  `on_press` and updates its open model first. A later `on_press` replaces only that application
  callback. Popup content mounts during the subsequent update, not before the notification.
- `dialog(id, title, open, options, content)` is a controlled modal with a bounded, scrollable
  body. Closing disposes its content; keep a persistent draft in application state. `tabs`
  moves focus with arrows and selects with Enter/Space; the application owns its panels.
  Use a display binding to retain a panel's editor/undo, or `show` to dispose it when hidden.
  `segmented` is a compact radio group, so arrows both focus and select.
- `popup_list(id, label, open, editor, items, choose)` attaches already filtered suggestions to
  an existing editor. It retains editor focus, skips disabled items, and calls `choose` with a
  stable ID. Its keyboard and pointer choices yield to an active IME. The application owns
  filtering and insertion; this is separate from a searchable modal `combobox`.
  Editor `on_key` receives unconsumed keys; editor `on_blur` runs after the popup closes.
- `slider(id, model, min, max, step)` binds a number; `number_field(raw, min, max, step)` binds
  raw text, preserving incomplete drafts until Enter or blur. Application `on_submit` and
  `on_blur` run after normalization, preserving the binding. Invalid drafts remain unchanged
  and still notify; validate before saving. `parse_number` and
  `normalize_number` let ordinary Rust implement application validation and submission.
  `progress(value, max)` reads `Option<f64>`: `None` means unknown, not zero. Numeric bounds
  and values are shared by AccessKit and Inspector; agents can use `set_value` on editable
  numeric controls with their observed target.
- `menu_button(id, label, open, menu)` snapshots an ordinary `Menu` each opening. Its actions
  run existing commands after closing and restoring focus; submenus support Right/Left,
  navigation skips disabled items, and typing searches labels. Shortcut labels are display
  hints; use `bind_keys` to register shortcuts. `invoke_after_update` runs a command after
  pending view and focus updates; synchronous `Window::invoke` retains its existing behavior.
  Adding `on_press` preserves opening; `on_key` receives only keys the menu trigger did not
  consume. Press callbacks observe the open model before the popup is mounted.
- `octara vocab focus.next` (or any command) shows the built-in commands and their keys.

A custom bound component can use the same event-layer mechanism:

```rust
use octara::prelude::*;

let mut window = Window::headless(|| {
    let value = state("value", String::new);
    text_input().id("name").value(value)
        .behavior(|b| b.on_input(move |text| value.set(text)))
        .on_input(move |proposed| assert_eq!(value.get(), proposed))
        .into()
});
window.dispatch(Event::Input("name".into(), "Ada".into())).unwrap();
assert_eq!(window.kernel().value_of("name").as_deref(), Some("Ada"));
```

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

- `async_memo(source, compute)`: an async derivation that restarts when its source changes.
  Read its `AsyncState` with `.with(|s| ..)` or `.get()`; states are `Pending`, `Ready`, and
  `Failed`. Refresh with `.refresh()`. Only the synchronous source tracks dependencies;
  superseded results cannot publish. Refresh and failure retain the last successful value.
- `background(|| ..)`: blocking work on a worker thread, awaited in a task.
- `channel()`: a `Sender` for other threads (a socket, an LLM's tokens) and a `Receiver` on the UI
  thread. `interval(period, f)` ticks while its scope lives.
- Register `on_cleanup(..)` during component construction to retire external requests and cancel
  external work when that scope is disposed. Input handlers restore their mounted scope, so tasks
  they start are automatically cancelled with that scope. Dropping a task handle does not cancel it.
  Create App-lifetime tasks inside `App::current().run(|| spawn(..))`, without a current window;
  keep prompt/focus tasks mounted. Cancelling an awaiting task also withdraws its prompt.
- The system: `open_window(options, view)` / `close_window(id)`; `open_url(url)`;
  `write_to_clipboard(text or image)` / `read_from_clipboard()`; `prompt_for_paths(options)` and
  `prompt_for_new_path(dir, name)`, awaited in a task (`None`: cancelled); files dropped from the
  system reach `.on_drop(|paths| ..)`, styled meanwhile by `.drag_over(|p| ..)`; the pointer's
  look is `.cursor(CursorStyle::..)` (`cursor_pointer()`), else the platform's.

## App lifetime

`App::run` enters a persistent root scope. The same key and type return the same state across
calls; the initializer runs once. A conflicting type is rejected without replacing the value.
Window/component keys remain declarations per build, with duplicate-key diagnostics.

```rust
use octara::prelude::*;
use octara::platform::HeadlessPlatform;
use std::time::Duration;

let app = App::new(HeadlessPlatform::default());
let count = app.run(|| state("count", || 0));
let task = app.run(|| spawn(async move {
    sleep(Duration::from_millis(10)).await;
    count.set(1);
}));
app.poll();
app.advance(Duration::from_millis(10));
assert_eq!(app.run(|| state("count", || 99).get()), 1);
assert!(task.is_finished());
app.shutdown();
assert!(app.is_shutdown());
```

App state and tasks survive window closure and reload. `app.shutdown()` closes windows, runs
cleanup and cancels tasks; it is idempotent. It rejects new App entries and windows. The desktop
shell calls it when its event loop ends (including its last-window-exit policy); other hosts must
call it too. Dropping Rc handles alone is not a lifecycle protocol. Shutdown cannot preempt an
executing Rust callback or kill a blocking worker; their result interest and late wakes are retired.

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

For an already running desktop window on macOS/Linux, set
`OCTARA_INSPECT=/absolute/path/app.sock` when launching its normal shell, then attach with
`octara inspect --connect /absolute/path/app.sock` or
`octara run --connect /absolute/path/app.sock --script steps.jsonl --json`. Use an unused socket
path. The connection reads and drives that process, with framework input and GPU captures.
Live `double_tap`, `long_press` and `settle` wait on real time while the event loop continues.
`size` requests logical client-area pixels from the native window; `live.resize` reports its
actual size and whether the GPU surface and framework viewport agree. Virtual `advance` remains
headless. Live captures keep the app running; replies name the actual input route. A lost reply
does not prove an action failed: inspect before retrying it.

What a person points at reaches you as a point or a picked line: `{"op":"pick","x":120,"y":40}`
names the element at a point. In a running debug window, `mod+shift+c` picks with the mouse and
copies a line such as `button "Save" (id save) at src/app.rs:52:9; made by toolbar() called at
src/app.rs:30:5`.

## When something is wrong

- A diagnostic or layout issue names its code: `octara explain OVERFLOW` gives its cause and the
  rule behind it; it also finds the code inside a pasted panic or message.
- Unsure what a method takes: `octara vocab hover`, `octara vocab Styled::p_4`,
  `octara vocab Spring::SNAP`. A type lists its members.

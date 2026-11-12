# The core: ownership, context, input and frames

Status: confirmed. Ownership and context, input as data, frames, recording and replay, and what
people point at are implemented; the rest of the plan below is not. Designed for the first product (an AI chat app),
several windows, the [platform layer](platform.md) and agents.

## Goals

- One notion of "the current window", carried by the reactive runtime, not kept in step by hand.
- What is the app's is shared by its windows: the theme, the text system (fonts registered once
  are everyone's), the clock, reduced motion.
- A closed window is freed, with everything it ran.
- Input is data: one entry, recordable and replayable; tests, agents and backends drive the same
  way.
- What the platform must do after a frame is data too, so every backend shows the same thing.
- Text is rich, selectable across blocks, and streams without re-emitting what did not change.

## Ownership

| Owner | What |
| --- | --- |
| `App` (a handle, `Clone`) | an `Rc<AppState>`; nothing else |
| `AppState` (one per app) | the reactive `Runtime`; the `Scheduler` and its clock; `Resources`; the `Platform`; the windows (`SlotMap<WindowId, Rc<WindowState>>`) in the order they opened; windows to open and close at the end of the frame; tuning, the target platform's conventions, the keymap; the capture painter; the recording |
| `Resources` (`kernel::Resources`, shared by every window's kernel) | the theme (one value: a change restyles every window), the text engine (font collection, fallback, shaping and layout caches; texts are keyed by kernel and node), touch, reduced motion |
| `Window` (a handle on an `App` and a `WindowState`, `Clone`) | its `Kernel` (tree, styles, layout, paint, semantics); interaction (focus, hover, presses, edits, selection); size and scale, with their signals; handlers and commands; overlay slots; followed scroll states and sizes; after-layout hooks; its root view, scope and slots; its `PlatformWindow`; what its platform was last sent; a capture in progress |
| `Kernel` | the node tree and everything per node; it reads `Resources` (an `Rc`), it owns none of them |

A window handle outlives its window (`is_closed`). Closing a window disposes its root scope (its
bindings, watches and tasks) and the signals the window holds, then frees it; a thousand opens
and closes leave nothing behind.

## Context

- Two thread-locals, both set by the app's entry points and nothing else: the reactive runtime
  and the `AppState`.
- The reactive runtime carries one typed owner, `Owner(u64)`: every binding, memo, watch and
  task records the owner current when it is made and restores it when it runs. The view layer's
  owners are windows (a window's number).
- `window()` (crate-internal) is the current owner's window, looked up in the app's windows.
  Code that runs for a closed window cannot happen (its scope is disposed); code outside any
  window that reaches for one is a bug and panics with what ran.
- Free functions are the app's API (`state`, `spawn`, `open_window`, `close_window`, `windows`,
  `current_window`, …): they resolve through this context, so components never pass a context
  around.
- Kernel borrows are never held across app code: handlers are looked up, the borrow released,
  then called.

## Driving an app

- `App::new(platform)` makes an app without windows; `app.open_window(options, view)` opens one
  and returns its `Window`; `close_window(id)` closes one. From app code, `open_window(..)` and
  `close_window(..)` take effect at the end of the frame. `Window::headless(view)` is a window of
  a new headless app: what tests and agents drive.
- App-level operations are `App`'s: the clock (`now`, `advance`, `advance_to`, `next_deadline`,
  `poll`, `settle`, `run_until_idle`, `pending`, `send`), theme and fonts, target platform,
  reduced motion, hot reload of every window, diagnostics and work, the painter, `platform()`,
  `run(f)` for state shared by windows, `windows()`, `window(id)`, `set_check_consistency`.
  Window-level ones are `Window`'s: input, focus, inspection, the display list, the kernel, size
  and scale, native views, captures, the platform window.
- One input entry: `Window::event(InputEvent) -> bool` (whether something handled it).
  `InputEvent` is data: `Pointer` (with touch ids), `PointerLeave`, `Wheel`, `Key`, `Text`,
  `Composition`, `Resize` (size and scale), `Accessibility` (an assistive technology's action),
  `Native`, `NativeSize` and `NativeSnapshot` (what a native view reports), and `Semantic` (a
  step by test id or name). The drivers tests and agents use (`tap`, `click_at`, `wheel`,
  `press_key`, `type_text`, `resize`, `dispatch`, …) send events. Headless the clock is virtual
  (`advance`); a backend feeds the wall clock.
- Recording: `App::start_recording` / `stop_recording` give a `Recording` (each input with its
  time on the app clock and its window; serde). `App::replay` plays one on an app built the same
  way, which ends with the same trees and display lists. A snapshot's pixels are not recorded.
- After each operation the app settles every window (tasks, the reactive flush, then each
  window's layout and after-layout hooks) and presents each window's `Frame` to its platform
  window (`PlatformWindow::present`) when something changed: native view changes and placements
  (`NativeChange`), the focused field's text input, and whether to draw. The display list is not
  in it: the backend takes it (`Window::display_list`) when it draws, on its own pacing.
  Questions with answers are methods (`shows_native`, `snapshot_native`, `native_busy`).
- With consistency checks on (tests), every window is compared with a from-scratch rebuild after
  each outermost operation, and a difference panics.
- The inspector protocol's snapshot names its window and lists the app's (`window`, `windows`);
  a request line may name a window (`"window": n`, `WindowId::number`), else it goes to the
  window it was given. Its steps (`tap`, `type`, …) lower to `InputEvent`s.

## What people point at

- **Components**: `#[component]` (the `octara-macros` crate, written on the token stream) on a
  function that builds part of the view. Its body runs in an immediately called closure, so
  `#[track_caller]` does not reach through it and each element keeps its own line; the
  function itself is `#[track_caller]`, and a thread-local frame holds its name, module and
  call site while the body runs. An element records the frame current when its builder is made
  (`El`), and the views that build later (`show`, `each`, `virtual_each`, `component`,
  overlays) capture the frame where they are written and restore it when they build. A
  component that is also `#[track_caller]` is a leaf (the kit's): its elements' sources are its
  caller's, and a component entered from the same call site belongs to the outer one
  (`primary_button` calling `button` records `primary_button`). Nothing is allocated; state
  scopes stay `component(name, ..)`'s.
- The snapshot shows a node's `component` where it differs from its parent's: the boundaries.
- **Pick**: `Window::pick(x, y)` and the `pick` request: the topmost element at the point, its
  ancestors, and one line to paste (`button "Save" (id save) at src/app.rs:52:9; made by
  toolbar() called at src/app.rs:30:5`).
- **Picking**: `InputEvent::Picking(on)` (`Window::set_picking`) puts a window in picking mode:
  the element under the pointer is outlined in the display list, pointer input
  goes to picking instead of the app, a click picks (its line to the clipboard, the pick in the
  next `Frame`) and ends picking, Escape ends it; wheels still scroll. It is an input, so a
  recording replays it. Debug shells toggle it with `mod+shift+c`, and print the pick.
- The accessibility label is the user's; a hand-written developer tag waits until these prove not
  enough.

## Text

Its own design comes first; what is decided:

- A text node holds runs: one string and styled ranges (family, size, weight, slant, color,
  decoration, a link). Blocks (paragraphs, headings, code blocks, list items) are nodes; inline
  styles are runs inside a block. Markdown renders to blocks of runs.
- Layout is per block from the shared text system; appending to the last block re-lays out that
  block only. Painting keeps each block's glyphs until the block changes.
- Selection is the document's, not a field's: a selectable region has an anchor and a focus
  addressed by stable keys (a row's key and a block path, and an offset), so it survives rows of
  a virtual list leaving and coming back; copying reads the text from the source.
- Accessibility: text runs, the selection, and live regions for streaming text.

## Incremental frames

- Painting keeps each subtree's items (shadows, quads, glyphs, paths) until that subtree's paint
  changes; the display list is assembled from kept parts. Accessibility geometry is sent for the
  nodes that moved, not by walking the tree.
- Proof: while a message streams, the glyphs emitted per frame grow with the tail, not the
  message; the accessibility nodes sent equal the nodes that changed.

## The platform boundary

- `Platform`: the loop and its waker; displays; windows (normal, dialog, popup); the clipboard
  (text, images, files); file dialogs; opening URLs; menus; the app's lifecycle.
- `PlatformWindow`: `present(Frame)` and the window's own requests (title, minimize, close).
  Rendering and the accessibility adapter belong to the backend, so an owned backend swaps
  whole. Input comes back as `InputEvent`s; focus in and out, menu picks, drops and close
  requests join them.
- The headless platform and every backend pass one conformance suite.

## Steps

1. Ownership and context. Implemented.
2. Input as data, frames out, recording and replay, windows in the protocol. Implemented.
2.5. What people point at: component boundaries, picking by point and in a picking window.
   Implemented.
3. The text core (its design first): runs, blocks, block layout, document selection, text runs and
   live regions in accessibility.
4. Incremental paint and accessibility.
5. The platform boundary completed: rendering and accessibility in the backend, cursor, drops,
   image clipboard, URLs, file dialogs; the winit backend with several windows (it shows one);
   the conformance suite.
6. Popups and menus ([windows](windows.md)).
7. The AI chat components and the AI chat reference app.

# The core: ownership, context, input and frames

Status: confirmed; in progress. Based on an architecture review against the first product (an AI
chat app), several windows, the [platform layer](platform.md) and agents; it supersedes the
single-window structure (one `App`, one `Kernel`, an implicit current window). There is no
compatibility layer: what this replaces is removed.

## What was wrong

- "The current window" was three mechanisms (a thread-local view context, an ambient
  `Rc<dyn Any>` on reactive nodes, a mutable `current` in the app), kept in step by hand.
- App-wide resources lived in each kernel: the theme, the text engine (fonts and shaping
  caches), the clock, reduced motion. Fonts registered in one window were missing in another.
- Windows were reachable from `&self` only through an unsafe stable pointer, and never freed.
- Input was some thirty method calls, the agent protocol another path: no record and replay, and
  every backend re-derives the frame, accessibility and cursor logic.
- Text was one string with one style per node; selection existed only in text fields; a
  streaming message re-emitted all its glyphs every frame.

## Ownership

| Owner | What |
| --- | --- |
| `App` (owned by the caller, `&mut`) | an `Rc<AppState>`; nothing else |
| `AppState` (one per app) | the reactive `Runtime`; the `Scheduler` and its clock; `Resources`; the `Platform`; the windows (`SlotMap<WindowId, Rc<Window>>`); window requests; the recorder; the capture painter |
| `Resources` (one per app, shared by every window) | the theme (one value, a change restyles every window), the text system (font collection, fallback, shaping and layout caches), the image store, the keymap and commands, tuning, the target platform's conventions |
| `Window` (one per window; today's `Ui` and `Window` merged) | its `Kernel` (tree, styles, layout, paint, semantics); interaction (focus, hover, presses, edits, selection); size, scale and safe area, with their signals; handlers; overlay slots; followed scroll states and sizes; after-layout hooks; its root view, scope and slots; its `PlatformWindow`; what its platform was last sent; a capture in progress |
| `Kernel` | the node tree and everything per node; it reads `Resources` (an `Rc`), it owns none of them |

A closed window leaves the slot map and is dropped once nothing holds it; its scope is disposed
first, so no binding, watch or task of it runs again.

## Context

- Two thread-locals, both set by the app's entry points and nothing else: the reactive runtime
  (as today) and the `AppState`.
- The reactive runtime carries one typed owner, `Owner(u64)`, not a view-layer value: every
  binding, memo, watch and task records the owner current when it is made and restores it when it
  runs. The view layer's owners are windows (`WindowId` is the owner's number).
- `window()` (crate-internal) is the current owner's window, looked up in `AppState.windows`; it
  replaces `ui()`. Code that runs for a window that has closed cannot happen (its scope is
  disposed); code outside any window that reaches for one is a bug and panics with what ran.
- Free functions stay the app's API (`state`, `spawn`, `open_window`, `current_window`, …): they
  resolve through this context, so components never pass a context around.
- Kernel borrows are never held across app code: handlers are looked up, the borrow released,
  then called (as today); debug builds check it.

## Driving an app

- `App::new(platform)` makes an app without windows; `app.open_window(options, view) ->
  WindowId` and `open_window(..)` from app code open them; `close_window(id)` closes one.
- One input entry: `app.event(window, InputEvent)`. `InputEvent` is data: pointer (with touch
  ids), key, text, composition, wheel, focus in and out, resize with scale and safe area,
  accessibility actions, native view events and snapshots, menu picks, drops, close requests.
  Headless the clock is virtual (`advance`); a backend feeds the wall clock.
- The recorder keeps `(time, window, InputEvent)`: a recorded session replays to the same
  snapshots and display lists on the virtual clock.
- `app.frame()` settles every window (tasks, the reactive flush once, then each dirty window's
  layout and after-layout hooks). `app.window_frame(window) -> Frame` gives what a window shows:
  the display list, the accessibility update (when a reader listens), native view changes and
  placements, the text input state, the cursor, the title. A backend presents it
  (`PlatformWindow::present`), then the next frame is requested by the app when something
  changes or moves.
- Agents and tests: the inspector protocol names a window in every request (default: the only
  one); its high-level steps (`tap`, `type`, …) lower to `InputEvent`s. `Harness` (in
  `octara::testing`) is a headless app with one window and the driving methods tests use.

## Text (decided here; its own design comes with step 3)

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
  whole. Input comes back as `InputEvent`s.
- The headless platform and every backend pass one conformance suite.

## Removed

`App`'s implicit window methods (they live on the window and the `Harness`), `App::with_window`,
the thread-local view context and the ambient value, `Ui`, the per-kernel theme, text engine
and clock, the shell's polling of app state, the scale passed per call, and the window storage
that could not free a window.

## Steps

1. Ownership and context: `AppState`, `Resources`, windows in a slot map, one typed owner,
   `window()`, `App::new(platform)` and `open_window` / `close_window` headless, the `Harness`;
   every test on it. Proof: two windows share a signal and update together; closing one
   disposes its bindings, watches and tasks and frees it (a thousand opens and closes leave
   nothing); a font registered once is used in both; theme changes restyle both.
2. Input as data and frames out: `InputEvent`, `app.event`, `Frame`, `present`, the recorder and
   replay; the inspector lowers to events and names windows. Proof: a recorded session replays
   to identical snapshots and display lists.
3. The text core (its design first): runs, blocks, block layout, document selection, text runs and
   live regions in accessibility.
4. Incremental paint and accessibility.
5. The platform boundary completed: rendering and accessibility in the backend, cursor, drops,
   image clipboard, URLs, file dialogs; the winit backend with several windows; the conformance
   suite.
6. Popups and menus ([windows](windows.md) steps 4 and on).
7. The AI chat components and the AI chat reference app.

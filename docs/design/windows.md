# Windows and popups

Based on the [research](../research/windows-and-platform.md). Status: confirmed; several windows
are implemented headless, in the protocol, in captures and in the winit backend; requests (menus,
prompts, file dialogs) are implemented, drawn in the window where the platform shows none.
Ownership and context are [the core's](core.md).

## Model

- One `App` holds the one reactive runtime and its windows. A window owns what is its own: its
  `Kernel` (root, viewport, overlay layer), interaction state (focus, hover, presses, edits),
  size, scale, its viewport and breakpoint signals, its platform window and what was presented
  to it, and a capture in progress. The kernel does not know other windows.
- Each window has its own root scope, named by the window (state is kept by scope path, so two
  windows' roots never share a key): closing a window disposes its scope only. Signals, lists
  and resources made outside a window's view (`App::run`) are the app's and shared by every
  window.
- What reads "the window" (`window_size()`, breakpoints, focus commands, `current_window()`)
  means the window whose view is being built, whose event is being handled or whose task runs.

## API (GPUI's names, Octara's free functions)

- `App::open_window(WindowOptions, || View) -> Result<Window, String>`; from app code,
  `open_window(..) -> Result<WindowId, String>` (built at the end of the frame),
  `close_window(id)`, `windows()`, `current_window()`. `WindowOptions`: title, size, kind.
- `WindowKind::{Normal, Dialog}`; `Popup(PopupOptions)` with `PopupOptions { parent,
  anchor_rect, anchor, gravity, constraint_adjustment, offset, grab }`, placed by the platform,
  comes with popups. `on_window_closed(..)` comes with close requests.

## Popups and menus

- Popups (popovers, selects, tooltips) are overlays drawn in the window, on every platform (as
  Rune's and GPUI's on macOS and Windows); OS popup windows only where the platform needs them
  to leave the window's bounds (Wayland `xdg_popup`), later.
- What the app asks a person is a request ([`octara::ask`]): `popup_menu(menu, at)` (Rune's
  name) gives the id of the item picked, `prompt(level, message, detail, answers)` (GPUI's) the
  index of the answer, the file dialogs their paths; `None` when dismissed; a task awaits each.
  A request is its window's state (`Window::asks`, the snapshot's `asks`); the window presents it
  to its platform window when that shows the kind (`shows_ask`), else draws a menu or a prompt
  itself; the answer comes back as input (`InputEvent::Answer`), recorded and replayed, and
  tests and agents answer with `Window::answer` or the `answer` request, or press a drawn item.
- Drawn: a window's asks layer, mounted after its root, holds a menu at its point (an overlay
  placed as the room allows, a modal: a press outside or Escape dismisses it, arrow keys move,
  focus returns where it was; items `menu:{id}`, submenus open to the right) and prompts over a
  scrim, mounted only while one is open (answers `prompt:{i}`, the last the default and focused;
  Escape dismisses). A menu item shows its keys as the platform writes them (`⌘C`, `Ctrl+C`).
- Native on macOS: a menu is an `NSMenu` popped up at its point, a prompt an `NSAlert` sheet on
  the window (the last answer the default, a `Cancel` answer on Escape), a file dialog rfd's
  sheet; menus and alerts open from the main run loop after the loop's callback returns (AppKit
  tracks a menu in a loop of its own; winit's handler must not be entered again from it). A
  withdrawn request closes (`cancelTracking`, `endSheet`). After an answer the pointer is
  sampled again (no move came while the menu had it). Windows' native menus and alerts come
  later (drawn there now), with the menu bar's items running commands (GPUI's model).
- Tooltips are drawn in the window.

## Agents

- The inspector's snapshot names its window and lists the app's (`window`, `windows`: number,
  title, size); requests take an optional `window` (default: the window the protocol was given).
  Test ids are per window. A popup is a window whose parent is named.

## Steps

1. Several windows headless; windows in the inspector protocol and captures. Implemented.
2. Several windows in the winit backend; the shell routes events by window. Implemented.
3. Requests: the mechanism, file dialogs on it, menus and prompts drawn in the window.
   Implemented. Native menus and alerts on macOS. Implemented. Then the macOS menu bar; Windows'
   native menus and alerts, Wayland popups later.

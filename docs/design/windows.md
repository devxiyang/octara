# Windows and popups

Based on the [research](../research/windows-and-platform.md). Status: confirmed; several windows
are implemented headless, in the protocol and in captures; the winit backend shows one window;
popups and menus are not implemented. Ownership and context are [the core's](core.md).

## Model

- One `App` holds the one reactive runtime and its windows. A window owns what is its own: its
  `Kernel` (root, viewport, overlay layer), interaction state (focus, hover, presses, edits),
  size, scale, its viewport and breakpoint signals, its platform window and what was presented
  to it, and a capture in progress. The kernel does not know other windows.
- Each window has its own root scope: closing a window disposes its scope only. Signals, lists
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

- In-window overlays stay the default and the fallback on every platform; a popup becomes its
  own OS window where the platform supports it and it must leave the window's bounds (Wayland
  `xdg_popup`, macOS panels), from the same view.
- Dismissal is an event from the popup (a grab does not see clicks inside the app); the focus
  before it opened returns when it closes; a modal dialog blocks its parent's input.
- Menus (Rune's): asking for one answers the picked id or `None`; native on macOS and Windows,
  drawn elsewhere. Headless, an open menu stays in the tree until `menu_pick` / `menu_cancel`.
- Tooltips are drawn in the window; a window only when one must leave the window's bounds.

## Agents

- The inspector's snapshot names its window and lists the app's (`window`, `windows`: number,
  title, size); requests take an optional `window` (default: the window the protocol was given).
  Test ids are per window. A popup is a window whose parent is named.

## Steps

1. Several windows headless; windows in the inspector protocol and captures. Implemented.
2. Several windows in the winit backend; the shell routes events by window.
3. Native menus (macOS, Windows), then OS popups (Wayland, macOS panels).

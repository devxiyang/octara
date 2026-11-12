# Windows and popups

Based on the [research](../research/windows-and-platform.md). Status: confirmed; in progress. The
ownership and context it runs on are [the core's](core.md), which supersedes step 1 below.

## Model

- One `App` holds the one reactive runtime and its windows: `windows: SlotMap<WindowId,
  Window>`. A `Window` owns what is the window's own: its `Kernel` (root, viewport, overlay
  layer), interaction state (focus, hover, presses, edits), size, scale, its viewport and
  breakpoint signals, its native host and what was sent to it, its accessibility state and a
  capture in progress. The kernel does not know other windows.
- Each window has a root scope under the app's scope: closing a window disposes its scopes only.
  Signals, lists and resources are the app's and shared by every window.
- What reads "the window" (`window_size()`, breakpoints, focus commands, `open_window`'s parent)
  means the window whose view is being built or whose event is being handled.

## API (GPUI's names, Octara's free functions)

- `open_window(WindowOptions, || View) -> WindowHandle`; `WindowHandle::close()`,
  `windows()`, `on_window_closed(..)`. `WindowOptions`: title, size, kind.
- `WindowKind::{Normal, Dialog, Popup(PopupOptions)}`; `PopupOptions { parent, anchor_rect,
  anchor, gravity, constraint_adjustment, offset, grab }`, placed by the platform.

## Popups and menus

- In-window overlays (today's) stay the default and the fallback on every platform; a popup
  becomes its own OS window where the platform supports it and it must leave the window's
  bounds (Wayland `xdg_popup`, macOS panels), from the same view.
- Dismissal is an event from the popup (a grab does not see clicks inside the app); the focus
  before it opened returns when it closes; a modal dialog blocks its parent's input.
- Menus (Rune's): asking for one answers the picked id or `None`; native on macOS and Windows,
  drawn elsewhere. Headless, an open menu stays in the tree until `menu_pick` / `menu_cancel`.
- Tooltips are drawn in the window; a window only when one must leave the window's bounds.

## Agents

- The inspector's snapshot lists `windows`; every node names its window; requests take an
  optional `window` (default: the main window). Test ids are per window. A popup is a window
  whose parent is named.

## Steps

1. Move the window's own state from `App` into `Window`, one window, no change in behavior.
2. `open_window` for normal windows and dialogs; the shell routes events by window.
3. Windows in the inspector protocol and captures.
4. Native menus (macOS, Windows), then OS popups (Wayland, macOS panels).

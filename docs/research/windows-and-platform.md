# Windows, popups and the platform layer

How frameworks share one state across several windows and OS popups, and whether to keep winit
or own a platform layer per OS. Researched 2026-11; it backs the [windows](../design/windows.md)
and [platform](../design/platform.md) designs. Paths are in the local reference clones
(`~/code/archive/reference/{zed,rune,exact}`); items marked *unverified* come from secondary
sources only.

## Several windows, one state

- **GPUI**: `App` owns the entities and `windows: SlotMap<WindowId, Option<Box<Window>>>`
  (`gpui/src/app.rs:884-900`); each `Window` owns its platform window, frames and scale factor
  (`gpui/src/window.rs:1220-1269`). `open_window` takes a closure building the root view, so any
  window's views hold the same entities (`app.rs:1605-1618`); windows are reached by handle
  (`update_window`, `app.rs:3411`). Focus, hit testing and DPI are per window.
- **Iced** (one daemon state, `window::Id` passed to `view`), **Masonry/Xilem** (a render root
  per window): the same shape. **Flutter**'s new windowing API puts every window in one widget
  tree (behind a flag in 2026). **Rune** gives each window and overlay its own state type and
  runtime, sharing by messages (`rune/src/overlay.rs:1-3`). **Slint** globals are per window
  (issue #784). Exact has one surface; sheets and dialogs are top-layer content in it.
- One runtime above per-window trees keeps shared state free, gives each window its own focus,
  hit testing and accessibility tree, and lets a window be rebuilt or closed alone.

## Popups

- **GPUI** draws popovers, menus and tooltips inside the window by default (`deferred` and
  `anchored`, `gpui/src/elements/{deferred,anchored}.rs`; Zed's `ContextMenu` and
  `PopoverMenu`), dismissal an event from the popup. A native popup window is opt-in:
  `WindowKind::AnchoredPopup(PopupOptions { parent, anchor_rect, anchor, gravity,
  constraint_adjustment, offset, grab })` (`gpui/src/platform/popup.rs:5-50`), placed by the
  platform; Wayland uses `xdg_popup` with a grab, macOS and Windows reject it so callers fall
  back to in-window popovers (`gpui_windows/src/window.rs:445-448`). A grab does not cover
  clicks inside one's own app: closing then is the app's job.
- **Rune** shows native menus (`NSMenu`, `TrackPopupMenuEx`) where the platform has them and
  answers `None` elsewhere so the app draws its own (`rune/src/popup_menu/mod.rs:1-49`).
  Headless, an open menu stays in the tree until a script sends `menu-pick` or `menu-cancel`.
  Tooltips are AppKit's on macOS, drawn in the window elsewhere. Overlays are non-activating
  panels (`NSPanel`, layer-shell, `WS_EX_NOACTIVATE`).
- **Flutter** popup windows translate or shrink to stay on screen; tooltip windows take no focus;
  modal dialogs block their parent. **Compose** `Popup` is in-window, `DialogWindow` a window.
- Wayland forces anchor-relative popups with a parent; in-window drawing is the only option on
  the web and mobile.

## Agents and several windows

- Rune's headless tools take `--window KEY` and `--list-windows`; its tree lists every window's
  frame. No other framework found exposes a window id in its test protocol.

## winit, or an own platform layer

- **winit 0.30.13**: IME is events only, `Preedit` and `Commit` with a cursor area
  (`winit/src/event.rs:746`): no surrounding text, replacement range or per-range rects. Child
  windows only through a parent handle; no popup kind.
- **winit 0.31** (beta, breaking): `user_event` becomes `user_wake_up`, `can_create_surfaces`
  for mobile surface loss, real Wayland child windows, the repository split into platform crates.
  Per Rune's notes on beta.3 (*unverified*): macOS `selectedRange` still returns `NSNotFound`,
  `winit-uikit` implements `UIKeyInput` only, no live-resize transaction sync, no popups or
  menus.
- **Reaching around winit**: Rune pins 0.30 and patches winit's own AppKit classes at run time
  (`rune/src/platform/macos/panel_bridge.rs`, 357 lines); its plan is "winit to start, own thin
  shells as soon as text input lands". Octara's AppKit code already outgrew its winit shell.
- **GPUI** owns its platforms: `Platform` (about 80 methods) and `PlatformWindow` (about 104;
  `gpui/src/platform.rs:394, 1274`), an `InputHandler` modeled on NSTextInputClient
  (`platform.rs:2364`), about 53k lines across macOS, Windows, Linux and iOS. Makepad and
  Flutter's embedders also own their shells; Slint has winit as one pluggable backend; Iced,
  egui, Xilem, Floem and Blitz are on winit with weak IME.
- **AccessKit** adapters, **muda** (menus), **rfd** (dialogs) and **tray-icon** take raw handles
  or nothing: they work with either choice.

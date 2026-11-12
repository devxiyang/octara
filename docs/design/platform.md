# The platform layer

Based on the [research](../research/windows-and-platform.md). Status: confirmed; the traits, the
headless platform, the winit backend behind them (several windows, frame clocks, the pointer,
URLs, the clipboard with images, file dialogs, drops, activation) and the conformance checks are
implemented; the owned macOS backend is not.

The app and the kernel never see a windowing library's types. A platform backend implements a
narrow set of traits (GPUI's names); headless runs (tests, agents, `octara run`) implement the
same traits. Input reaches the app as [`InputEvent`s](core.md#driving-an-app) and each window's
platform is presented a `Frame` after each operation.

## Traits

- `Platform`: runs the loop and wakes it; displays; opens windows (normal, dialog, popup with a
  parent and an anchor); menus (application and context menus, their actions and validation);
  file dialogs, revealing paths, opening URLs; the clipboard; appearance and cursor; the app's
  lifecycle (suspend, resume, memory warnings); the keyboard layout.
- `PlatformWindow`: bounds, content size, scale, appearance, safe-area insets; title, minimize,
  zoom, fullscreen, activate; resize, close, active status; `present(Frame)`: native views'
  changes and placements, the focused field's text input (its caret for the input method,
  secure, multi-line), whether to draw; the soft keyboard; the accessibility tree; surface loss
  and recovery. Questions it answers: whether it shows a kind of native view, a view's snapshot,
  whether a view is busy.
- `InputHandler` (NSTextInputClient's model, as GPUI's): text for a range, the selected and
  marked ranges, replace text in a range, replace and mark, unmark, bounds for a range, the
  character index at a point. Backends without a direct text protocol (winit's IME events) feed
  it the same way.

## Implemented

- `octara::platform`: `Platform` (the clipboard's text, the waker, `open_window`) and
  `PlatformWindow` (`present`, `shows_native`, `snapshot_native`, `native_busy`); `Frame`,
  `TextInput`, `WindowOptions` (title, size, `WindowKind::{Normal, Dialog}`).
- `HeadlessPlatform` and `HeadlessWindow`: what `Window::headless` runs on. A headless window
  keeps the native changes it was presented, the text input, a count of frames, and can be told
  to show native views, take snapshots or be busy (`HeadlessPlatform::with_window`).
- `App::new(platform)` takes a backend's platform, whose `open_window` makes each window's
  platform window; `App::platform()` and `Window::platform_window()` reach them.
- The shell's `WinitPlatform` (arboard, the event loop's proxy) and `WinitWindow` implement
  them. The platform opens a winit window inside the loop callback the app runs in (the shell
  holds the active loop for each callback); every window has its own surface (one adapter and
  renderer for all), accessibility adapter, native view host, input method position and secure
  input, and frame clock. The app's `open_window` and `close_window` open and close real
  windows; the loop ends with the last one. The enum of platform conventions is
  `TargetPlatform` (`App::target_platform`).
- Frames: a window draws when the app presents something new (`Frame::redraw`, native
  changes). After a quiet spell (no frame for a refresh of its display) it draws at once; within
  a burst, on its frame clock's next tick: on macOS its view's display link (macOS 14), which
  ticks while a frame waits and pauses otherwise, skipping ticks closer than a 60 Hz frame in Low
  Power Mode (the link reports the panel's rate); elsewhere winit's redraw requests, paced by
  the compositor's frame callbacks (Wayland, through `pre_present_notify`) or the swap chain's
  blocking present. Metal's drawable pool is no frame clock (it does not hold back while the
  layer presents with its transaction), as Rune found.
- The pointer's look: `CursorStyle` (GPUI's names) set on an element with `.cursor(..)`
  (`cursor_pointer`, `cursor_text`, `cursor_grab`, …) for it and its descendants; else the
  platform's conventions: the I-beam over a text field or selectable text, the hand over a link,
  the arrow elsewhere (buttons included). Each `Frame` carries the look over the hovered element.
- URLs: `open_url(url)` asks the system to open it (the shell: the `open` crate; headless:
  recorded). A link that no `on_link` above it takes opens this way.
- The clipboard holds text, an image or both: `ClipboardItem` (`new_string`, `new_image`, `text`,
  `image`); `write_to_clipboard(item)` and `read_from_clipboard()` (GPUI's names); the
  platform's `read_text` / `write_text` and `read_image` / `write_image` (the shell: arboard).
- Requests ([`octara::ask`]): what an app asks a person (a menu, a prompt, paths to open, where
  to save) is its window's state: `Window::asks` and the snapshot's `asks` list it. A window
  presents a request in its `Frame` (`asks`) when its platform window shows that kind
  (`shows_ask`), and tells it when one no longer waits (`withdrawn`); the answer comes back as
  input (`InputEvent::Answer { id, answer }`), so a session that answered one replays, and tests
  and agents answer with `Window::answer` or the `answer` request. A task awaits it:
  `prompt_for_paths(PathPromptOptions { files, directories, multiple, prompt })` and
  `prompt_for_new_path(directory, name)` (GPUI's names), `None` when cancelled. The shell shows
  file dialogs with rfd (made on the main thread, awaited on a thread of their own, the answer
  sent through the loop's proxy).
- Files dragged from the system arrive as `InputEvent::FileDrag` (`Over` a point with their
  paths, `Drop`, `Leave`): the element at or above the point with `on_drop` is `DRAG_OVER`
  (styled by `drag_over`), and gets the paths when they drop. The shell turns winit's
  per-file events into one drop, and asks AppKit where the pointer is while files are dragged
  (AppKit sends no moves then).
- Activation: a backend sends `InputEvent::Active` when a window becomes the active one or stops
  being (the shell: winit's `Focused`); `Window::is_active` (GPUI's `is_window_active`). A
  window that is not active shows no caret in its focused field and wakes for no blink.
- Conformance: `octara::conformance::run(app)` checks the contract through what the app sees
  (the clipboard's text and images, restored afterwards; windows opening and closing; each
  operation that changes something presenting a frame and an idle clock none; activation; the
  pointer's look). The headless platform runs it in tests; the shell with
  `OCTARA_CONFORMANCE=1` (CI on macOS and Windows). URLs and file dialogs, which need a person
  or act outside the app, are not in it.
- Menus: `set_menus` on the platform shows the app's menus with their states and keys filled in;
  a pick comes back as `InputEvent::Command` (see [windows](windows.md#popups-and-menus)).
- Not yet: `InputHandler`, displays, lifecycle.

## Backends

1. winit 0.30: macOS, Windows and Linux; raw handles for AppKit views, DirectComposition, `muda`
   and `rfd`. winit 0.31 is evaluated when it is released.
2. macOS, owned (NSWindow, an NSView with NSTextInputClient, menus, live resize in the frame's
   transaction, panels for popups), sharing Apple code with a later iOS backend.
3. Windows and Linux stay on winit until direct text protocols (TSF, text-input-v3) become a
   priority; Android as winit's until it proves inadequate.
- AccessKit's platform adapters are used directly where the backend is ours.

## Steps

1. The traits, the headless platform, the winit backend behind them. Implemented.
2. The owned macOS backend.

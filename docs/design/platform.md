# The platform layer

Based on the [research](../research/windows-and-platform.md). Status: confirmed; step 1 implemented.
[The core](core.md) moves the boundary to frames out (`PlatformWindow::present`) and input in
(`InputEvent`).

The app and the kernel never see a windowing library's types. A platform backend implements a
narrow set of traits (GPUI's names, about 45 methods to start); headless runs (tests, agents,
`octara run`) implement the same traits.

## Traits

- `Platform`: runs the loop and wakes it; displays; opens windows (normal, dialog, popup with a
  parent and an anchor); menus (application and context menus, their actions and validation);
  file dialogs, revealing paths, opening URLs; the clipboard; appearance and cursor; the app's
  lifecycle (suspend, resume, memory warnings); the keyboard layout.
- `PlatformWindow`: bounds, content size, scale, appearance, safe-area insets; title, minimize,
  zoom, fullscreen, activate; input in (`PlatformInput`), resize, close, active status, frame
  requests; `set_input_handler` and the text input configuration (secure, multi-line, the
  return key, the content type), the input method's position, the soft keyboard; the
  accessibility tree; the native view host; `draw(frame)`, where the frame and the native views'
  placements reach the screen together; surface loss and recovery.
- `InputHandler` (NSTextInputClient's model, as GPUI's): text for a range, the selected and
  marked ranges, replace text in a range, replace and mark, unmark, bounds for a range, the
  character index at a point. Backends without a direct text protocol (winit's IME events) feed
  it the same way.
- What existed became part of it: the clipboard, the native host, the waker.
- As built (step 1, `octara::platform`; frames since [the core](core.md)'s step 2): `Platform`
  (the clipboard, the waker, `open_window`) and `PlatformWindow` (`present(Frame)`: native
  changes, the focused field's text input, whether to redraw; `shows_native`,
  `snapshot_native`, `native_busy`); `HeadlessPlatform` and
  `HeadlessWindow` are `Window::headless`'s; `App::new(platform)` takes a backend's, whose
  `open_window` makes each window's; `App::platform()` and `Window::platform_window()` reach
  them. The shell's
  `WinitPlatform` (arboard, the event loop's proxy) and `WinitWindow` (the native host, the
  input method's position, secure input) implement them: the app no longer installs a clipboard,
  a waker or a host, and the shell no longer polls the caret. The enum of platform conventions
  is `TargetPlatform` (`App::target_platform`). Opening windows joins with
  [windows](windows.md) step 2; `InputHandler` and a `PlatformInput` stream with the owned
  macOS backend.

## Backends

1. winit 0.30 (today's shell, behind the traits, behavior unchanged): macOS, Windows and Linux;
   raw handles for AppKit views, DirectComposition, `muda` and `rfd`. winit 0.31 is evaluated
   when it is released.
2. macOS, owned (NSWindow, an NSView with NSTextInputClient, menus, live resize in the frame's
   transaction, panels for popups), sharing Apple code with a later iOS backend.
3. Windows and Linux stay on winit until direct text protocols (TSF, text-input-v3) become a
   priority; Android as winit's until it proves inadequate.
- AccessKit's platform adapters are used directly where the backend is ours.

## Steps

1. The traits, the headless platform, the winit backend behind them.
2. The owned macOS backend.

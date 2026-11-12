# Captures

Based on the [research](../research/capture.md). Status: confirmed; implemented (the window server's picture awaits a check with the screen unlocked).

An agent's picture of the app: what the window shows, native views included, with a report of
what the picture can be trusted for. One request, the same on every platform and headless.

## Request

- `{"op":"capture","path":"shot.png"}` in the inspector protocol (`octara run`, and the window's
  agent channel later). Options: `settle_ms` (how long to wait for the app to settle, default
  5000), `scale` (device pixels per logical pixel; default the window's, 2 headless), `pixels`
  (`painted`, the default, or `display`).
- The reply carries a report (`capture`):
  - `path`, `width`, `height`, `scale`;
  - `pixels`: `painted` (Octara composed it) or `display` (the window server's picture), and
    `fallback`: why `display` was asked and `painted` given;
  - `renderer`: `gpu` (the window's renderer, offscreen), `reference` (headless), or `display`;
  - `settled`, and `pending`: what was still running when the wait ended (tasks, motion,
    native views loading);
  - `native`: one entry per native view shown: its test id, kind and rect, `shown` (`snapshot`,
    `placeholder`, `failed` or `live`), a `reason` when not a snapshot, and `misses`: what the
    view's snapshot cannot show (WebKit: hardware video and WebGL come out black).
- `CAPTURE_FAILED` when no picture can be made (no painter, an unwritable path).

## Painted (default)

1. Settle: run until no task, timer or motion is left (subscriptions do not count) and no
   native view is busy (`PlatformWindow::native_busy`: a page loading), at most `settle_ms`. Headless this
   runs on the app clock; in a window, on the wall clock.
2. Snapshots: the platform window is asked for each shown view's snapshot
   (`PlatformWindow::snapshot_native`, which
   says what the snapshot misses, or why it cannot take one); each comes back through
   `InputEvent::NativeSnapshot` within 5 s or the view is reported `failed`.
3. Paint: the display list, with each snapshot drawn in its view's place, rendered by the
   window's GPU renderer offscreen (the renderer that ships) or, headless, the reference
   renderer; written as an 8-bit sRGB PNG. The snapshots are cleared afterwards, so the live
   frame never shows a stale one.

Works without permission, headless, and with the screen locked or the display asleep (when
the window server draws nothing). Secure fields are painted as bullets as always.

## Display

`pixels: display` asks for the window server's picture of the app's own window: what the
user sees, composition timing, z-order and system materials included. macOS: in-process
`CGWindowListCreateImage` (no permission for one's own window; obsoleted in the SDK, so found at
run time), after the frame's transaction is flushed and two refreshes have passed. Windows:
Windows.Graphics.Capture of the own window (later). Where it is unavailable (headless, Wayland,
a locked screen or a sleeping display, an empty picture) the capture is painted and `fallback`
says why. Native views are `live` in it.

As built (macOS): the latest frame is presented, and 40 ms later (two refreshes and a margin)
the content view's rect in global display coordinates is taken from the window server
(framing ignored, best resolution). It falls back when the session's screen is locked
(`CGSSessionScreenIsLocked`), the symbol is gone, the window is not visible, or the picture's
size is not the window's (a sleeping display).

## Entry points

- The inspector protocol (`octara run --script`, interactive `octara run`): headless, reference
  renderer; the harness links `octara-render` for it.
- A window: `OCTARA_CAPTURE=shot.png` (with `OCTARA_CAPTURE_AFTER_MS`, `OCTARA_CAPTURE_SCROLL`,
  `OCTARA_CAPTURE_PIXELS=display`) captures once and exits, printing the report as JSON; the
  window's agent channel (Native Island step 4) will take the same request.

## Steps

1. The request, the report and painted captures headless (reference renderer); `busy` and
   snapshot misses in the platform window.
2. In a window: wall-clock settling, busy views, snapshot deadlines, the GPU renderer offscreen.
3. `pixels: display` on macOS, with the fallback.

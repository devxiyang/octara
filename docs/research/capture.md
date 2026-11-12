# Captures with native views

How frameworks and tools take a picture of a window that holds platform views (web views,
maps, video), and what each platform allows an app capturing its own window. Researched
2026-11; it backs [the capture design](../design/capture.md). Items marked *unverified* came
from secondary sources only.

## Strategies

| Strategy | Used by | Misses | Permission |
|---|---|---|---|
| A. The app composites each view's own snapshot into its frame | Exact (default), wry PR #1674, Electron `capturePage`, Flutter native plugins | what a view's snapshot cannot draw: WebKit's snapshot renders hardware video and WebGL black ([WebKit bug 198107](https://bugs.webkit.org/show_bug.cgi?id=198107), open since 2019); each host's timing (WebView2 `CapturePreview` fails before the first `ContentLoading` and may show the previous page after a navigation) | none |
| B. The window server's picture of the app's own window | Exact (`window: true`, macOS), Android `PixelCopy`, Windows.Graphics.Capture | nothing is drawn while the screen is locked or the display asleep; protected content is blanked (`FLAG_SECURE`, hardware-protected surfaces); Wayland needs a portal prompt | varies (below) |
| C. Read back the app's own GPU frame only | GPUI (`render_to_image`), Rune (headless readback) | native views are outside the surface | none |
| Keep native views out | Rune ("native views never appear in offscreen output, so goldens stay deterministic"), SwiftUI `ImageRenderer` (placeholders for representables) | everything native | none |

## Prior art

- **Exact** (local clone, `host/apple`). Agent protocol over JSON lines; `screenshot` writes a
  PNG and returns metadata. The default capture draws the app's own hierarchy with every web
  view and opted-in native module (`snapshot: true` per tag) composited from its own snapshot
  at its node, live views hidden. Snapshots are asynchronous with a 5 s deadline per view; a
  failure is logged and the view skipped. Web snapshots retry three times 120 ms apart.
  Before capturing it settles: waits up to 3 s for images, settles motion, and reports what
  did not settle (`imagesPending`, `moving`). `window: true` (macOS) uses the window server
  (`CGWindowListCreateImage` through `dlsym`, obsoleted in the SDK) after flushing the
  transaction and waiting two refreshes; an empty picture (locked, asleep) is an error. On a
  simulator a driver can take the display's picture instead, gated by checks, and the reply
  says which it used (`pixels: display|app`, with a reason). Secure fields are hidden in the
  accessibility tree only, not in pixels.
- **Rune**. Native materials sit under the Metal layer and never appear in captures; captures
  are wgpu offscreen renders read back on a virtual clock. Its agent research rejects a
  screenshot-only agent API (structured tree first) and a software renderer standing in for the
  GPU one.
- **GPUI**. `Window::render_to_image` renders the scene to an offscreen Metal texture and
  reads it back (test support); no native views. Video (`surface`) is part of the scene.
- **Flutter**. `RenderRepaintBoundary.toImage` cannot capture platform views
  ([#83856](https://github.com/flutter/flutter/issues/83856)); `integration_test` takes a
  device-level screenshot (Android needs `convertFlutterSurfaceToImage` first).
- **React Native** (`react-native-view-shot`). Blank captures for WebView, video and maps are
  long-standing issues; `captureScreen()` takes a device screenshot instead.
- **SwiftUI `ImageRenderer`**. Renders only SwiftUI content; WKWebView and maps come out as
  placeholders ([forum](https://developer.apple.com/forums/thread/736236)).
- **Electron**. `webContents.capturePage` snapshots the web contents, not the window.
- **Tauri / wry**. No shipped API; PR #1674 adds `WebView::screenshot` from each platform's
  web snapshot (WKWebView's needs no permission). An earlier OS-level attempt captured "every
  window except the WebView" (PR #1258).

## Own-window capture per platform

- **macOS** (measured on macOS 27, this machine): in-process `CGWindowListCreateImage` of the
  app's own window returned its pixels with `CGPreflightScreenCaptureAccess()` false, so no
  Screen Recording permission is needed, as Exact's comment says. ScreenCaptureKit
  (`SCScreenshotManager`, even with `SCShareableContent.currentProcess`) failed without the
  permission (error -3811). `screencapture` (another process) fails without it. With the screen
  locked, the window server returned a 158 × 214 blank picture instead of the window, and the
  app presented no frames. Whether the window server's picture includes the CAMetalLayer and
  WKWebView's out-of-process layers is still to be measured with the screen unlocked (Exact
  says Metal layers are included).
- **iOS**: `drawViewHierarchyInRect:afterScreenUpdates:` (Exact's base, with web snapshots
  composited); *unverified* which layers it misses (Exact: remote layers rasterize blank).
- **Android**: `PixelCopy.request(Window)` copies the window's last buffer; protected sources
  fail ([docs](https://developer.android.com/reference/android/view/PixelCopy)).
- **Windows**: Windows.Graphics.Capture shows a border unless the user grants borderless access
  ([docs](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.isborderrequired));
  `PrintWindow` reportedly returns black for WebView2 (*unverified*). WebView2's own
  `CapturePreview` captures the page.
- **Linux**: Wayland has no own-window capture; the xdg-desktop-portal screenshot asks the user.
  WPE renders the page into buffers the app already holds.

## What this means for Octara

- Strategy A is the only one that works everywhere without permission, headless, and with the
  screen locked (the usual case for an agent working remotely): it is the default.
- B shows what the user sees (composition timing, z-order, system materials) where the
  platform allows it; it must fall back to A and say why.
- The painted frame should come from the GPU renderer in a window (what ships), the reference
  renderer headless.
- A capture is a report, not only pixels: whether the app settled, what each native view shows
  and what its snapshot cannot show.

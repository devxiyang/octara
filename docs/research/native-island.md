# Native Island research

Embedding platform-native views (maps, web views, video, camera, platform pickers) in Octara's
layout. Sources: the reference clones (Exact, Rune, Zed/GPUI) and general knowledge of Flutter,
React Native, Compose, SwiftUI and desktop toolkits (marked "known" below; not re-verified).

## The references

- **Exact** (`docs/recipes/native-views.md`, `host/apple/modules/ExactNativeViews.swift`): a
  native view is a Swift view the host puts in the node's box. The instance gets the whole props
  object on every change (`setProps`), reports an `intrinsicSize` (an authored size wins), names
  an optional `focusTarget` (the host owns first responder), takes synchronous `agentInput` from
  tools, gives a PNG `snapshot()`, and ends with `destroy()`. Events go back as message / press /
  change / load. Time is the session's clock, never a native timer, so a test clock moves it.
  Touches the view declines fall to the box underneath. Accessibility: a silent view is one
  element under its box's label with traits from its role; a view with parts sets its own
  elements and the box becomes a group. The Linux host's frame command for a native view is a
  rect plus eight corner radii.
- **Rune** (`research/11-platform-shells.md`, `PLAN.md`; design only, nothing implemented):
  rejects Flutter-style arbitrary interleaving. Two slots per window: an **underlay** behind a
  transparent Metal layer, with an alpha-0 cutout (and corner radius) punched in the frame
  (video layers, maps, glass materials), and an **overlay** child view above it, synced on the
  main thread inside the same `CATransaction` (web views, pickers, popovers). A `NativeSurface`
  primitive carries zero-copy video and camera frames (IOSurface). Rune rejects web views for
  generated UI.
- **GPUI**: no general embedding. `surface(CVPixelBuffer)` is an ordinary element painted as a
  texture (sorted with quads and text, sampled through `CVMetalTextureCache`): video is a scene
  primitive, not a view. Text input is the window view's own `NSTextInputClient` (macOS) and the
  equivalent on iOS, not a hosted native field.

## Verified from source (2026-11)

A second pass read the clones line by line.

- **Rune**, implemented with tests: the **underlay** for system materials
  (`crates/rune/src/system_material/macos.rs`). The CAMetalLayer is a sublayer of winit's
  content view and non-opaque (post-multiplied alpha); a flipped host `NSView` is inserted below
  every subview, returns nil from `hitTest:` (materials take no input) and holds the material
  views, built at alpha 0; the Metal layer is kept above the host (`zPosition` 1). The hole is
  made in paint order by **omission**: the element's own fill is skipped and its shadow gets a
  rounded hole (`runtime/paint/build.rs`), so children and later content draw over the
  material, and anything opaque painted before it hides it. One corner radius (continuous
  curve), ancestor clips ignored, at most 4 materials.
- **Rune, frame sync measured** (`research/26`, 120 Hz, frame-by-frame screen capture): view
  frames set and the drawable presented inside one `CATransaction` with
  `presentsWithTransaction`: 1.8 % of moving frames off by more than 4 px (every miss one frame
  ahead); unsynced: 29 %. `presentsWithTransaction` is owned by one flag ORing its causes
  (materials present, window move, live resize) and is on only while something moves: it costs
  a wait for the GPU to schedule each frame.
- **Rune, not implemented**: the overlay slot, `WKWebView`, `MKMapView`, `AVPlayerLayer` (research
  prose only). Video is a texture: `NativeSurface` imports IOSurface-backed `CVPixelBuffer`s
  through `CVMetalTextureCache` into wgpu-hal, drawn in its own pass with clips, radii and
  opacity. Flutter-style interleaving is rejected (extra full-size Metal layers, clip masks,
  touch buffering across threads).
- **Exact** never faces a native view over a GPU surface on Apple: every node is an AppKit or
  UIKit view and a native view is a subview of its node's view (AppKit hit testing, main-thread
  layout, no extra sync). Its Metal-painted Apple carrier is a benchmark without native views.
  On Android (one painted canvas) a native view is attached off screen and its drawing recorded
  into the canvas at the element's place (`RenderNode`), so it interleaves, clips and
  transforms. Worth taking: a `snapshot()` per view (PNG) that captures draw in place of the
  live view; views created when near the viewport and hidden when far (a map is 110–135 MB on
  Android); a focus target put into the Tab order; an input channel for agents
  (`agentInput`); per-corner radii (the Linux frame command carries eight values); events from
  any thread hopped to the main one, stale ones dropped.

## Elsewhere (known)

- **Flutter**: Android virtual display (offscreen texture; touch, a11y and text input are
  fragile), hybrid composition (the real view in the hierarchy; thread merging, one-frame sync
  risk), texture layer (fast, only for views that tolerate it; the default now). iOS always uses
  the real `UIView` with overlay layers and a mask per clip: exact z-order, but costly, with
  delayed touches.
- **React Native Fabric**: every component is a native view; no shared canvas.
- **Compose `AndroidView` / SwiftUI `UIViewRepresentable`**: the parent layout gives the view a
  frame; clipping is a rect or rounded rect; the view draws above or below the canvas.
- **Slint, Iced, Dioxus**: no first-class embedding; `wry::WebView::build_as_child` with
  `set_bounds` over a winit window is the common web view route.
- **Desktop child views over a wgpu surface**: a sibling view above the GPU layer always draws
  on top; behind it needs a transparent layer; clipping is a rect or corner radius only; GPU
  content cannot interleave with it; frame updates and the present must land in one transaction
  or the view lags a frame. Windows `HWND` children have airspace problems; DirectComposition
  visuals are the better route.
- **Texture sharing**: Apple `CVPixelBuffer` / `IOSurface` into Metal, then wgpu through
  `wgpu-hal`; Android `AHardwareBuffer` into Vulkan, or `SurfaceTexture`. Zero-copy video and
  camera.

Not re-verified (from memory): wry, Avalonia `NativeControlHost`, Compose desktop `SwingPanel`
and Compose iOS `UIKitView` default to native views above the content, rectangular clips, and
often lag a frame on moves; Flutter iOS splits its output into overlay Metal layers around a
real `UIView` (full interleaving, high cost); WebView2 in visual hosting mode and Chromium's
surface layers composite web content as part of the compositor tree; a `WKWebView` snapshot
takes tens of milliseconds (screenshots, not frames).

## Conclusions

1. **Video and camera are textures** (GPUI's `surface`, Rune's `NativeSurface`): they clip,
   animate and interleave like any image. Only what cannot be a texture is a view.
2. **Views live in two slots per window** (Rune): an underlay (maps, materials) behind a
   transparent cutout, and an overlay (web views, pickers, a platform text field) above. No
   interleaving with Octara content; an element painted over an underlay or under an overlay is
   a `check` finding.
3. **Placements are data**: after layout, each island has a rect, corner radii, a clip and a
   slot; the shell applies them on the main thread in the same transaction as the frame. A clip
   that is not a rect or rounded rect cannot be honored (hide, or show a snapshot, and report).
4. **Props are one value, events are typed, time is the app's clock** (Exact).
5. **Headless**: a placeholder box with its semantics and test id, a fake handle that records
   props and placements and lets tests send events; `inspect` shows the kind, props and
   placement.
6. **Accessibility**: the island is a node with role and label; the platform view's own subtree
   is joined where the adapter allows, else it is one labeled element, and `check` asks a label.
7. **Order**: placement plumbing and the headless fake; the macOS shell's overlay and underlay
   with a web view; focus handoff and accessibility; the texture path with video; then iOS and
   Android.

## Windows and Linux (researched 2026-11)

### Windows: WebView2 in visual hosting

- `ICoreWebView2Environment3::CreateCoreWebView2CompositionController(hwnd)` creates a web view
  whose output goes to a DirectComposition visual given with `put_RootVisualTarget`; the host
  commits its own device afterwards ([controller](https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2compositioncontroller)).
  Windowed hosting (a child HWND, what wry uses) gets input, focus, IME and accessibility from
  the system but cannot sit below the app's surface or move in its transaction.
- In visual hosting the host forwards input: `SendMouseInput` (client coordinates; wheel in
  screen coordinates; `LEAVE` too; capture on press), `SendPointerInput` for touch and pen,
  the cursor from `CursorChanged`, focus with `MoveFocus` (keys then go to the controller's
  HWND), drag and drop through an own `IDropTarget` (`CompositionController3`) and
  `DragStarting` (`CompositionController5`); accessibility is a UIA provider
  (`CompositionController2::get_AutomationProvider`) to graft into the window's tree. IME in
  this mode is undocumented: prototype early.
- DPI: the visual inherits its ancestors' scale; set `RasterizationScale`. The background is
  opaque or fully transparent (`DefaultBackgroundColor` alpha 0 or 255).
- One DirectComposition device and target on the window (created with
  `WS_EX_NOREDIRECTIONBITMAP`: winit's `with_no_redirection_bitmap(true)`); wgpu 30 creates
  its DX12 swap chain on a visual (`SurfaceTargetUnsafe::CompositionVisual`), with
  premultiplied alpha available there (not on a plain HWND), and never commits: the host does.
  Placements, then `Present`, then `Commit`, on the UI thread, move the views with the frame;
  the page's own pixels come from the browser process and are not frame-locked.
- `IDCompositionRectangleClip` has per-corner radii (clips draw, not input: hit-test the radius
  ourselves). Snapshots: `ICoreWebView2::CapturePreview` (fails before the first
  `ContentLoading`, may show the previous page after a navigation). No Rust project hosts
  WebView2 this way; `webview2-com` 0.39 (on `windows` 0.62) projects the COM API.

### Linux: WPE WebKit as a texture

- Without GTK owning the window, WebKitGTK cannot be embedded (X11 only, through reparenting).
  WPE renders offscreen: with the WPE Platform API an embedder subclasses `WPEDisplay`,
  `WPEToplevel` and `WPEView`; WebKit hands each frame to `render_buffer` as a `WPEBuffer`
  (DMA-BUF when the display names a DRM device, shared memory otherwise), and the embedder
  answers `buffer_rendered` / `buffer_released` ([Igalia](https://blogs.igalia.com/llepage/the-wpe-platform-api/)).
  The API was a preview in 2.50 and 2.52; 2.54 makes it the default and deprecates libwpe
  (secondary sources).
- wgpu-hal 30 imports a single-plane DMA-BUF (`Device::texture_from_dmabuf_fd`, feature
  `VULKAN_EXTERNAL_MEMORY_DMA_BUF`); copying into an own texture then releasing the buffer keeps
  WebKit's buffer lifetime out of the renderer. The `cef` crate does the same for CEF.
- Input goes through `wpe_view_event` (pointer, scroll, keys, touch), focus in and out, cursor
  callbacks; IME through a `WPEInputMethodContext` subclass. WebKit runs on a GLib main loop to
  bridge with winit's.
- Accessibility: WebKit's AT-SPI tree joins a parent only through the AT-SPI `Socket.Embed`
  call, which AccessKit's Unix adapter does not offer today; otherwise it shows as a separate
  application.
- Distribution: Debian unstable and Arch ship 2.52; Debian stable 2.48; no Flatpak runtime has
  WPE; no Rust bindings exist (sys bindings or a C shim). Servo (Rust, AccessKit grafting, GL
  only, weaker compatibility) and CEF (Chromium, OSR with DMA-BUF, about 200 MB) are second
  backends behind the same host trait.

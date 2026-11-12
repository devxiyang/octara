# Native Island

Based on the [research](../research/native-island.md). Status: confirmed; the elements, the headless fake, the macOS overlay (web views) and the macOS
underlay (maps) are implemented; focus, accessibility joining, lazy creation and the other
platforms are not.

## What is native

- **Textures, not views, for video and camera** (GPUI's `surface`, Rune's `NativeSurface`):
  `surface(source)` is an element drawing frames a platform producer hands over (an IOSurface /
  `CVPixelBuffer` on Apple, a `HardwareBuffer` on Android), zero-copy into the renderer. It clips,
  rounds, animates and interleaves like an image.
- **Views only where a texture cannot work**: web views, maps, platform pickers, a platform text
  field. `native_view(kind)` is the general element; typed builders wrap it (`web_view(url)`).
- **One kind, a view or a texture per platform**: where a platform has no view to host, its
  host renders the kind into frames Octara draws like an image (Linux: WPE WebKit for `web`),
  forwarding input to it. The app's code is the same everywhere.

## Two slots, no interleaving (Rune)

- Each window has an **underlay** (views behind Octara's surface, shown through a cutout the
  renderer clears to transparent: maps, materials) and an **overlay** (views above it: web views,
  pickers). Octara content is never drawn between two native views or over an overlay view.
- The underlay's cutout is **cleared to transparent** where the view is in paint order, so
  content painted after it draws over it (a map's controls) and what was painted before it (the
  page, the card the map sits in) does not hide it. (Rune leaves a hole by not painting the
  element's fill, which works only when nothing opaque lies below.) An element painted after an overlay view and overlapping
  it would be hidden by the view: `check` reports it (`NATIVE_COVERED`), so the layout the
  author sees is the layout the user gets.
- Clipping is a rect with corner radii (what platforms support). A view clipped by anything else
  (a path, a rotated ancestor) is hidden and reported.

## Hosting on macOS (Rune's structure)

- winit's content view and its CAMetalLayer stay as they are. Overlay views (and a clip
  container around each) are subviews above the Metal layer. Underlay views go in a host view
  inserted below every subview, the Metal layer kept above it; the Metal layer becomes
  non-opaque (premultiplied alpha) when the first underlay view appears.
- A view's clip is its container's frame with `masksToBounds` and a corner radius; the view sits
  inside at its offset. One radius per corner box, as Octara's style has (four values when the
  style gets per-corner radii). The container (flipped, top-left origin) passes the pointer
  through except over its view.
- Placements and the frame's present happen inside one `CATransaction` (actions disabled), with
  `presentsWithTransaction` on only while a native view moves or the window is resized (one flag
  owning its causes, as Rune's); wgpu-hal exposes the switch on the Metal surface.
- Events from views (delegates, any thread) are queued and the event loop woken; the app gets
  them on the main thread, stale ones dropped.
- Input: AppKit hit-tests overlay views before the content view, so they get their own pointer
  events. An underlay view is under the Metal view: where Octara's hit test finds the underlay
  view topmost, the event goes to it (a map's drags and pinches). Each underlay
  placement carries the rects Octara content covers (`covered`: what is hit before the view),
  and the underlay host's `hitTest:` takes the point only inside a visible underlay view and
  outside those rects, so AppKit delivers the view's own gestures, scrolling and magnification;
  everywhere else the event falls through to winit's view.

## Hosting on Windows (DirectComposition)

- The window is created without a redirection bitmap and holds one DirectComposition target.
  Its tree: the underlay container, the wgpu swap chain's visual (DX12 on a composition visual,
  premultiplied alpha), the overlay container; each view sits in a container visual with its
  offset, a rounded rectangle clip (per corner) and opacity.
- `web` is WebView2 in visual hosting (a composition controller whose root visual is the view's
  container). The shell applies placements, presents, then commits the device, on the UI
  thread; the page's own pixels are committed by the browser process and may trail by a frame.
- The shell forwards pointer input (mouse, pointer, wheel, leave, capture), focus, cursor and
  drag and drop to the view, and grafts its UIA provider into the window's accessibility tree.
  IME and accessibility are spiked first.
- Snapshots: `CapturePreview`.

## Hosting on Linux (textures)

- `web` is WPE WebKit (2.54 or later, the WPE Platform API) rendering offscreen: each frame's
  DMA-BUF is imported into wgpu and copied into the view's texture (shared memory when there is
  no DRM device, with a diagnostic), drawn in paint order like an image: it clips, rounds and
  interleaves, so `NATIVE_COVERED` does not apply to it.
- Input goes to `wpe_view_event`, IME through an input method context bridge, WebKit's GLib
  loop is bridged to winit's. Accessibility: WebKit's AT-SPI subtree is embedded where the
  adapter allows, else it is a separate application (a known gap).
- Bindings are a C shim or sys crate; a spike proves the DMA-BUF path on Mesa and NVIDIA first.
  Servo and CEF can be second backends behind the same host.

## Placement and lifecycle

- A native view is a leaf box in the layout: sized by its authored size, else the size the view
  reports (Exact's `intrinsicSize`).
- After each layout, every view has a placement: rect, corner radii, clip, slot, visible. The
  shell applies changed placements on the main thread in the same transaction as the frame's
  present, so views move with the content they sit in.
- Props are one value handed over whole when it changes; events come back typed; the view's
  timers follow the app clock (Exact). Created when mounted, destroyed when unmounted; a view
  in a scroll container is created when it comes near the viewport and hidden (later released)
  when it goes far (Exact: maps are heavy).
- Every host can snapshot a view (an image): screenshots, `inspect` captures and the CPU
  renderer draw the snapshot in its place instead of the placeholder (Exact).

## Input, focus, accessibility

- Pointer input inside a view's visible rect goes to the view; elsewhere to Octara. A scroll
  (wheel or trackpad) goes to a view only while focus is in it: a person clicks into a web view
  or map to scroll it, and otherwise the page around it scrolls, as a page's embedded map does
  (macOS: the views' `hitTest:` passes a scroll gesture to winit's view unless the window's
  first responder is inside the view; AppKit keeps a gesture with the view its start went to).
  Presses, drags and pinches always go to the view. A view is one
  Tab stop: the host names its focus target, which joins the Tab order; focusing it makes it
  first responder, and Escape (or Tab past it) returns focus (Exact).
- Agents type into and press inside a native view through the host (`agent_input`), refused
  when the view is hidden or not focused (Exact).
- The view is a node with a role and a label in the accessibility tree; its own subtree is joined
  where the platform adapter allows, else it is one labeled element. `check` asks a label
  (`UNLABELED_NATIVE_VIEW`).

## Headless and tests

- Without a platform that shows a kind, a native view is a placeholder box with its semantics
  and test id. The headless window keeps the changes it was presented and lets tests send the
  view's events; `inspect` shows its kind, props and placement.

## Implemented

- Elements: `native_view(kind)` with `.slot(..)`, `.props(..)` (one JSON value, bindable) and
  `.on_native(..)`; `web_view(url)`. A view is 300 × 150 until it reports its size. The display
  list marks each view's place in paint order (`Item::Native`) after its placeholder.
- Frames: each frame presents its `NativeChange`s (create, update, place, destroy) to the
  platform window, whose `shows_native`, `snapshot_native` and `native_busy` answer the app's
  questions; events, natural sizes and snapshots come back as `InputEvent`s (`Native`,
  `NativeSize`, `NativeSnapshot`). A shown view draws no placeholder, and a shown underlay
  view's `Item::Native` is `cut`: both renderers clear its rounded rect to transparent
  (destination-out; the GPU renderer switches pipelines for that instance).
- macOS overlay (`octara-shell`'s host): kind `web` is a `WKWebView` (props `url`, or `html` with
  an optional `base_url`; a change of these reloads, other props do not; events `load` `{url}`,
  `title` `{title}` (observed, so a script's title change is seen too) and `fail` `{error}`).
  Each view sits in a clip container added to winit's view when first placed. The host keeps
  the placements a frame presents; the shell settles before taking the surface texture, opens a
  `CATransaction` (actions off) while views exist, draws, applies the placements, presents and
  commits, and turns the Metal layer's `presentsWithTransaction` on for frames where a view
  moves, resizes or shows, or the window resizes. Delegate events go to an inbox and wake the
  loop; the shell sends them to the window as `InputEvent::Native`.
- macOS underlay: kind `map` is an `MKMapView` (props `center`, `span`, `style`; event `region`;
  busy while tiles load; snapshots from `MKMapSnapshotter`). The underlay host is made with the
  first underlay view, below every subview, the Metal layer kept above it (z position 1, overlay
  containers at 2) and configured non-opaque. A view inside a group-opacity layer is not cut
  through the layer (a known limit).
- Snapshots belong to [captures](capture.md): `Window::capture_begin()` asks the platform window
  (`snapshot_native`, which says what the snapshot misses or why it takes none) for an image of
  every visible view; it comes back as `InputEvent::NativeSnapshot` and is drawn in the view's
  place instead of the placeholder until the capture ends. `native_busy` (a page loading) keeps
  a capture waiting. The macOS host uses `WKWebView`'s snapshot (not waiting for a screen
  update), redrawn into straight RGBA. The shell's capture prints each view's AppKit frame next
  to its placement.
- Not yet: per-corner radii (the style has one radius), keyboard focus of a clicked map.

## Steps

1. Elements, placements, the headless fake and `check` rules (slot conflicts, clips, labels).
   Implemented.
2. macOS overlay: WKWebView, clip containers, placements in the frame's transaction, view
   snapshots for captures. Implemented.
3. macOS underlay: the host view below, a non-opaque Metal layer, the cutout cleared in paint
   order, pointer input to the underlay view where Octara does not cover it; a map
   (`MKMapView`) as the first kind. Implemented.
4. Focus targets in the Tab order, Escape back, accessibility joining; views created near the
   viewport and hidden when far; the agent input channel.
5. Windows: DirectComposition hosting, WebView2 in visual hosting (input, focus, cursor,
   accessibility, IME spiked first).
6. `surface`: video and camera frames as zero-copy textures (Rune's `NativeSurface` path).
7. Linux: WPE WebKit as a texture (the DMA-BUF spike first), on the `surface` path.
8. iOS, then Android; the platform text field as an overlay kind.

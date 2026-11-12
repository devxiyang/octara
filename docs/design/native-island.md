# Native Island

Based on the [research](../research/native-island.md). Status: confirmed; steps 1 and 2 implemented.

## What is native

- **Textures, not views, for video and camera** (GPUI's `surface`, Rune's `NativeSurface`):
  `surface(source)` is an element drawing frames a platform producer hands over (an IOSurface /
  `CVPixelBuffer` on Apple, a `HardwareBuffer` on Android), zero-copy into the renderer. It clips,
  rounds, animates and interleaves like an image.
- **Views only where a texture cannot work**: web views, maps, platform pickers, a platform text
  field. `native_view(kind)` is the general element; typed builders wrap it (`web_view(url)`).

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
  view topmost, the shell forwards the event to it (a map's drags and pinches).

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

- Pointer input inside a view's visible rect goes to the view; elsewhere to Octara. A view is one
  Tab stop: the host names its focus target, which joins the Tab order; focusing it makes it
  first responder, and Escape (or Tab past it) returns focus (Exact).
- Agents type into and press inside a native view through the host (`agent_input`), refused
  when the view is hidden or not focused (Exact).
- The view is a node with a role and a label in the accessibility tree; its own subtree is joined
  where the platform adapter allows, else it is one labeled element. `check` asks a label
  (`UNLABELED_NATIVE_VIEW`).

## Headless and tests

- Without a shell, a native view is a placeholder box with its semantics and test id. A fake
  host records props and placements and lets tests send the view's events; `inspect` shows its
  kind, props and placement.
- As built (step 1): `native_view(kind)` with `.slot(..)`, `.props(..)` (one JSON value, bindable)
  and `.on_native(..)`; `web_view(url)`. A view is 300 × 150 until it reports its size
  (`App::native_size`). After each frame the app tells its `NativeHost` what changed (create,
  update, place, destroy); headless, a `RecordingHost` keeps the calls. The display list marks
  each view's place in paint order (`Item::Native`) after its placeholder.
- As built (step 2, `octara-shell`'s macOS host): kind `web` is a `WKWebView` (props `url`, or
  `html` with an optional `base_url`; a change of these reloads, other props do not; events
  `load` `{url}`, `title` `{title}` (observed, so a script's title change is seen too) and
  `fail` `{error}`). Each view sits in a clip container added to winit's view when first placed.
  The app's settle hands placements to the host, which keeps them; the shell settles before
  taking the surface texture, opens a `CATransaction` (actions off) while views exist, draws,
  applies the placements, presents and commits, and turns the Metal layer's
  `presentsWithTransaction` on for frames where a view moves, resizes or shows, or the window
  resizes. Delegate events go to an inbox and wake the loop; the shell hands them to
  `App::native_event`. Other kinds and underlay views keep their placeholder (step 3).
- Snapshots: `App::snapshot_native()` asks the host (`NativeHost::snapshot`) for an image of
  every visible view; it comes back with `App::native_snapshot(id, image)` and is drawn in the
  view's place instead of the placeholder. The macOS host uses `WKWebView`'s snapshot (not
  waiting for a screen update), redrawn into straight RGBA. The shell's capture mode
  (`OCTARA_CAPTURE=out.png`, `OCTARA_CAPTURE_AFTER_MS`, `OCTARA_CAPTURE_SCROLL=px`) writes the
  frame with the snapshots through the reference renderer and prints each view's AppKit frame
  next to its placement, then exits: screenshots without the screen-recording permission.

## Steps

1. Elements, placements, the headless fake and `check` rules (slot conflicts, clips, labels).
   Done.
2. macOS overlay: WKWebView (`url` or `html` props; `load` and `title` events), clip containers,
   placements in the frame's transaction with `presentsWithTransaction` while moving, view
   snapshots for captures. Done as built below; per-corner radii wait for the style to have
   them.
3. macOS underlay: the host view below, a non-opaque Metal layer, the cutout cleared in paint
   order, events forwarded to the underlay view; a map (`MKMapView`) as the first kind.
4. Focus targets in the Tab order, Escape back, accessibility joining; views created near the
   viewport and hidden when far; the agent input channel.
5. `surface`: video and camera frames as zero-copy textures (Rune's `NativeSurface` path).
6. iOS, then Android; the platform text field as an overlay kind.

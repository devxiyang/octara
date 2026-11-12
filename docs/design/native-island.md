# Native Island

Based on the [research](../research/native-island.md). Status: confirmed; step 1 implemented.

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
- The underlay's cutout is cleared where the view is in paint order, so content painted after
  it draws over it (a map's controls). An element painted after an overlay view and overlapping
  it would be hidden by the view: `check` reports it (`NATIVE_COVERED`), so the layout the
  author sees is the layout the user gets.
- Clipping is a rect with corner radii (what platforms support). A view clipped by anything else
  (a path, a rotated ancestor) is hidden and reported.

## Placement and lifecycle

- A native view is a leaf box in the layout: sized by its authored size, else the size the view
  reports (Exact's `intrinsicSize`).
- After each layout, every view has a placement: rect, corner radii, clip, slot, visible. The
  shell applies changed placements on the main thread in the same transaction as the frame's
  present, so views move with the content they sit in.
- Props are one value handed over whole when it changes; events come back typed; the view's
  timers follow the app clock (Exact). Created when mounted, destroyed when unmounted.

## Input, focus, accessibility

- Pointer input inside a view's visible rect goes to the view; elsewhere to Octara. A view is one
  Tab stop; focusing it makes it first responder, and Escape (or Tab past it) returns focus.
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

## Steps

1. Elements, placements, the headless fake and `check` rules (slot conflicts, clips, labels).
2. The macOS shell: overlay and underlay hosting, placement sync in the frame's transaction, hit
   routing; the web view (WKWebView) first.
3. Focus handoff and accessibility joining; a map as the first underlay.
4. `surface`: frames from AVFoundation into the renderer; video and camera.
5. iOS, then Android; the platform text field as an overlay kind.

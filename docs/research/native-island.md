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

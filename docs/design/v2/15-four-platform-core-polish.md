# 15 — Tier-1 Core Interaction, Accessibility, Forms, RTL & Airspace Closure (`0.0.9`)

> **Design premise**: Freeze and finish every core cross-cutting UI capability across **Tier-1 platforms (macOS 15.4+, iOS 18.4+, Web, Linux)** before scaling out the `modules/*` ecosystem. Zero backwards-compatibility shims, zero pre-15.4/18.4 Apple fallbacks, zero `#[cfg(target_os)]` in native core orchestration, and 100% DST testability.
>
> **Prerequisite**: `0.0.8` (`docs/design/v2/14-octara-module-architecture.md`) complete.

---

## 1. Seven Core Framework Mechanisms

| # | Mechanism | Architectural Layer | Platforms Closed |
| --- | --- | --- | --- |
| **C1** | **Unified Gesture Engine & Mobile Scroll Physics** (`GestureEvent`, `PullToRefresh`, `ScrollDismissesKeyboard`) | `octara-model` / `octara-tree` / `octara-runtime` / `octara-host-*` / `octara-headless` | macOS · iOS · Web · Linux · DST |
| **C2** | **Soft-Keyboard & Form Input Traits** (`KeyboardType`, `ReturnKeyType`, `AutoCapitalize`, `Autocorrect`, `TextContentType`) | `octara-model` / `octara-tree` / `octara-host-*` / `octara-kit` | iOS · Web · macOS · Linux · DST |
| **C3** | **iOS Native Closure** (`UIAccessibility` / VoiceOver semantic tree, `StatusBarStyle`, safe-area/rotation parity) | `octara-model` / `octara-runtime` / `octara-host-native` (`os/ios`) | iOS |
| **C4** | **Web Production Closure** (`visualViewport` keyboard inset, `document.title` & `theme-color`, `beforeunload`, multimodal clipboard write, ARIA `inert` focus trap) | `octara-model` / `octara-runtime` / `octara-host-web` | Web |
| **C5** | **Desktop Drag-Out, Window Levels/Panels & Linux Native Polish** (`NSDraggingSession`/Web drag out, `WindowLevel`, floating panel, Wayland `text-input-v3` IME, fractional HiDPI, XDG Portal) | `octara-model` / `octara-tree` / `octara-runtime` / `octara-host-native` / `octara-host-web` | macOS · Linux · Web |
| **C6** | **Airspace `SnapshotOnOcclude`, BiDi/RTL & Accessibility Preferences** (`Direction::{Ltr, Rtl}`, `SystemPreferences::{locale, text_scale, high_contrast}`) | `octara-model` / `octara-tree` / `octara-pipeline` / `octara-host-*` | All Tier-1 + WebView |
| **C7** | **P0 120Hz `CADisplayLink` Presentation Pacing, VSync Input Coalescing & Scroll / `virtual_each` Hot-Path Optimization** (monotonic GPU presentation without `Retry(+16ms)` cascade, replace `CAMetalDisplayLink` with unified macOS/iOS `CADisplayLink` + adaptive `CAFrameRateRange` ProMotion + `targetTimestamp` scanout sampling + screen-tracking + idle-grace, causal VSync `MouseWheel`/`CursorMoved` coalescing + real `WheelPhase`, CPU/GPU scroll & virtual-list profiling) | `octara-host-native` / `octara-pipeline` / `octara-tree` / `octara-markdown` / `octara-render` | macOS · iOS · Linux |

---

## 2. Detailed Specifications

### 2.1 C1 — Unified Gesture Engine & Mobile Scroll Physics
1. **Layer-0 Gesture Values (`octara-model::input`)**:
   ```rust
   #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
   #[serde(rename_all = "snake_case")]
   pub enum GesturePhase { Started, Changed, Ended, Cancelled }

   #[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
   #[serde(rename_all = "snake_case")]
   pub enum GestureEvent {
       Pinch { phase: GesturePhase, scale: f32, delta_scale: f32, anchor: Point, modifiers: Modifiers },
       Rotate { phase: GesturePhase, radians: f32, delta_radians: f32, anchor: Point, modifiers: Modifiers },
       LongPress { phase: GesturePhase, position: Point, pointer: PointerKind },
       DoubleTap { position: Point, pointer: PointerKind },
   }
   ```
2. **Routing (`octara-tree::arbiter`)**:
   - Add `HandlerKind::Gesture` (`el.on_gesture(|w, ev: &GestureEvent| -> bool)`) bubbling along `logical_parent`.
   - Synthesize `LongPress` (default 500 ms hold with ≤ 8 px movement threshold on virtual clock) and `DoubleTap` cleanly inside the shared arbiter so macOS, iOS, Web, Linux, and DST behave identically.
   - Forward native hardware pinch/rotate events:
     - **macOS**: `magnifyWithEvent:` / `rotateWithEvent:` via `OsWindow`.
     - **iOS**: `UIPinchGestureRecognizer` / `UIRotationGestureRecognizer`.
     - **Linux**: `winit` touchpad `PinchGesture` / `RotationGesture`.
     - **Web**: `ctrlKey` wheel zoom + Safari/Pointer multi-touch pinch.
3. **Scroll Ergonomics**:
   - Add `ScrollDismissesKeyboard::{Never, Immediately, Interactively}` on scroll containers; when triggered during touch drag on iOS/Web, emit editor blur / keyboard dismissal cleanly.
   - Expose `overscroll_offset` state on vertical scroll containers + a headless `PullToRefresh` controller in `octara-headless` (and styled wrapper in `octara-kit`) supporting threshold haptic hook, refreshing spinner state, and spring settle.

---

### 2.2 C2 — Soft-Keyboard & Form Input Traits
Add a compact `InputTraits` struct to `octara-model::text` attached to `TextInput` (stored sparsely in `octara-tree` editor table, zero cost on non-editor nodes):

```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyboardType { #[default] Default, Email, Number, Decimal, Phone, Url, Search }

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReturnKeyType { #[default] Default, Done, Go, Next, Search, Send }

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoCapitalize { #[default] Sentences, Words, Characters, None }

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextContentType { #[default] None, Username, Password, NewPassword, OneTimeCode, Email, Url, Telephone }
```

- **iOS (`os/ios`)**: Map directly onto `UITextInputTraits` (`keyboardType`, `returnKeyType`, `autocapitalizationType`, `autocorrectionType`, `textContentType`) when syncing `ActiveEditorView`.
- **Web (`octara-host-web`)**: Project onto DOM attributes (`inputmode`, `enterkeyhint`, `autocapitalize`, `autocorrect`, `autocomplete` incl. `"one-time-code"` / `"current-password"` / `"new-password"`).
- **Authoring (`octara-kit`)**: Builder helpers `.keyboard_type(...)`, `.return_key(...)`, `.content_type(...)`, `.autocapitalize(...)`, `.autocorrect(bool)` on `TextInput`.

---

### 2.3 C3 — iOS Native Closure (`UIAccessibility` & Status Bar)
1. **iOS VoiceOver Bridge (`crates/octara-host-native/src/os/ios/a11y.rs`)**:
   - Project settled `octara-pipeline` semantic tree nodes (roles, labels, values, checked/expanded/disabled states, live regions, bounds in UIKit coordinates) into `UIAccessibilityElement` instances attached to the root `UIView`'s `accessibilityElements`.
   - Route VoiceOver activation (`accessibilityActivate`), increment/decrement (`accessibilityIncrement`/`Decrement`), and focus announcements back through `A11yAction` into the runtime `Cycle`.
   - Emit `UIAccessibilityLayoutChangedNotification` / `UIAccessibilityAnnouncementNotification` on modal/live-region commits.
2. **Status Bar & Safe-Area Closure**:
   - Add `pub status_bar: StatusBarStyle` (`Auto`, `Light`, `Dark`, `Hidden`) to `WindowPresentation` (`Auto` tracks `WindowPresentation.theme.color_scheme`).
   - Wire `UIViewController::preferredStatusBarStyle` / `prefersStatusBarHidden` + `setNeedsStatusBarAppearanceUpdate` on presentation diff.
   - Verify portrait/landscape rotation and Dynamic Island / Home Indicator updates emit `HostReply::ViewportChanged` with exact `safe_area` + `keyboard_inset`.

---

### 2.4 C4 — Web Production Closure (`octara-host-web`)
1. **`window.visualViewport` Keyboard Inset**:
   - Listen to `visualViewport` `resize` and `scroll` events; compute `keyboard_inset = max(0.0, innerHeight - visualViewport.height - visualViewport.offsetTop)` and publish via the exact same `WindowViewport.keyboard_inset` used on iOS.
2. **Page Chrome & Unload Guard**:
   - Sync `WindowPresentation.title` → `document.title` and `WindowPresentation.theme` background → `<meta name="theme-color">` (creating/updating the tag cleanly).
   - Wire `window` `beforeunload` to the window's `on_close_request` handler (calling `preventDefault()` / setting `returnValue` when the handler returns `false`).
3. **Multimodal Clipboard Write**:
   - Extend `ServiceRequest::WriteClipboard(Vec<ClipboardItem>)` (replacing text-only `WriteClipboardText`, keeping zero dual-track) so both Native (`NSPasteboard` / Linux clipboard) and Web (`navigator.clipboard.write([new ClipboardItem(...)])`) can write text and PNG images cleanly.
4. **Complete Modal ARIA Focus Trap & `inert`**:
   - When an overlay with `LayerPolicy { inert_outside: true, focus_trap, .. }` is mounted in `DomEngine`, set the native HTML `inert` attribute on occluded sibling roots, lock `Tab`/`Shift+Tab` inside the modal subtree, and restore focus to the opener upon dismiss (matching Native `LayerPolicy` 100%).

---

### 2.5 C5 — Desktop Drag-Out, Window Levels/Panels & Linux Native Polish
1. **Drag-Out Protocol (`octara-model` / `octara-tree` / `octara-host-*`)**:
   ```rust
   #[derive(Clone, Debug, PartialEq, Eq)]
   pub enum DragItem {
       Text(String),
       File(PathBuf),
       Image { media_type: String, bytes: Vec<u8> },
   }
   ```
   - Add `el.on_drag_start(|w| -> Option<Vec<DragItem>>)` triggered once pointer drag crosses the system drag threshold.
   - **macOS**: Initiate `NSDraggingSession` (`beginDraggingSessionWithItems:event:source:`) with `NSString` / `NSURL` / `NSImage` pasteboard writers.
   - **Web**: Set `draggable="true"` and populate `DataTransfer` in `dragstart`.
   - **Linux**: Integrate standard drag source or clean fallback.
2. **Window Levels & Non-Activating Panels (`octara-model::window`)**:
   - Add `WindowLevel::{Normal, Floating, AlwaysOnTop}` and `pub non_activating_panel: bool` to `WindowOptions` / `WindowPresentation`.
   - **macOS**: Configure `NSWindowLevel` (`NSNormalWindowLevel` / `NSFloatingWindowLevel` / `NSStatusWindowLevel`) and `NSWindowStyleMaskNonactivatingPanel` (`NSPanel` subclass) for Spotlight / HUD floating windows that do not steal activation from other apps.
   - **Linux**: Map `WindowLevel` onto `winit::window::WindowLevel`.
3. **Linux Native Host Polish (`crates/octara-host-native/src/os/desktop/`)**:
   - Verify & complete Wayland `text-input-v3` IME cursor-rect syncing (`set_ime_cursor_area`) and preedit composition.
   - Verify fractional HiDPI scale factor changes update `WindowViewport.scale` and re-shape text cleanly without blur.
   - Read XDG Desktop Portal `org.freedesktop.appearance` (`color-scheme`, `reduced-motion`, `contrast`) into `SystemPreferences`.

---

### 2.6 C6 — Airspace `SnapshotOnOcclude`, BiDi/RTL & Accessibility Preferences
1. **`AirspacePolicy::SnapshotOnOcclude` (`octara-pipeline` + `octara-host-native` + `octara-host-web`)**:
   - During frame projection, compute the union bounding boxes of active Octara overlays (`overlay_root` live children).
   - For every mounted native view whose `NativeViewFactory::airspace_policy()` is `AirspacePolicy::SnapshotOnOcclude` and whose `NativeSlot == Overlay`:
     - On transition `unoccluded → occluded`: trigger `NativeViewInstance::snapshot(...)`, cache the returned `Picture` inside the node's paint slot, and call `set_occluded_by_snapshot(true)`.
     - On transition `occluded → unoccluded`: drop the frozen snapshot overlay and call `set_occluded_by_snapshot(false)`.
   - Verify deterministically in `HeadlessDriver` + real native window tests.
2. **BiDi / RTL Foundation (`octara-model` / `octara-tree` / `octara-pipeline` / `octara-host-web`)**:
   - Add `Direction::{Inherit, Ltr, Rtl}` on `StyleIR` (propagated along `logical_parent`, root default derived from `SystemPreferences.locale`).
   - Add logical spacing/border helpers (`ms`, `me`, `ps`, `pe`, `start`, `end`) resolving against effective `Direction` in `StyleResolver` (zero extra layout cost in Taffy/Parley; Parley base paragraph direction follows resolved `Direction`; Web projects HTML `dir="rtl"`/`"ltr"`).
3. **System Accessibility & Locale Preferences (`SystemPreferences`)**:
   - Extend `SystemPreferences` with `pub locale: Arc<str>` (BCP-47, e.g. `"en-US"`, `"zh-Hans"`, `"ar"`), `pub text_scale: f32` (default `1.0`, bound to iOS Dynamic Type / Linux font scale / Web root scale), and `pub high_contrast: bool`.

### 2.7 C7 — P0 120Hz `CADisplayLink` Presentation Pacing, VSync Input Coalescing & Scroll / `virtual_each` Hot-Path Optimization
1. **Eliminate Finished-Frame Discard & `Retry(+16ms)` Cascade (`octara-host-native/src/presentation.rs`, `window.rs`)**:
   - **Monotonic GPU Presentation**: Replace the strict `stamp.native_rev == latest` equality rejection in `Presentation::accepts` and `HostWindow::finish_draw` with monotonic acceptance (`stamp.native_rev >= last_presented_rev`, preserving strict `tree`, `surface_generation`, `device_generation`, and `size` guards). A completed GPU frame `N` MUST still be presented even when newer input pushed `latest` to `N+1` while frame `N` was rendering; after presenting `N`, keep the display clock active so `N+1` draws on the very next VSync.
   - **Native-View Synchronization Without Blocking Pure-Octara Frames**: Apply native view placements synchronized up to `stamp.native_rev` (keeping the `CATransaction` atomic frame-sync guarantee for mounted native views) without discarding valid completed GPU frames whenever input advances `latest`.
   - **External Display-Link Pacing Must Never Trigger `Retry(+16ms)`**: Distinguish `DrawableAcquisition::WaitingForClock` (awaiting the next `CADisplayLink` tick) from `DrawableAcquisition::Retry` (non-externally-paced surface `Timeout`/`Occluded`). Under `CADisplayLink` pacing, awaiting the next VSync tick MUST NOT set `self.redraw_at = now + 16ms` and MUST NOT issue out-of-band `window.request_redraw()` calls that skip real display-link ticks.
2. **Replace `CAMetalDisplayLink` with Unified Apple `CADisplayLink` (`macOS 15.4+` & `iOS 18.4+`)**:
   - Delete `CAMetalDisplayLink` and its pre-minted drawable lock-in; share one modern `CADisplayLink` frame-clock driver across macOS and iOS:
     - **macOS (`15.4+`)**: Create via `NSView::displayLinkWithTarget:selector:` on the window's content view so the link automatically tracks whichever `NSScreen` the window occupies (seamlessly switching refresh rates when dragged between a 60Hz 5K external display and a 120Hz ProMotion MacBook display).
     - **iOS (`18.4+`)**: Create via `CADisplayLink::displayLinkWithTarget:selector:` added to `NSRunLoop::mainRunLoop()` in `NSRunLoopCommonModes` (unified with macOS).
   - **Modern `CADisplayLink` Features**:
     - **Adaptive ProMotion `CAFrameRateRange` (`preferredFrameRateRange`)**: Query the window screen's live `maximumFramesPerSecond` and dynamically configure `CAFrameRateRange`:
       - *Interactive / Scroll / Continuous Motion*: request high ProMotion range (e.g. `minimum: 80.0`, `maximum: max_fps`, `preferred: max_fps` clamped to screen capability, or `60.0` on 60Hz displays).
       - *Low-Frequency / Reduced Motion / Idle Grace*: step down `preferredFrameRateRange` when only low-rate transitions are active, and pause (`setPaused(true)`) cleanly once the short ~120 ms interactive grace window elapses with no pending revisions (`0 Hz / 0% CPU` at rest).
     - **Exact Scanout Prediction (`targetTimestamp`) & Live Refresh Interval (`targetTimestamp - timestamp`)**: Convert `link.targetTimestamp` into the monotonic sample instant fed to `MotionEngine` / `scene.sample(...)` for zero phase jitter, and propagate the live display frame interval (`1/120 s` vs `1/60 s`) into text-stream coalescing and frame-pacing telemetry.
     - **Decoupled On-Demand Drawable Acquisition**: Acquire the Metal drawable (`CAMetalLayer::nextDrawable` / `wgpu::Surface`) **only** when a VSync tick actually has a dirty/animated frame to paint—eliminating `CAMetalDisplayLink`'s forced extra frame latency (`preferredFrameLatency` removed) and wasted drawable reservations.
3. **Causal VSync Input Coalescing & `WheelPhase` Fidelity (`shell.rs`)**:
   - Map `winit`'s `MouseWheel` `TouchPhase` accurately to `WheelPhase` (stopping hardcoded `WheelPhase::Discrete`) so trackpad direct vs. momentum scroll physics work natively.
   - Coalesce bursty `CursorMoved` (keep latest position) and `MouseWheel` (accumulate `delta` for matching `(window, phase, mods)`) and flush them at the start of each `CADisplayLink` VSync tick—before running `Cycle` + layout + paint—while flushing immediately ahead of any discrete button/key/phase-change event to preserve strict causal ordering.
4. **Flamegraph-Backed CPU / GPU Scroll & Bulk-Switch Hot-Path Optimization (`octara-pipeline`, `octara-tree`, `octara-runtime`, `octara-host-native`)**:
   - **P0-A — Lazy / On-Demand `AccessKitProjector` + Scroll-Container Offset Fast-Path (`crates/octara-pipeline/src/a11y/mod.rs`, `octara-host-native`, `octara-inspect`)**:
     - Keep `AccessKitProjector` dormant (`0.00 ms` per frame) until an assistive consumer activates it (`accesskit_winit` initial-tree request, iOS VoiceOver activation, or `Inspector` a11y query); materialize via `snapshot_full` on first activation and track incremental diffs thereafter.
     - Even when accessibility IS active, pure scroll offset updates MUST update only the scroll container's scroll properties/transform rather than pushing every scrolled descendant into `staging.moved` and running `Tree::is_locally_accessible` + `build_entry` + `accesskit::Properties::eq` across hundreds/thousands of unchanged nodes.
   - **P0-B — O(1) Scroll-Transform Retained Paint (`crates/octara-pipeline/src/paint/mod.rs`)**:
     - Keep scroll-container children compiled in scroll-content-local coordinates beneath a single scroll-offset transform/clip wrapper so pure `SetScroll` rebuilds **O(1)** subtrees (the scroll container wrapper only) instead of re-evaluating 600–1,600 descendant `draw_keys` / `subtrees_rebuilt` / `child_spans_rebuilt`.
     - Keep offset changes separate from layout geometry changes. Use content-space hit grids and resolve the current scroll chain for world bounds, text selection, and clips.
   - **P1-C — O(1) Bulk Teardown & Preflight (`crates/octara-runtime/src/window.rs::is_retiring`, `crates/octara-tree/src/transaction.rs::Preflight::retirement_ancestor`)**:
     - Replace repeated per-node ancestor-chain walks during bulk subtree retirement and transaction preflight with memoized / flat retired-set lookup so heavy list/thread switches avoid O(N × depth) overhead.
   - **Regression Gates**: Add DST & Release micro-benchmarks asserting O(1) `subtrees_rebuilt` on pure scroll (100+ scrolled rows), zero `AccessKitProjector` work while dormant, zero stale-frame discards / `Retry` stalls under overlapping 60Hz/120Hz scroll & hover input, and verified Release timings for pure micro-scroll, fast-fling `virtual_each` + `octara-markdown`, and heavy list switches.
5. **Retained GPU Spatial Buffers & Damage**:
   - Use stable spatial slots in the display list and shared CPU/GPU scroll snapping. In a compatible frame, reuse prepared draw order and instance buffers and upload only changed spatial rows.
   - On a compatible initialized backing texture, compute damage in device pixels from old and new content bounds and from moved scrollports. Below 60% coverage, clear and rasterize the damaged rectangle, then transfer the completed frame to the current drawable. A new target, a large damaged area, or a pass with non-local effects uses full rendering.

---

## 3. Execution Waves & Verification Gates (`0.0.9`)

### Wave A — Gestures, Soft-Keyboard Traits, BiDi/RTL & `SystemPreferences` (Cross-Platform Core)
- Implement **C1** (`GestureEvent`, arbiter `LongPress`/`DoubleTap`, hardware Pinch/Rotate, `ScrollDismissesKeyboard`, `PullToRefresh`), **C2** (`InputTraits`), and **C6-part** (`Direction` RTL/logical spacing + `SystemPreferences::{locale, text_scale, high_contrast}`).

### Wave B — P0 120Hz `CADisplayLink` Pacing, Input Coalescing, Scroll Hot-Path & iOS/Web Closure
- Implement **C7** (monotonic GPU presentation without `Retry(+16ms)` cascade, replace `CAMetalDisplayLink` with unified macOS/iOS `CADisplayLink` + adaptive `CAFrameRateRange` ProMotion + `targetTimestamp` scanout sampling + screen-tracking + ~120 ms idle grace window + on-demand drawable acquisition, causal VSync `MouseWheel`/`CursorMoved` coalescing + real `WheelPhase`, Release profiling & optimization of `SetScroll` / `virtual_each` + `octara-markdown` / bulk switch, plus automated overlapping-input pacing regression test).
- Implement **C3** (iOS `UIAccessibility` VoiceOver bridge, `StatusBarStyle`, safe-area/rotation verification) and **C4** (Web `visualViewport` keyboard inset, `document.title`/`theme-color`, `beforeunload`, unified `WriteClipboard(Vec<ClipboardItem>)`, ARIA `inert` focus trap).

### Wave C — Desktop Drag-Out, Window Levels/Panels, Linux Polish & Airspace `SnapshotOnOcclude`
- Implement **C5** (macOS/Web Drag Out, `WindowLevel` / `NSPanel`, Linux Wayland IME/HiDPI/XDG Portal) and **C6-part** (`AirspacePolicy::SnapshotOnOcclude`).
- Pass full `0.0.9` release gate: `tools/check_layers.py`, Clippy `-D warnings`, dual Wasm + Headless Chrome smoke, `aarch64-apple-ios-sim`, full workspace + CLI doctests, and DST + Release pacing regression ceilings.

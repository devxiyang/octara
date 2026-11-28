# 21 — Direct Apple Metal Renderer (`octara-render-metal`), Ultra-Low-Latency Presentation & 2.5D / Zero-Copy 3D Compositor (`0.0.11`)

> **Design premise**: Zero `wgpu` / zero `naga` on macOS `15.4+` and iOS `18.4+` by default; zero historical baggage or backwards-compatibility shims in `octara-gpu` or `octara-host-native`.
> **North star**: Eliminate the `wgpu` presentation and compile/binary tax on Apple Silicon & iOS, crush continuous-resize livelocks and post-acceptance presentation tail latency (`~30.9 ms` → sub-1-VSync), resolve dual-display 144 Hz display clocking empirically on real hardware, tighten causal wheel coalescing, and build a lean **2D + 2.5D `Mat4` + Zero-Copy `ExternalTexture`** compositor behind `SPI-1`.

---

## 1. Real-Hardware Diagnosis (`Octara 0.0.10` Live M4 + 144 Hz Dual-Display Telemetry)

End-to-end production telemetry on Apple M4 (`macOS 27.2`, 60 Hz Retina + 144 Hz external `P27QBC-RG`) revealed four concrete host/presentation bottlenecks directly coupled to the `wgpu` + `CAMetalLayer` bridge:

1. **Continuous Window Resize Livelock (`0/119` `presentedTime > 0`, `303 ms` recovery stall)**:
   - `shell.rs` calls `host.invalidate_surface(false)` on every raw `WindowResized` event, which immediately drops `self.draw = None` (abandoning the in-flight `CAMetalDrawable` → `presentedTime = 0`) and clears `surface_size`, forcing a `SurfaceLease` GPU drain barrier + `wgpu::Surface::configure()` on every resize tick. At 60 Hz resize input, each new in-flight frame is assassinated before `finish_draw()` can present it.
2. **Post-Acceptance Tail Latency (`~30.9 ms` at steady 60 fps on Retina)**:
   - Every frame currently creates **3 separate `MTLCommandBuffer`s**: (1) a redundant full-screen `clear()` CB in `MetalFrame::texture()`, (2) `wgpu`'s rendering CB on the GPU worker, and (3) an empty CB on the main thread after a `HostWake` cross-thread RunLoop round-trip just to call `presentDrawable()` — compounded by triple buffering (`CAMetalLayer.maximumDrawableCount = 3`).
3. **Dual-Display 144 Hz External Monitor Throttled to ~60 Hz**:
   - On mixed 60 Hz + 144 Hz dual displays, `NSView::displayLinkWithTarget:selector:` (created once at window init with `minimum = 80.0`) delivered only ~60 callbacks/sec even on a blank `NSView`. Selecting the right Apple display clock requires an empirical side-by-side probe on the real M4 dual-display hardware.
4. **Over-Conservative Causal Wheel Coalescing (`octara-runtime/src/window.rs`)**:
   - `can_coalesce_wheel()` rejected coalescing whenever a scrollport `port` had `FeedbackSignal::Scroll` / `ScrollAnchor` / `Pinned` bound to `port` itself (true for every `virtual_each` and chat transcript), preventing 120 Hz wheel bursts from coalescing to one cycle per VSync (`2,141` uncoalesced wheel cycles).
5. **`wgpu` Dependency & Binary Footprint on Apple**:
   - Carrying `wgpu` + `naga` on macOS/iOS adds ~120+ transitive crates, 4–8 MB of iOS/macOS binary size, compile time, and CPU validation overhead solely to drive 4 tiny 2D UI micro-pipelines (~350 lines of shader).

---

## 2. Six-Pillar Clean-Sheet Architecture (`0.0.11`)

### Pillar 1 — Decouple Device-Free Render Planning from GPU Backends

Break `octara-render`'s coupling between pure planning and `wgpu`, and make `octara-gpu` & `octara-host-native` backend-polymorphic:

1. **`crates/octara-render` (Layer 3a, 0 OS FFI)**:
   - **`feature = "plan"`** (default; depends only on `octara-model`, `bytemuck`, `half`, `color`, `rustc-hash`, `swash`, `kurbo`, `zeno` — **0 `wgpu`**): exposes `FramePlan`, `PlannedBatch`, `DamageRect`, `SpatialTable`, `GlyphInst`/`QuadInst`/`ShadowInst`/`ImageInst`, `GlyphCache`, `ImageMips`, `CpuRenderer`, `OutputColor`, `FrameProfile`, and `plan_frame*`.
   - **`feature = "wgpu"`** (implies `"plan"` + `dep:wgpu` + `web-time` on wasm32): the `wgpu` backend (`WgpuRenderer`) used by Linux (Vulkan/GL), Windows (DX12), Web (`feature = "webgpu"`), and opt-in macOS A/B parity tests.
2. **New Crate: `crates/octara-render-metal` (Layer 3a, Apple `macOS 15.4+` / `iOS 18.4+`)**:
   - Depends **only** on `octara-model`, `octara-render (default-features = false, features = ["plan"])`, `objc2`, `objc2-foundation`, `objc2-metal`, `objc2-quartz-core`, `objc2-core-video` — **0 `wgpu`, 0 `naga`** by default!
   - Optional `feature = "custom-wgsl"` (strictly opt-in): lazily translates cross-platform WGSL `CustomPaint` programs to MSL via `naga` and caches the `MTLRenderPipelineState`. Native MSL `CustomPaint` runs with **0 `naga`**.
3. **Backend-Polymorphic `octara-gpu` & `octara-host-native`**:
   - Define a clean `GpuBackend` / `SurfaceSession` / `DrawableTarget` abstraction inside `octara-gpu` (replacing hardcoded `wgpu::Instance` / `wgpu::Adapter` / `wgpu::Device` / `wgpu::Queue` / `wgpu::Surface` in `octara-host-native/src/window.rs` & `shell.rs`).
   - On macOS & iOS (default): `octara-host-native` selects **`MetalBackend`** (`octara-render-metal`), binding directly to `OctaraView`'s `CAMetalLayer` with **0 `wgpu` in `cargo tree`**!
   - On Linux / Windows (or macOS `--features wgpu` for parity testing): selects **`WgpuBackend`**.
   - Register `octara-render-metal` in `tools/check_layers.py` (Layer 3a) and root `Cargo.toml` workspace members.

---

### Pillar 2 — Single-`MTLCommandBuffer`, Zero-Bounce Metal Presentation & Stage Telemetry

1. **Single `MTLCommandBuffer` Per Frame**:
   - Implement the 4 native MSL micro-pipelines (`glyph.metal`, `quad.metal`, `shadow.metal`, `image.metal`) with Metal Vertex Pulling (`VsOut = 4 B` `inst_id: uint`), 4-vertex `TriangleStrip`, `texture2d_array`, hardware `sampler`, and `SpatialTable` + retained `MTLBuffer` instance buffers.
   - **Zero redundant `clear()` CB**: use `MTLLoadAction::Clear` (full frame) or `MTLLoadAction::Load` (5K `DamageRect` partial backing) directly in the render pass descriptor.
   - **Apple Silicon TBDR Memoryless Depth**: allocate the Early-Z `Depth16Unorm` attachment with `MTLStorageMode::Memoryless` (0 DRAM bandwidth!).
   - **5K Persistent Private Backing + Single-CB Blit**: rasterize dirty damage rects into a persistent `MTLStorageMode::Private` backing texture, then blit into the acquired `CAMetalDrawable` texture inside the **exact same `MTLCommandBuffer`**.
2. **Zero Main-Thread Bounce on Non-Transactional Present**:
   - When `!transactional` (standard scroll/hover/typing frames with no synchronized native-view placement move), the GPU worker encodes `command_buffer.presentDrawable(drawable)` (or `presentDrawable_afterMinimumDuration` when pacing) **on the same rendering `MTLCommandBuffer`** before `commit()` — eliminating the worker → main-thread RunLoop round-trip before presentation!
   - When `transactional` (synchronized `WKWebView` / native-view placement or AppKit `inLiveResize`), keep `layer.setPresentsWithTransaction(true)` + `command_buffer.waitUntilScheduled()` + `drawable.present()` inside the main-thread `CATransaction`.
3. **Low-Latency Double Buffering & Hardware Stage Telemetry**:
   - Set `CAMetalLayer.setMaximumDrawableCount(2)` for interactive low-latency pacing (preventing triple-buffer queue buildup).
   - Expose lightweight opt-in `FrameStageTimings` (`tick_at`, `cpu_done_at`, `encode_done_at`, `gpu_start_s`, `gpu_end_s`, `presented_at_s`) via `MTLCommandBuffer::addCompletedHandler` + `MTLDrawable::addPresentedHandler` so every millisecond of tail latency is directly observable.

---

### Pillar 3 — Zero-Stall Live Resize Architecture

1. **Coalesce `WindowResized` to Display Cadence (`FrameClockTick`)**:
   - `PlatformEvent::WindowResized` marks `host.pending_resize = true` (coalescing multiple 60 Hz resize notifications) instead of calling `invalidate_surface(false)` and killing `self.draw = None` mid-flight.
2. **Never Abandon Valid In-Flight Drawables on Non-Fatal Resize**:
   - `Presentation::accepts()` distinguishes fatal invalidation (device lost / surface rebuilt / wrong tree) from non-fatal size changes: an in-flight frame rendered at `(W_old, H_old)` completes and presents normally (Core Animation displays it smoothly without `presentedTime = 0` drops), and the very next VSync tick renders at `(W_new, H_new)`.
3. **Zero-Drain `CAMetalLayer.drawableSize` Updates**:
   - Direct `octara-render-metal` updates `CAMetalLayer::setDrawableSize` cleanly between frames without `wgpu::Surface::configure()`'s full `SurfaceLease` GPU drain barrier.
4. **AppKit `inLiveResize` Transaction Lock**:
   - While `NSWindow::inLiveResize()` is active, enable `presentsWithTransaction = true` and `kCAGravityTopLeft` so Metal content stays pixel-locked to the dragging window edge with zero rubber-banding.

---

### Pillar 4 — Empirical Dual-Display 144 Hz Apple Frame Clock

1. **Empirical Hardware Probe First (On Real M4 Dual-Display: 60 Hz Retina + 144 Hz External)**:
   - Measure actual callback cadence and `MTLDrawable.presentedTime` intervals across:
     - (a) `NSView::displayLinkWithTarget:selector:`,
     - (b) `NSScreen::displayLinkWithTarget:selector:` (re-created on `NSWindowDidChangeScreenNotification`),
     - (c) `CAMetalDisplayLink` (`preferredFrameLatency = 1`, `CAFrameRateRange`),
     - (d) `CVDisplayLinkCreateWithCGDisplay(display_id)` (hardware `CGDirectDisplayID` VSync).
2. **Adopt the Empirically Winning Clock Strategy**:
   - Re-bind automatically on `NSWindowDidChangeScreenNotification` whenever a window moves between displays (e.g., 60 Hz Retina ↔ 144 Hz external), and verify real `6.945 ms` / 144 Hz delivery (or document the exact OS ceiling if macOS 27.2 enforces a compositor-level limit across all 4 APIs).

---

### Pillar 5 — Precision Causal Wheel Coalescing (`octara-runtime`)

Fix `WindowRuntime::can_coalesce_wheel()` in `crates/octara-runtime/src/window.rs`:
- Keep per-sample uncoalesced dispatch **only** when a nested scrollport exists (`chain.next().is_some()` or a descendant scrollport inside `port`) or when an external/ancestor `FeedbackSignal::Geometry` observer could shift which scrollport owns `pos` mid-VSync.
- Allow coalescing when `FeedbackSignal::Scroll` / `ScrollAnchor` / `Pinned` is bound to `port` itself (e.g., `virtual_each` / chat transcript), since those observers strictly consume the net scroll offset after the VSync batch — cutting 120 Hz wheel cycles in half at 60 Hz with 0 semantic regression.

---

### Pillar 6 — 2.5D `Mat4` Layer Transforms & Zero-Copy `ExternalTexture` Compositor

Keep core crates 100% free of 3D game-engine bloat (no glTF, PBR, skeletal animation, or physics) while supporting the two real UI 3D layers natively across Metal, `wgpu`, and Web:

1. **Layer 1 — 2.5D UI Perspective & Spatial Transforms (`Mat4`)**:
   - Add `Transform3D` (perspective, rotate X/Y/Z, translate X/Y/Z, scale, transform-origin) to `octara-model::style` / `Visual` / `SpatialTable`.
   - `SpatialTable` keeps a 2D translation fast path plus an optional `Mat4` uniform index; vertex shaders apply `clip_pos = mat4 * vec4(local_xy, 0.0, 1.0)` using the existing `Depth` buffer, with offscreen layer rasterization when a transformed container has children.
   - `octara-host-web/src/css.rs` projects 1:1 to standard CSS `transform: perspective(...) matrix3d(...)` and `transform-origin`.
2. **Layer 2 — Zero-Copy `ExternalTexture` SPI (3D Viewports & Hardware Video)**:
   - Expose `ExternalTextureHandle` in `octara-model` / `octara-render`:
     - `Metal` (`Retained<MTLTexture>` / `IOSurface` / `CVPixelBuffer` via `CVMetalTextureCache`),
     - `Wgpu` (`Arc<wgpu::Texture>`),
     - `WebCanvas` (browser `<canvas>` WebGL/WebGPU compositor slot).
   - Composited directly inside `ImagePipeline` with **0 pixel readback, 0 airspace occlusion bugs**, full rounded-corner clipping, shadows, opacity, 2.5D `Mat4` transforms, and Octara UI/modals composited seamlessly on top.

---

## 3. Phased Implementation Plan & Hard Exit Gates (`0.0.11`)

### Wave E1 — Empirical 144 Hz Clock, Zero-Stall Resize, Precision Wheel Coalescing & Backend-Agnostic `octara-render` / `octara-gpu`
1. Run the 4-way Apple display-clock empirical benchmark on M4 dual-display (60 Hz + 144 Hz) and land the winning clock + `NSWindowDidChangeScreenNotification` re-binding.
2. Implement VSync-coalesced zero-stall resize (`pending_resize`, non-fatal in-flight draw completion, `inLiveResize` `presentsWithTransaction` + `kCAGravityTopLeft`) and verify **0 `presentedTime = 0` livelocks** under 60 Hz continuous resize + smooth recovery (`< 50 ms`).
3. Fix `WindowRuntime::can_coalesce_wheel()` so 120 Hz wheel bursts on `virtual_each`/transcripts coalesce cleanly per VSync (verified with deterministic tests).
4. Split `octara-render` into device-free `plan` + optional `wgpu` backend; introduce backend-polymorphic `GpuBackend` in `octara-gpu` and decouple `octara-host-native/src/window.rs` & `shell.rs` from hardcoded `wgpu` types.
5. Gate: `check_layers.py`, strict Clippy, full workspace tests, dual Wasm + 4 modules Wasm, iOS Simulator, Linux ARM64 all green.

### Wave E2 — First-Party `octara-render-metal` (Zero-`wgpu` / Zero-`naga` on Apple + Single-CB Zero-Bounce Presentation)
1. Build `crates/octara-render-metal` (4 native MSL micro-pipelines, `MTLStorageModeMemoryless` Early-Z depth, 5K `DamageRect` persistent `Private` backing + single-CB blit, `SpatialTable` + retained `MTLBuffer`s, single-CB zero-bounce `presentDrawable` for non-transactional frames, `maximumDrawableCount = 2`, hardware `FrameStageTimings` telemetry, optional `custom-wgsl` feature).
2. Switch `octara-host-native` default Apple backend (`macOS 15.4+` / `iOS 18.4+`) to `octara-render-metal` (keeping `--features wgpu` opt-in for A/B pixel parity testing).
3. Gates:
   - `cargo tree -p octara-host-native` on macOS & `aarch64-apple-ios-sim` (default) shows **0 `wgpu`** and **0 `naga`**.
   - CPU Reference Oracle ↔ `octara-render-metal` ↔ `octara-render (wgpu)` **pixel-exact parity** across all glyph, quad, shadow, image, rounded-clip, gradient, path, and 5K `DamageRect` fixtures.
   - Live macOS 60 Hz Retina & 144 Hz external latency benchmark proving elimination of the 3-CB overhead and measurable compression of acceptance → `presentedTime` tail latency (with per-stage `GPUStart/End` + `presentedTime` breakdown).
   - Live iOS Simulator Metal rendering + macOS AppKit / WebKit underlay / 60 Hz live resize regression green.
   - Measure and report macOS & iOS release binary size reduction.

### Wave E3 — 2.5D `Mat4` Layer Transforms & Zero-Copy `ExternalTexture` Compositor
1. Implement `Transform3D` (`Mat4` in `SpatialTable`, offscreen layer pass when needed, 1:1 CSS `matrix3d` in `octara-host-web/src/css.rs`) across `octara-render-metal`, `octara-render (wgpu)`, `CpuRenderer`, and Web DOM.
2. Implement zero-copy `ExternalTexture` compositing (`MTLTexture`/`IOSurface`/`CVPixelBuffer`, `wgpu::Texture`, Web `<canvas>`) with rounded clips, shadows, opacity, and UI overlays on top.
3. Gates: CPU ↔ Metal ↔ wgpu ↔ Headless Chrome parity tests for 2.5D card flip/tilt + zero-copy `MTLTexture` external 3D/video overlay compositing; full workspace & cross-target release gates green.

# 21 — Direct Apple Metal Renderer (`octara-render-metal`) & 2.5D / Zero-Copy 3D Compositor (`0.0.11`+)

> **Design premise**: Zero `wgpu` / zero `naga` on macOS `15.4+` and iOS `18.4+`. Keep Octara's core rendering engine ultra-lean as a **2D + 2.5D UI compositor**, while providing a **first-class zero-copy GPU texture bridge** for embedded 3D engines, video decoders, and custom shaders.  
> **Architectural seam**: Plugs directly into `SPI-1` (`SurfaceRenderer` in [12 — Pluggable SPI Architecture](12-pluggable-spi-architecture.md)) with bit-for-bit CPU Reference Oracle parity.

---

## 1. Strategic Motivation: Why a Direct Metal Backend & Why 2D+2.5D + Zero-Copy 3D

### 1.1 Eliminating `wgpu` Tax on Apple Silicon & iOS
- Octara's UI renderer consists of **4 specialized 2D micro-pipelines** (`Glyph`, `Quad`, `Shadow`, `Image` — ~350 lines of shader code total) plus `Early-Z` opaque cores, `Texture2DArray` atlases, `SpatialTable` scroll/transform uniforms, and 5K `DamageRect` partial rasterization.
- Carrying `wgpu` on macOS & iOS pulls in **~120+ transitive crates** (`wgpu-core`, `wgpu-hal`, `naga`, `metal`, etc.), adds **4–8 MB** to iOS binaries, slows clean builds, adds per-frame CPU validation overhead, and duplicates `objc2-metal` / `CAMetalLayer` handling that `octara-host-native` already owns directly for `CADisplayLink`.
- By implementing a first-party **`octara-render-metal`** backend (~1,500–2,000 LoC directly over `objc2-metal` + MSL) behind `SPI-1` (`SurfaceRenderer`), macOS and iOS drop `wgpu` and `naga` completely, while Linux (Vulkan), Windows (DX12), and opt-in WebGPU continue sharing `octara-render`'s `wgpu` backend.

### 1.2 Clear Boundary for "3D in UI"
Octara explicitly rejects turning its UI core into a bloated 3D game engine (no glTF loader, PBR scene graph, skeletal animation, or physics in core crates). Instead, UI 3D needs are cleanly separated into two first-class layers:

1. **Layer 1 — 2.5D UI Perspective & Spatial Material (Built Into Core)**:
   - Covers ~95% of UI "3D" needs: card flips (`rotateY`), hover tilt/parallax, perspective carousels, 3D modal sheet depth stacks, and frosted/Liquid Glass refraction (`backdrop-filter`).
   - Requires **zero 3D mesh pipelines**: just a `Mat4` homogeneous perspective transform per composited layer (`clip_pos = mat4 * vec4(local_xy, 0.0, 1.0)`), offscreen layer rasterization when a transformed subtree has children, and our existing `Depth` buffer (`Depth16Unorm` / `Depth24Plus`).
   - Maps **1:1 to CSS `transform: perspective(...) matrix3d(...)`** in `octara-host-web` with zero semantic divergence.
2. **Layer 2 — Zero-Copy External GPU Texture Bridge (`ExternalTexture` SPI)**:
   - Covers real 3D viewports and hardware media: embedded RealityKit / SceneKit / Bevy / Filament / custom Metal or Vulkan 3D renderers, 3D Gaussian Splats, CAD viewports, maps, and hardware-decoded `CVPixelBuffer` video frames.
   - An external renderer draws into a GPU texture on the shared device (or `IOSurface`) and registers an `ExternalTexture` handle with Octara.
   - Octara composites that texture directly inside its `ImagePipeline` with **0 pixel readback, 0 airspace occlusion bugs, rounded-corner clipping, shadows, opacity, 2.5D `Mat4` transforms, and arbitrary Octara UI/modals composited seamlessly on top**.
   - Procedural 3D effects (e.g., raymarched AI voice orbs, audio visualizers, liquid glass shaders) continue using `CustomPaint` fragment shaders.

---

## 2. Direct Metal Backend Architecture (`crates/octara-render-metal`)

1. **Pure `objc2-metal` Implementation of `SPI-1` (`SurfaceRenderer`)**:
   - Consumes the exact same device-free `FramePlan`, `DamageRect`, `SpatialTable`, and `RetainedGpuBuffers` produced by `octara-render`'s pure planner.
   - 4 native MSL micro-pipelines (`glyph.metal`, `quad.metal`, `shadow.metal`, `image.metal`) using Metal Vertex Pulling (`VsOut = 4 B` `inst_id: uint`), 4-vertex `TriangleStrip`, `texture2d_array`, and hardware `sampler`.
   - **Apple Silicon TBDR Optimization**: Uses `MTLStorageModeMemoryless` for the `Depth` attachment (zero DRAM bandwidth for Early-Z Hi-Z passes!) and persistent `MTLStorageModePrivate` backing textures for 5K `DamageRect` partial-region rendering + `MTLBlitCommandEncoder` / scissor copy to `CAMetalDrawable`.
   - **Direct `CADisplayLink` Presentation**: Encodes straight into the on-demand `CAMetalDrawable` with `presentDrawable:afterMinimumDuration:` / `addPresentedHandler:` and native `MTLCaptureManager` Xcode GPU Capture support.
2. **Opt-In `CustomPaint` Strategy**:
   - Native MSL shaders run with **0 `naga`**.
   - Cross-platform WGSL `CustomPaint` programs are supported behind an optional `custom-wgsl` feature (compiling WGSL → MSL lazily and caching the `MTLRenderPipelineState`), so standard UI apps remain 100% `naga`-free.
3. **2.5D `Mat4` Layer & `ExternalTexture` Contract**:
   - `Transform3D` / `Perspective` on `Visual` / `SpatialTable` (`Mat4` in GPU uniform buffer; CSS `matrix3d` in `octara-host-web/src/css.rs`).
   - `ExternalTextureHandle` enum:
     - `Metal { texture: *mut c_void /* Retained<MTLTexture> */, size: [u32; 2], srgb: bool, premultiplied: bool }` (plus `IOSurface` / `CVPixelBuffer` zero-copy wrapper via `CVMetalTextureCache`);
     - `Wgpu { texture: Arc<wgpu::Texture> }` (Linux / Windows / WebGPU);
     - `WebCanvas` (browser `<canvas>` WebGL/WebGPU zero-copy compositor slot).

---

## 3. Target Milestone & Verification Gates (`0.0.11`+)

1. **Apple Zero-`wgpu` Verification**:
   - `cargo tree -p octara-host-native` on macOS & `aarch64-apple-ios-sim` (default features) shows **0 `wgpu`** and **0 `naga`**.
2. **Pixel-Exact Parity**:
   - CPU Reference Oracle ↔ `octara-render-metal` ↔ `octara-render-wgpu` pixel-exact parity across all glyph, quad, shadow, image, rounded clip, 5K `DamageRect`, 2.5D `Mat4` perspective, and `ExternalTexture` test fixtures.
3. **Performance & Binary Footprint**:
   - Measurable reduction in macOS/iOS binary size (`>= 4 MB` saved on iOS release) and steady-state 120 Hz Metal CPU encode time (`< 0.3 ms` on 5K partial damage frames).

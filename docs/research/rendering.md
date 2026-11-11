# Research: Rendering and Platform Layer

2026-11. Compares Rune (source and its research docs), GPUI (Zed `main` 985bbf4 and the official blog) and the Rust ecosystem
(Vello, wgpu, winit, Skia, Slint, Makepad, Iced, Floem, Freya, Blitz, Xilem, egui). The resulting design is in [Rendering design](../design/rendering.md).

## Threads and frames

| | Rune | GPUI |
| --- | --- | --- |
| Threads | PLAN says the render thread owns animation; **the actual code is entirely on the main thread** (README:2911) | Everything on the main thread; the background only runs async tasks |
| Per frame | Commit if changed → Taffy → sample animations → rebuild instance list → submit, present | Re-render dirty views → layout → prepaint/paint → new `Scene`; cached views replay the previous frame's paint operations |
| Cadence | macOS uses CVDisplayLink (tried CAMetalDisplayLink, which ran 3–5 frames ahead; abandoned); other platforms use winit `request_redraw`; no drawing when idle | macOS 14+ uses CAMetalDisplayLink, iOS uses CADisplayLink, Windows uses DWM; triple buffering, steady state does not wait for the GPU |
| Animation | Analytic spring tracks sampled at frame time; animation does not re-run views | — |
| Latency | Live values are read only after the drawable is acquired; `input_time()` records input to encode | Built-in input-to-frame latency histogram |

## Scene and renderer

| | Rune | GPUI |
| --- | --- | --- |
| Scene | One flat `Vec<Instance>` rebuilt each frame, 128-byte instances, a dozen or so kinds (rounded rect + border, shadow, glyph, gradient, image, triangle, layer, ...) | One `Vec` per primitive kind (Quad, Shadow, Path, Underline, monochrome / polychrome / subpixel sprites, Surface); an R-tree assigns draw order by overlap, then batches |
| Clipping | Each instance carries a rect clip, four-edge softening, rounded-corner clip | Rect clip only |
| Opacity | Offscreen layers (Rgba16Float) implement group opacity, blur, blending | Just multiplied into the color; no group compositing |
| Shaders | One WGSL uber shader; shadows use Evan Wallace's analytic formula | One set per backend: MSL, HLSL, WGSL; SDF rounded rects, the same shadow formula; paths triangulated with lyon + 4× MSAA |
| Backend | wgpu 30 (Vulkan / DX12 / WebGPU); macOS defaults to a self-written Metal backend (via `wgpu::custom`, about 4.7k lines) | Metal on macOS / iOS, DX11 on Windows (Vulkan fails on many machines), wgpu on Linux / Web (Blade dropped 2026-02) |
| Batching | One pipeline, split only at atlas pages, effects and layer boundaries; settings page: 174 instances in 1 draw | Batched by primitive kind and draw order |
| Cost | Settings page averages 0.13 ms CPU per frame; 2000-instance frame 54 µs; cold start about 300 ms (`Renderer::new` 232 → 85 ms after precompiling shaders) | 120 FPS; empty wgpu window uses about 100 MB of memory (unconfirmed) |

Why not a general-purpose renderer:

- **Rune**: each Vello compute dispatch takes 0.4–0.8 ms, and it was moved to `research/`; the Skia binary is 15–25 MB and takes
  over the encoder; femtovg, tiny-skia and vello_gpu lack per-quad analytic rounded corners, multi-layer shadows, frosted glass and custom shaders.
- **GPUI**: Pathfinder was not fast enough; a few hand-written shaders beat a general vector engine.
- Both reach the same conclusion: the vast majority of UI is rounded rects, shadows and text, so draw it directly with a few instanced primitives.

## Text

| | Rune | GPUI |
| --- | --- | --- |
| Shaping | Parley 0.12 (HarfRust, skrifa) | Per platform: CoreText, DirectWrite, cosmic-text + swash |
| Rasterization | swash / zeno and skrifa outlines; several smoothing modes, macOS default ports CoreGraphics' treatment (error within 3/255) | Platform rasterization; 4 subpixel variants in x; LCD subpixel anti-aliasing |
| Atlas | R8 pages of 2048², keyed by x phase (1/60 px) buckets | etagere packing; emoji are polychrome sprites |
| Fonts | Ships Inter; system fonts via fontique, CJK and symbols fall back on demand | Default `.SystemUIFont`, fallback by list |

## Platform layer

| | Rune | GPUI | Ecosystem |
| --- | --- | --- | --- |
| Window | winit 0.30.13 plus native patches | Self-written per system (AppKit, Win32, Wayland/X11, UIKit) | winit 0.31 beta adds `safe_area`, IME hints and a real lifecycle |
| IME | Via winit's `Ime` events (NSTextInputClient not implemented directly) | `InputHandler` modeled on NSTextInputClient, wired to UITextInput, XIM | winit mobile: Android has no composition and no insets; iOS only has `UIKeyInput`, no CJK composition |
| Accessibility | AccessKit 0.25 (macOS, UIA, AT-SPI), tree built only when a client exists | AccessKit only integrated in 2026-05 | AccessKit has Android and iOS adapters, roughly usable |
| Mobile | iOS embedding only; no Android | iOS experimental as of 2026-10; no Android | Only Slint, Makepad and egui ship on both iOS and Android (limited text input) |

## Testing and screenshots

- Rune: the agent CLI (`--snap`, `--tree`, `--script`, `--video`) and the window use the same renderer and a fixed virtual clock; **reference renderer `rune-ref`**:
  a scalar CPU implementation of the same formulas, BLAKE3 goldens, differential fuzz testing against wgpu output (per-channel error ≤ 2).
- GPUI: deterministic scheduler `TestAppContext`; `HeadlessAppContext` can attach the real text system and take screenshots; the visual test platform is macOS only.
- Ecosystem: `vello_cpu`'s f32 pipeline suits goldens; egui_kittest stores PNGs, compares with tolerance and writes `.new.png` on failure;
  bundled fonts, fixed scale, frozen time; pixels and the semantic tree are snapshotted together.

## Lessons

- Driver coverage of the backend matters more than API elegance (GPUI: Vulkan fails widely on Windows; Blade was dropped).
- Cold start is dominated by shader compilation, so precompile or cache.
- Accessibility, IME and mobile must enter the kernel design from the start; GPUI added them years late, and retrofitting touched everything (stable IDs, roles, insets, gestures).
- Frame cadence on each platform is its own engineering project (ProMotion, display link, Windows nested message loops).
- Two renderers (real and reference) drift apart if the formulas are implemented separately; pin them with differential testing.

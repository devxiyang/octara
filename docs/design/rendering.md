# Rendering

Status: settled (2026-11). Based on [rendering and platform layer research](../research/rendering.md). Follows Rune and GPUI: draw a small number of
instanced primitives directly, with no general-purpose vector engine.

## 1. Layers

```
Kernel (retained tree, layout, text)
  └─ Display list DisplayList: primitives in paint order, logical pixels, pure data (can cross threads)
        ├─ CPU reference renderer: deterministic PNG (screenshots, golden images, the Agent's "eyes")
        └─ GPU renderer: wgpu, one set of WGSL instanced SDF shaders, shared by all platforms
Platform layer: winit (desktop); a thin self-written layer on mobile (iOS UITextInput, Android InputConnection, safe areas)
```

- The display list is in the core crate `octara` (the `paint` module), the renderers are in `octara-render`, and the core does not depend on the GPU.
- The two renderers implement the same formulas (rounded-rectangle coverage, border, clipping, glyph blending), pinned by differential tests: the GPU
  output and the reference output differ by at most 2 per channel.

## 2. Display list

- Depth-first traversal from the root in child order; the overlay layer is the root's last child, so it is painted on top.
- Primitives:
  - `Quad`: rectangle, corner radii, fill color, per-edge border widths and border color;
  - `Glyphs`: a run of glyphs (font data, font size, color, each glyph's id and baseline position);
  - `PushClip` / `PopClip`: clipping for scroll containers and `overflow_hidden` (rounded clipping comes later);
  - `Shadow`: an offset, expanded, blurred rounded rectangle, painted before the element's `Quad`; the blur radius follows CSS (σ = blur / 2, same as
    Rune), and coverage uses Evan Wallace's formula (same as the GPUI shader);
  - `Quad::gradient`: a two-stop linear gradient (GPUI's `linear_gradient`), the gradient line as in CSS, colors interpolated premultiplied;
  - `Image`: the image is placed into the element's border-box interior according to the fit mode and clipped to the corner radii; bilinear sampling
    (premultiplied, clamped to edge), and the GPU does the same interpolation in the shader with `textureLoad`, matching the reference renderer pixel for pixel.
- Opacity: when a subtree paints only one thing, it is multiplied into the color; otherwise it is group opacity (Rune's offscreen layer):
  `PushLayer` / `PopLayer`, painted opaque within the layer, then composited by opacity, so overlapping parts do not show through each other. The GPU
  splits passes by layer, the layer texture has the same size as the target, and compositing reads per pixel (no filtering).
- Coordinates are logical pixels with scroll offsets already included; screen scale is applied by the renderer.
- Rebuild every frame at first (Rune's settings page takes 0.13 ms per frame); later update incrementally by `Dirty::PAINT`.

## 3. Text

- All platforms lay out with Parley; glyph positions are obtained at the final width and cached per node, paragraph and width.
- Glyphs are rasterized with swash into an R8 atlas; the x direction has subpixel variants; color glyphs (emoji) use an RGBA atlas.
- Headless tests use only the bundled fonts (Inter, IBM Plex Mono), with identical results on any machine; real windows use system fonts and system fallback.

## 4. Threads and frames

- On the main thread first (Rune and GPUI both actually do this); the display list is data, leaving room to add a render thread later.
- Redraw only on change, and paint nothing when still; presentation uses vsync (FIFO). Per-platform display link pacing and input latency statistics come later.

## 5. Steps

1. Display list (the kernel produces primitives, testable without a GPU);
2. CPU reference renderer: deterministic PNG screenshots;
3. wgpu renderer: offscreen rendering, differential tests against the reference renderer;
4. macOS window (winit): mouse, wheel, keyboard, focus, input method (IME), scale factor, redraw on demand, system fonts, AccessKit;
5. Later: Windows / Linux, animation, the mobile thin layer.

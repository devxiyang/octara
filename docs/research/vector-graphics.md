# Vector graphics research: paths, icons, canvas

Sources: the reference clones (paths relative to `~/code/archive/reference/`) and crate versions
checked with `cargo search` (2026-11).

## Rune

- `crates/rune/src/shape.rs`: shapes render in the uber-quad pass without the atlas — a stroke is
  a capsule or box per segment, a fill is ear-clipped into triangles with antialiased boundary
  edges. Butt and round caps only, no miter joins, simple polygons only (no holes, no fill
  rules); translucent joints blend twice. `Path`: `move_to` / `line_to` / `quad_to` / `cubic_to` /
  `close`, flattened at 0.1 pt. Rune's own docs call this a weak spot.
- `el.rs`: `canvas(shape: Shape) -> El` with role image: declarative data, no paint closure; the
  app rebuilds the `Shape` when it changes (sparklines, waveforms); `view_box` maps coordinates
  to the element's size.
- `text/icon.rs`: icons are SVG bodies (a subset: path, rect, circle, ellipse, line, polyline,
  polygon, fill and stroke, caps, joins, even-odd, group opacity, transforms) rasterized on the
  CPU with `swash::zeno` into R8 coverage in the glyph atlas, cached per pixel size, tinted by the
  element's color. Icon sets can be swapped; multicolor icons take a full SVG path.
- Research (`research/12-research-rendering.md`, `PLAN.md`): `vello_cpu` as reference and
  headless renderer and sparse strips for icons and illustrations are "spike" items; compute
  Vello is rejected (compatibility, memory). Its CPU reference renderer has no paths yet.

## GPUI

- `svg()`: parsed with usvg, drawn by resvg into a tiny-skia pixmap at 2× and kept as an alpha
  mask in the sprite atlas (`svg_renderer.rs`, `render_alpha_mask`), tinted by the text color:
  icons are single-color.
- `PathBuilder` (`path_builder.rs`): wraps `lyon::path::Builder`; `fill()` / `stroke(width)`,
  `move_to`, `line_to`, `curve_to`, `cubic_bezier_to`, `arc_to`, `add_polygon`, `close`,
  transforms, `dash_array`; `build()` tessellates with lyon. `window.paint_path(path, color)`; the
  wgpu backend draws paths into a 4× MSAA texture and composites it.
- `canvas(prepaint, paint)`: two `FnOnce` closures run every frame; imperative, no hit testing or
  semantics of its own.
- `ui::Icon`: `IconName` is an enum of embedded SVG files; `IconSize` 10 / 12 / 14 / 16 px;
  `.color(..)` tints the mask.

## Exact

- Icons are semantic (`symbol:back`, `symbol:close`, ...): SF Symbols on Apple, template SVG paths
  elsewhere, tinted as a template.
- Canvas: a Canvas 2D recorder producing a command list (with `Path2D` parsing SVG path data),
  replayed by Vello on Apple and tiny-skia on Linux and Windows.

## Crates (2026-11)

| Crate | Version | Use |
| --- | --- | --- |
| zeno | 0.3.3 | path → coverage mask: non-zero / even-odd, strokes, dashes, transforms; already used by swash |
| tiny-skia | 0.12.0 | full CPU rasterizer |
| kurbo | 0.13.1 | curve geometry: flattening, arcs, SVG path data, bounds, hit testing |
| usvg / resvg | 0.49.0 | SVG parsing and rendering (fine at build time) |
| lyon | 1.0.19 | tessellation; needs MSAA or its own antialiasing |
| vello / vello_cpu / vello_hybrid | 0.11 / 0.3 / 0.2 | compute renderer / sparse strips on CPU / on wgpu without compute; pre-1.0 |

Lucide icons are ISC licensed (ship the notice); 24 × 24 grid, 2 px round strokes.

## Conclusions

1. **Coverage masks on the CPU, shared by both renderers** (Rune's icons, GPUI's SVGs): a path
   is rasterized with zeno into an R8 mask, cached in the atlas by path, scale and subpixel
   phase, and drawn as a tinted mask sprite. The CPU and GPU renderers then draw identical
   coverage, which the renderer parity tests rely on. Large or per-frame content rasterizes into
   a transient mask sized to its bounds. Lyon with MSAA and compute Vello are not taken; sparse
   strips can be revisited once stable.
2. **A real path model** (better than Rune's): non-zero and even-odd fills with holes, butt /
   round / square caps, miter / round / bevel joins, dashes, transforms. GPUI's builder names;
   kurbo for geometry (flattening, arcs, SVG path data, hit testing).
3. **Icons from a bundled set, as path data generated at build time** (no runtime SVG parser),
   tinted by the text color, strokes kept as geometry so weight can vary; a seam for platform
   symbols later (Exact).
4. **Canvas as declarative data** (Rune, Exact's recorder), not GPUI's per-frame closures: a
   closure building the drawing, re-run only when what it reads or its size changes, giving a
   display-list fragment that tests can compare.

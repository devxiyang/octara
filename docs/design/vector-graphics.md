# Vector graphics: paths, icons, canvas

Based on the [research](../research/vector-graphics.md). Status: confirmed; steps 1 to 3 implemented.

## Paths

- `Path` is an immutable builder, like elements: `Path::new().move_to(x, y).line_to(..)
  .curve_to(ctrl, to).cubic_bezier_to(c1, c2, to).arc_to(..).close()` (GPUI's names), and shapes:
  `Path::rect`, `Path::rounded_rect`, `Path::circle`, `Path::ellipse`, `Path::svg("M3 12h18")`
  (SVG path data). Stored as its own commands (shared, cheap to clone) with a content hash, so
  renderers cache masks by what a path is; `contains(point, rule)` hit-tests it.
- Painting a path: a fill with a rule (`FillRule::NonZero`, `EvenOdd`; holes allowed) and / or a
  stroke: `Stroke::new(width).cap(LineCap::Round).join(LineJoin::Miter).dash(&[4.0, 2.0])`
  (caps butt / round / square, joins miter / round / bevel with a miter limit). Paint is a token,
  a color or a gradient, as for boxes. A transform (affine) applies to the path.

## Rendering

- The display list gains a path item: the path, its transform, fill or stroke, paint, the clip
  and the source node, ordered with quads, glyphs and images.
- A stroke becomes an outline with kurbo (zeno's own stroker bevels every join sharper than a
  right angle, whatever the miter limit), in the path's coordinates, so a transform shapes the
  pen as it shapes the path.
- Coverage is computed on the CPU with zeno (already used by swash for glyphs) into an R8 mask,
  keyed by (path content, the transform's linear part, quarter-pixel phase, style, visible
  region), and drawn by both renderers as a mask filled with its paint. The CPU reference
  renderer and the GPU renderer therefore draw the same coverage, and the parity tests hold.
- Only the visible part is rasterized (the path's box within the innermost clip and the target),
  so a path larger than the window costs the window. The GPU renderer stores a mask in tiles of
  at most 1024 pixels in a 4096 mask atlas.
- Content that changes every frame (a canvas animating) makes new keys: the reference renderer
  drops masks unused for 120 frames; when a GPU atlas fills up, every atlas starts over and the
  frame is built again with only what it draws (glyphs and images included, which before kept
  failing once their atlas was full).

## Icons

- `icon(icons::SEARCH)` is an element: square, `size_4` (16 px) by default, sized with the usual
  methods (`size_5`, `size_6`); drawn in the current text color (`text_color(..)` tints it), so an
  icon in a button follows the button's color and states.
- The set is Lucide (ISC, the same family Zed's icons come from), every icon (2052 in
  lucide-static 1.72.0, aliases left out), in a crate of its own (`octara-icons`, like
  `octara-fonts`): `examples/generate.rs` runs each SVG through `Icon::svg` and writes path data
  and fill / stroke flags into `src/lucide.rs`, checked in, so no SVG is parsed at run time.
  Strokes stay geometry (24 × 24 grid, 2 units), so `.stroke_width(..)` can vary them.
  `icons::by_name`, `icons::search` (names and Lucide's tags) and `icons::ALL` find them.
- SVG path data is parsed by kurbo (zeno's parser misses arcs repeated without their letter,
  which Lucide uses).
- Apps add their own: `Icon::svg(include_str!("logo.svg"))` (a subset of SVG: paths and basic
  shapes, groups, fill and stroke attributes, transforms; colors ignored; parsed by roxmltree
  when called), or `Icon::path(Path)`.
- Semantics: an icon is decorative (hidden from assistive technology) unless it has a `label`,
  which makes it an image. An icon-only button gets its name from its own `label`, and `check`
  reports one without (`UNNAMED_CONTROL`). `inspect` shows a labeled icon's name (`"icon"`).
- Later: a platform seam for SF Symbols and Material Symbols by semantic name (Exact).

## Canvas

- `canvas(move |c| { .. })` is an element whose drawing is built by a closure, given the
  element's size: `c.fill(&path, paint)`, `c.stroke(&path, &stroke, paint)`, `c.rect(..)`,
  `c.text(..)`, `c.image(..)`, `c.transform(..)`, `c.clip(..)`. The closure is a binding: it reruns
  only when what it reads (signals, `live_text`, its size) changes, and its result is a
  display-list fragment, comparable in tests. Nothing redraws per frame unless something changed.
- Layout: a canvas has no content size; it is sized like a box (`w`, `h`, `flex_1`, `size_full`).
- Semantics: a canvas is an image; it takes a `label` (`check` reports an unlabeled, visible
  canvas with a new code). Interaction uses the element's own handlers; `on_drag` and
  pointer events give positions local to the canvas. Labeled hit regions inside a canvas
  (`c.region(path, role, label)`, exposed to `inspect` and accessibility) come in a later step.

## Steps

1. `Path`, `Stroke`, `FillRule`, the display-list item, zeno coverage in the atlas, the CPU
   renderer; screenshot tests.
2. The same in the GPU renderer; parity tests.
3. `octara-icons` (generated Lucide set) and `icon`; `Icon::svg`.
4. `canvas` and its `check` rule; the showcase draws a chart.

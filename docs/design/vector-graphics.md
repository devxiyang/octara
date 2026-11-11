# Vector graphics: paths, icons, canvas

Based on the [research](../research/vector-graphics.md). Status: proposed, awaiting confirmation.

## Paths

- `Path` is an immutable builder, like elements: `Path::new().move_to(x, y).line_to(..)
  .curve_to(ctrl, to).cubic_bezier_to(c1, c2, to).arc_to(..).close()` (GPUI's names), and shapes:
  `Path::rect`, `Path::rounded_rect`, `Path::circle`, `Path::ellipse`, `Path::svg("M3 12h18")`
  (SVG path data). Stored as a kurbo `BezPath`; cheap to clone (shared).
- Painting a path: a fill with a rule (`FillRule::NonZero`, `EvenOdd`; holes allowed) and / or a
  stroke: `Stroke::new(width).cap(LineCap::Round).join(LineJoin::Miter).dash(&[4.0, 2.0])`
  (caps butt / round / square, joins miter / round / bevel with a miter limit). Paint is a token,
  a color or a gradient, as for boxes. A transform (affine) applies to the path.

## Rendering

- The display list gains a path item: the path, its transform, fill or stroke, paint, the clip
  and the source node, ordered with quads, glyphs and images.
- Coverage is computed on the CPU with zeno (already used by swash for glyphs) into an R8 mask,
  cached in the atlas by (path, transform scale, stroke, subpixel phase), and drawn by both
  renderers as a mask sprite filled with its paint. The CPU reference renderer and the GPU
  renderer therefore draw the same coverage, and the parity tests keep holding.
- Content that changes every frame (a canvas animating) skips the cache: its mask is
  rasterized into a transient region sized to its bounds.

## Icons

- `icon(icons::SEARCH)` is an element: square, `size_4` (16 px) by default, sized with the usual
  methods (`size_5`, `size_6`); drawn in the current text color (`text_color(..)` tints it), so an
  icon in a button follows the button's color and states.
- The set is Lucide (ISC, the same family Zed's icons come from), every icon, in a crate of its
  own (`octara-icons`, like `octara-fonts`): path data generated at build time from a pinned
  Lucide release and checked in, so there is no SVG parser at run time. Strokes stay geometry
  (24 × 24 grid, 2 px), so `.stroke_width(..)` can vary them.
- Apps add their own: `Icon::svg(include_str!("logo.svg"))` (a subset of SVG: paths and basic
  shapes, fill and stroke; parsed at first use), or `Icon::path(Path)`.
- Semantics: an icon is decorative (hidden from assistive technology) unless it has a `label`,
  which makes it an image. An icon-only button gets its name from its own `label`, and `check`
  reports one without (`UNNAMED_CONTROL`).
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

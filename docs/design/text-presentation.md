# C1: one text presentation result

Status: design and reproduced defect, not implemented. Follows C1a's shared TextBox constraints.

## Evidence

A temporary headless regression on 2026-11-17 mounted:

```rust,ignore
text(rich().text("visible\n").link("hidden", "/hidden"))
    .id("text").w(200).line_clamp(1)
```

The assertion that the hidden link had no rectangles failed: `link_rects` returned
`Rect { x: 0, y: 20, w: 51.710938, h: 20 }`. This establishes a geometry mismatch;
it does not by itself prove an OS click, keyboard activation or screen-reader announcement.
The temporary test was removed after recording the reproducer; no failing/ignored regression
is added to the passing suite. Restore it as a required passing test with implementation.

`octara-text::LaidText::glyph_runs` applies max_lines, fit width and ellipsis, but
`visual_lines` only receives wrap width. Kernel rich links, document selection and AccessKit
runs consume the latter. TextBox removed duplicate box arithmetic, not this semantic split.

Inputs already share `Field` for paint, hit testing and navigation. Native queries rebuild
from TextInputLayout plus the platform's current TextDocument and also use Field::reveal.
That independent document input is intentional: synchronous marked-text queries can precede
the next committed frame. Sharing geometry must not force them to read stale committed text
or enter application handlers while the text engine is borrowed.

## Decision

Move visibility decisions into the text layout implementation, where glyph clusters, source
byte ranges and line metrics are available. Return one presentation result describing drawn
glyphs, visible source fragments/caret stops and synthetic ellipsis geometry. Consumers must
not reconstruct visibility by comparing final glyph positions or by slicing UTF-8 bytes.

Keep full-document measurement separate: intrinsic sizing and editable navigation can need
the full layout. Presentation applies wrap width, fit width and line limits once. The exact
Rust names remain an implementation choice; do not add a runtime/public UI primitive.

Implement this inside octara-text first, retaining its downward-only dependencies. Migrate
Mono/Parley and custom test engines together; remove the superseded split presentation APIs
after callers move. Do not add a default fallback that silently returns unbounded visual lines.
Reuse the shaped layout and one keyed presentation cache; do not retain independent glyph and
visibility caches with different keys. Invalidation includes text/runs, font generation,
constraints and relevant style. Preserve incremental paragraph shaping and unchanged-prefix reuse.

## Consumer contracts

| Consumer | Required fact |
| --- | --- |
| Paint | Glyphs and synthetic ellipsis from the presentation result |
| Link hit geometry | Intersection of link source range with visible source fragments |
| Focus/activation | Fully hidden links cannot receive focus or be activated; partly visible links retain their allocation |
| Selection paint/hit | Only visible fragment geometry; ellipsis maps to a documented visible boundary, never an invented source character |
| Document copy | Preserve full logical text for explicit whole-document copy; clipping is presentation, not deletion |
| Accessibility | Full logical text may remain available, but positional runs and actionable links must describe visible content consistently |
| Editable/IME geometry | Full editable layout with scroll/reveal transform; synthetic document ellipsis must not enter native text offsets |

The visibility of a link is not its identity: resizing may hide and reveal the same allocation.
Existing target guards still check identity, while activation qualification checks visibility.
Changing link words/destination retains B2's replacement rules. Do not destroy/recreate links
on every width change or use developer IDs as continuity keys.

The copy and accessibility policies above must be exercised explicitly during implementation;
preserving logical text does not authorize inventing offscreen positional rectangles.

## Implementation and acceptance

1. Add the shared presentation result in octara-text. Cover one-line truncation, max_lines,
   hard newlines, empty lines, tiny widths, Chinese/emoji, ligatures and bidirectional text.
   Ellipsis is synthetic and must not accidentally inherit the final link's action.
2. Migrate Kernel glyph painting, link ranges, selection and AccessKit positional runs.
   Add visibility qualification for internal links using that same result. Retire old caches.
3. Consolidate the common Field construction/reveal calculation used by committed input and
   native query layout. Keep the current-document input and try-borrow failure behavior explicit.
4. Verify hidden/partly visible links by point, keyboard and guarded Agent requests; resize
   to hide/reveal and assert retained identity without hidden activation. Verify selection and
   copy independently, plus CJK/emoji caret/range round trips with composition and scrolling.
5. Run workspace/renderer regressions and focused shaping/cache checks. Obtain separate real
   window evidence for IME, scrolling and candidate positioning; headless evidence is insufficient.

No broader context, Signals, lifecycle or identity redesign is required for this change.

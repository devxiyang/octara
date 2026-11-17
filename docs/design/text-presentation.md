# C1: one text presentation result

Status: C1b implements the shared presentation result and ordinary document consumers.
C1c shares editable Field/reveal construction; real-window IME evidence remains unfinished.
Follows C1a's shared TextBox constraints.

## Evidence

A temporary headless regression on 2026-11-17 mounted:

```rust,ignore
text(rich().text("visible\n").link("hidden", "/hidden"))
    .id("text").w(200).line_clamp(1)
```

The assertion that the hidden link had no rectangles failed: `link_rects` returned
`Rect { x: 0, y: 20, w: 51.710938, h: 20 }`. This establishes a geometry mismatch;
it does not by itself prove an OS click, keyboard activation or screen-reader announcement.
The reproducer is now a required passing integration test in `text_presentation.rs`, alongside
selection, copy and activation checks. The hidden second-line link has no rectangles and is
absent from the actionable accessibility/Inspector projection.

Before C1b, `octara-text::LaidText::glyph_runs` applied max_lines, fit width and ellipsis, but
`visual_lines` only received wrap width. Kernel links, selection and AccessKit consumed the
latter. C1b removes `glyph_runs` and the independent Kernel glyph/line caches. `LaidText::present`
returns `TextPresentation`, and every ordinary document consumer uses that one result.
`visual_lines` remains an explicitly full editable-layout API for input navigation and native
queries; it is not a document presentation fallback.

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
the full layout. Presentation applies wrap width, fit width and line limits once. `TextPresentation` contains glyph runs and presented lines with visible source fragments;
`TextEllipsis` records synthetic geometry and its caret boundary. These are text-engine data,
not runtime/public UI primitives.

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

## Implemented document contract

- Parley records grapheme/source byte ranges, direction and advances before glyph extraction.
  Shaping atoms record fragment/glyph intervals, so a ligature is retained or removed whole.
  Font metadata and glyph buffers are allocated per styled font run, not per grapheme. Mono
  supplies the same source projection without drawing glyphs. There is no unbounded default
  implementation for custom engines.
- `present` applies a line limit and fits the final presented line. `Wrap::Truncate` explicitly
  presents one line, including when the logical text contains hard newlines; this corrects the
  former paint/measurement disagreement. A zero line limit produces no presented lines.
  A width too small for the ellipsis omits it and retains only complete atoms that fit.
- Fitting keeps the physical left portion, then places the ellipsis to its right, including in
  bidi text. Its caret maps to the source boundary at the physical right edge of the retained
  fragments, or the original line start if none fit. This is a source boundary, not an invented
  character. Ellipsis glyphs use brush zero and cannot inherit the final link's styling/action.
- Document range geometry intersects visible fragments and merges touching visual intervals;
  it does not deduplicate bidi affinities by byte or assume RTL ranges have increasing x.
  Selection painting excludes the ellipsis and hidden line breaks. A hard newline gets a
  positional fragment only while its following line is also presented. Explicit document
  selection/copy still reads the complete logical string.
- AX positional runs are assembled from visible source fragments, splitting at hidden gaps,
  direction changes and character limits. Character positions come directly from those
  fragments. The text node retains its full logical value/label. A hidden selection endpoint
  maps to the closest visible source boundary (downstream on equal-distance boundaries).
- Internal link allocations are unchanged by constraints. Shared shown/activation checks and
  the AX projection reject fully hidden links; partly visible links keep their original target.
  A current guarded target that is hidden is refused with `STEP_FAILED`, not `STALE_TARGET`.
  Revealing it permits activation with the same target again.
- One Kernel presentation cache is keyed by content/runs version, wrap/fit constraints, text
  style, font generation and content origin. Layout invalidates the AX child projection before
  its dirty set is drained, even for fixed-height texts whose visible link set changes without
  a height change. Revealed links are sent again with fresh bounds. Paragraph shape caching and
  unchanged-prefix reuse remain unchanged.

## Acceptance and remaining work

- [x] Engine cases: one-line fitting, line limits (including zero), hard newlines, empty lines,
      tiny widths, Chinese/emoji, a real multi-grapheme Inter ligature and bidi affinities.
      Mixed size/weight/slant/CJK/bidi glyph positions match Parley's original positioned glyphs
      at unbounded and wrapped widths.
- [x] Ordinary paint, link ranges, selection hit/paint and AccessKit positional runs use the
      shared result. Superseded document caches and the old glyph extraction API are removed.
- [x] Historical hidden second-line link, partly visible link/ellipsis, direct/pointer/keyboard/
      accessibility activation, guarded Agent refusal, repeated resize identity preservation,
      fixed-height incremental AX updates and full logical copy have passing headless regressions.
- [x] Consolidate Field construction/reveal for committed input and current-document native
      queries. Kernel creates the configuration before building Field; both paths share empty
      insertion-line construction, reveal and geometry conversion. Only Kernel persists scroll
      and its keep key. Native layout shapes its current document with try-borrow, without
      entering handlers or mutating the published configuration; empty documents need no engine.
      Kernel retains incremental paragraph reuse and secure masking before construction.
- [x] Qualify editable CJK/emoji composition and scrolling against that common construction.
      Prediction matches subsequently published geometry across repeated widths, manual scroll,
      selected/hidden composition cursors, cancellation, unmark and clearing. Existing marked
      range/caret round trips and busy-engine refusal/recovery remain passing requirements.
- [ ] Obtain real-window IME, scrolling and candidate-position evidence. No real window/OS input
      or screenshot was used in C1b.

C1b validation (2026-11-17, Rust 1.100.0): **900 workspace tests passed, 0 failed,
3 existing ignored doctests**. The full run includes renderer, Chat queued long-paragraph/code
bursts, HTTP, multiline and native-query regressions. Workspace all-targets Clippy, strict
`octara`/`octara-text` rustdoc, no-default-features core build, formatting, diff/layer checks pass.
The first sandboxed workspace attempt could not bind the local HTTP test sockets; the authorized
loopback-enabled rerun passed.

The existing `bench --stream` completed using the active debug cache: 5.2 glyph runs built per
token in both motion cases, 1.4/2.8 drawings built and 48/64 AX nodes compared. These are headless
debug work observations, not a release CPU comparison or a native frame-time claim; no new
release build cache was created. No broader context, Signals, lifecycle or identity redesign
was introduced.

C1c validation (2026-11-17): **901 workspace tests passed, 0 failed, 3 existing ignored
doctests**; workspace all-targets Clippy, formatting, diff and dependency-layer checks passed.
The new transition regression compares synchronous predictions with published field geometry
for both single-line and multiline inputs. No real-window or OS candidate validation was run.

## Follow-up architectural review

The [state-to-presentation review](editing-architecture-review.md) rechecks C1b/C1c.
Shared Field construction is retained, but `Kernel::field` still persists reveal state during
reads. R2 moves that mutation into explicit settlement; C1c does not establish read-only geometry.
The review also distinguishes document bidi coverage from editable caret-affinity coverage.
These are planned follow-ups, not changes implemented by this document.

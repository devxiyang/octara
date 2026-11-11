# Research: Layout

2026-11. Compares how Rune, Floem and Exact (source read) and SwiftUI, Compose, Flutter, GPUI, Iced, Slint, Makepad, Xilem/Masonry, Freya and Blitz (docs read)
approach layout. Conclusions and decisions are in [Layout design](../design/layout.md).

## Layout models

| Model | Frameworks |
| --- | --- |
| CSS Flexbox / Grid (Taffy) | Rune, GPUI, Floem, Blitz, Exact (patched Taffy on native, the browser on Web) |
| Constraints down, sizes up | Flutter, Compose, Iced |
| Parent proposes, child chooses | SwiftUI |
| Custom | Slint, Makepad, Freya (Torin), Xilem (per-axis `Dim` + measurement cache since 2026-01) |

- **Rune** explicitly rejects proposal-style layout: "agents already know flexbox", and proposal-style is harder to make incremental (`PLAN.md:220`). It also
  evaluated a custom engine: rewriting flexbox quirks would take about 2.5–3.5k lines with high regression risk; measured Taffy at 5,283 nodes in 0.058 ms, so it kept Taffy and wrote down the triggers for re-evaluation
  (`research/32-engine-plan.md`). Rune implements Taffy's traits over its own node storage, with styles packed into 64 bytes.
- **Exact** follows CSS, defaults to `display: block`, and uses Chrome as the reference for native layout; its patch makes dirty marking propagate up only to a replayable clip boundary, and list rows can replay cached subtree layout.
- **Floem** has one Taffy node per view, compares before calling `set_style` when styles change, computes from the window root, and relies on Taffy's cache.

## Default sizing

- **Content-determined is the mainstream**: SwiftUI, Compose, Iced, Freya, GPUI, Rune, Floem. Makepad defaults to Fill, and Slint containers fill by default.
- Default direction: Rune's `el` and Floem's `stack` lay out horizontally, with `row` / `col` stating direction explicitly; Exact and GPUI default to block.
- Changes to CSS defaults:
  - Rune: `flex-shrink: 0`, and `.shrink()` also sets `min_width: 0`; `truncate` sets it automatically. No explanation of the reasoning was found.
  - Floem, Exact, GPUI: copy CSS as is (shrinkable, `min-width: auto`), and document `min-width: 0` in a common-pitfalls page.
- **Most common pitfalls**:
  1. A fill child inside a content-sized parent: under CSS the percentage is treated as auto; in Makepad and Freya the result is 0 or undefined and the UI disappears;
     Iced lets the child's Fill promote the parent to Fill; Flutter reports an error on unbounded constraints;
  2. `min-width: auto` lets long text blow out the parent;
  3. Unbounded scroll axis;
  4. Rune's `grow` is `flex: g 1 0`, which collapses to 0 in a column whose height is content-determined.

## Text

| Wraps by default | Does not wrap by default |
| --- | --- |
| SwiftUI (also truncates automatically), Compose, Flutter, GPUI, Iced, Exact (CSS) | Rune's `text()` (`rich()` wraps), Floem, Slint |

- Ellipsis almost always has to be turned on explicitly. Rune: `truncate` / `truncate_middle` / `truncate_head` / `truncate_fade`, `max_lines(n)`.
- Measurement: all use a measure callback on a Taffy leaf, breaking lines by available width. Rune has two cache levels: glyphs (text, style, scale) and line breaks (plus width),
  so a width change only re-breaks lines (about 2 µs vs. about 30 µs for shaping). Taffy calls measure about 4 times per text run per frame.
- Exact's measurer is injected by the host, and tests use a deterministic monospace measurer; Rune turns off Parley's system fonts and bundles Inter and IBM Plex Mono.

## Scrolling and virtual lists

- Mature frameworks all **measure** row heights: Compose, Flutter, SwiftUI, and GPUI's `list` measure on demand; fixed size is only a fast path (GPUI `uniform_list`) or immature (Freya).
- Rune: `.virtualize(estimated row height)`, saves measured heights by key, anchors on the first visible row; overscan of two screens beyond the viewport.
- Exact: the estimate is only a hint; the host reports each row's measured size, and stale reports are dropped by token.
- Floem: fixed, per-item function, or by default the size of the first rendered item.
- Scroll containers all require a bounded scroll axis: Exact checks at compile time, Rune warns when the viewport collapses to 0, and Compose errors on nested same-direction scrolling.

## Overlays

- Rune: `anchored(key, side, align, gap)`, flips, 8 pt from the window; `anchored_fit` limits size by remaining space (second solve); overlays attach to the root and escape clipping.
- Exact: HTML top layer + CSS anchor positioning (`position-try-fallbacks`); replaced by the system popover on macOS.
- Floem: re-attached to the window root in the layout tree.

## Responsiveness

- Rune: `layout_with("root", |r| r[2] < 500.0)`; the view is re-run only when the result flips; a window resize alone does not run views, only re-solves and re-breaks lines.
- Floem: Bootstrap-style breakpoints (576/768/992/1200/1400). Exact: viewport units, `flex-wrap`, and `resize WxH` in tests.
- SwiftUI `ViewThatFits`, Compose `BoxWithConstraints`, Flutter `LayoutBuilder`.

## Layout diagnostics

- **Flutter is best**: "RenderFlex overflowed by N px" with yellow-and-black stripes; unbounded-constraint errors come with the constraint chain.
- Rune: warnings in a few cases (scroll viewport collapsed to 0, `.growth` misplaced); the layout checks in PLAN (overflow, overlap, truncated text, hit areas smaller than 44 pt) are not yet implemented.
- Exact: compile-time checks (e.g. `lower-scroll-grows`), and the `layout <target>` command can point out the style declarations in effect and the component they came from.
- The shared recommendation from the research: diagnostics should be queryable data, so tests and agents can assert "no layout warnings".

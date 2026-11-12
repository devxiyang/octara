# Layout

Status: settled (2026-11). See [layout research](../research/layout.md) for the basis. Principle: **aimed at desktop and mobile, not web standards**.
Method names follow GPUI (its vocabulary comes from Tailwind, which AI knows best); semantics follow native frameworks, mainly Rune (React Native / Yoga, SwiftUI and Compose are similar).
Where the two disagree, Rune prevails.

## 1. Model

- Taffy solves flexbox and Grid (`grid()`, `grid_cols(n)`, `col_span(n)`, etc., names and semantics as in GPUI). **Every element is a flex container** (as in Rune and React Native); there is no web block layout.
- `el()` / `row()` lay out horizontally, `col()` vertically (as in Rune, corresponding to SwiftUI's HStack / VStack and Compose's Row / Column); constructors take no arguments,
  children are added with `.child(..)` / `.children(iter)` (as in Rune and GPUI), and `.child("text")` shows text directly.
- **The window root node is a column** (as in React Native): content fills the window width and wraps by width; an app root that needs to fill the height writes `.size_full()` (the GPUI idiom).
- **Boxes do not shrink by default** (`flex-shrink: 0`, as in Rune and Yoga); `.flex_shrink()` turns on shrinking and sets the minimum size to 0 (as in Rune's `shrink`).
  **Text can shrink by default**: when it does not fit on a line, the text yields space and wraps (as in SwiftUI's Text).
- Fill the remaining space: `.flex_1()` (the idiom for scroll areas) or `.flex_grow()`; `align-items` defaults to stretch.
- Lengths are logical pixels (iOS pt, Android dp): a number or `px(..)`; `relative(0.5)` is a fraction of the parent; `rems(..)` is a multiple of 16 (as in GPUI).

## 2. Text

- **Wraps by default** (as in GPUI, SwiftUI, Compose, Flutter); `whitespace_nowrap()` for a single line; `text_ellipsis()` for a single line truncated with an ellipsis;
  `truncate()` additionally makes it shrinkable; `line_clamp(n)` limits the number of lines (name as in GPUI).
- `text_xs` … `text_9xl` **set only the font size** (as in GPUI): 12, 14, 16 (default), 18, 20, 24, 30, 36…; line height defaults to 1.25 times the font size
  (as in Rune, changeable by the theme's `line_height`); `line_height(px(22.0))` or `line_height(relative(1.5))` overrides; `leading_tight` etc. are common multiples.
- Text sizes are rounded up to whole pixels (as in Rune), avoiding alternating line heights when the line height is fractional.
- **Text style is inherited** (as in GPUI): the text color, font size, line height, font weight, font family, wrapping mode and `line_clamp` on a container apply to the text inside,
  and what a child sets itself takes priority; in `row().text_sm().child("caption")` the text is 14 px. When an inherited value changes, it propagates only down to the affected subtree.
- Measurement is a callback on a Taffy leaf, using the available width: min-content is the longest unbreakable segment, max-content is the width without wrapping.
- The text engine is a replaceable part of the kernel (`TextEngine`):
  - `ParleyEngine`: the default. Headless, `system-ui` and `ui-monospace` are the bundled Inter and IBM Plex Mono, so tests give identical results on any machine; real windows resolve them to the platform's fonts, with its fallback (Chinese, etc.). See [the text engine](text.md#the-engine).
  - `MonoEngine`: a deterministic monospace model, used for benchmarks that isolate text cost.
- Each text node owns its layout; shaped paragraphs (separated by `\n`) are kept by content and shared ([the text engine](text.md#the-engine)): appending re-shapes only the last paragraph, a width change only re-breaks lines, and a row scrolling back reshapes nothing. The kernel records the prefix bytes unchanged since the last layout, whose paragraphs are reused without comparing them.
- Known limitation: a very long paragraph with no line breaks is fully re-laid out every time (about 16 ms for 100 KB). As in Rune, this is mainly handled by splitting content into blocks (one text per Markdown block).

## 3. Viewport and window size classes

Neither Rune nor GPUI has breakpoints: Rune uses `layout_with` predicates (rerun only when the result flips), and GPUI reads the window size by hand. We keep that approach and add native
**window size classes** (Material 3: compact < 600, medium 600–839, expanded 840–1199, large 1200–1599, extra-large ≥ 1600, in dp),
so that style-only responsiveness need not rerun closures.

1. **Per-size-class styles are resolved by the kernel** (implemented): `.when_medium(|p| ..)`, `.when_expanded(..)`, `.when_large(..)`, `.when_extra_large(..)`
   and `screen(Breakpoint, ..)`, taking effect from small to large. When a boundary is crossed, only the nodes carrying such styles are re-resolved, and no closure runs. Size class is merged first, then interaction state,
   so feedback such as press is visible at any size. Boundary widths are set by the theme.
2. **Use Signals for structural changes** (implemented): `viewport()`, `breakpoint()` (changes only when a size class is crossed). Any condition is written as
   `memo(|| viewport().get().0 < 500.0)`: the Memo's cutoff makes readers rerun only when the result flips, equivalent to Rune's `layout_with`.
3. **Testing** (implemented): `Window::resize`, `Window::breakpoint`, `Window::at_sizes(&[(375.0, 667.0), (1280.0, 800.0)], |w| ..)`
   run the same check at each size and name the size on failure.
4. **Container size** (to do): styles by container width, the same mechanism as window size classes; evaluated by container width after layout, and if a hit flips, layout again,
   with a limit, reporting `LAYOUT_LOOP` when exceeded.
5. **Element size as a source** (implemented): `size_of(id)` notifies only when the size changes, and with `memo(|| size_of("panel").get().0 < 320.0)` reruns only when the condition flips
   (as in Rune's `layout_with`); it stabilizes within the same frame, and on oscillation it cuts off and reports `LAYOUT_LOOP`.

## 4. Layout diagnostics

(Implemented) Queryable after each layout; the result is structured data with the test id, path, creation location (`Window::layout_issues()`), and tests and Agents can assert it is empty.
Overflow is summarized as one entry per container and edge (as in Flutter's "RenderFlex overflowed by N px"), listing the children that overflow:

| Code | Situation | Suggestion |
| --- | --- | --- |
| `OVERFLOW` | A child exceeds the parent's content area (not scrolling, not an overlay) | `flex_shrink()`, `truncate()`, `flex_wrap()`, or put it in a scroll container |
| `TEXT_TRUNCATED` | Text is truncated by `truncate` / `text_ellipsis` or cut off by `line_clamp` (Info) | Ignore if expected; otherwise widen or wrap |
| `COLLAPSED` | An element with content has size 0 | Check whether `flex_1` / `relative(..)` sits in a parent whose size is determined by its content |
| `OVERLAY_CLAMPED` | The overlay fits on neither side and is pushed back into the window (no `fit` set) | `Placement::fit(true)`, or change direction |
| `MISSING_TOKEN` | A custom token not defined by the theme is used (painted magenta) | Define it in the theme |

## 5. Scrolling and overlays

- Scroll containers (`overflow_y_scroll()`, `overflow_x_auto()`, `overflow_hidden()`, `overflow_clip()`, names as in GPUI) can shrink below their content and clip; diagnostics do not count the scroll axis as overflow.
- Virtual lists go inside scroll containers and follow their offset and height; rows start from an estimate, are backfilled by key with measured heights, and the anchor is the first row that has
  been measured and is still in the viewport (like the browser's overflow-anchor and Rune).
- `follow_tail()` (GPUI's `FollowMode::Tail`): a scroll container keeps the end of its content in
  view as the content grows (a transcript, a log, a terminal). The state is the container's, in
  the kernel: following or released. Wherever someone puts the offset (the person's wheel, drag or
  momentum, a reveal, the app's `scroll_to`) it follows if that is the end (within a pixel) and
  lets go otherwise; clamping, glide steps and anchoring do not count. After each layout a
  following container moves to its end (or retargets its glide there); a virtual list inside it
  anchors to the end instead of its first row. `scroll_to_end(id)` glides to the end and follows
  again; `following_tail(id)` is tracked (a "back to latest" button reads it). A hot reload keeps
  a released container released.
- Overlays keep 8 px from the window edge; with `fit` (on by default for `BELOW` / `RIGHT`) their size is limited to the remaining space and they scroll.

## 6. Later

- Real windows: system UI font and system fallback (like GPUI's `.SystemUIFont` and Rune's CJK fallback); headless tests still use only the bundled fonts.
- Bounce at the scroll ends (render thread); mobile has no hover, and press feedback uses `active`.

# TODO

What is open, by area. For phase goals see [ROADMAP](ROADMAP.md); for what is built see
[ARCHITECTURE](ARCHITECTURE.md) and the [designs](design/). When an item is done, remove it and
make the corresponding design document describe it.

## Priorities

1. The [core](design/core.md): what people and agents point at (component boundaries, marked
   captures, picking by point, size explanations); the text core; incremental paint and
   accessibility; the [platform boundary](design/platform.md) completed (several winit windows
   included); popups and menus; the AI chat components and reference app.
2. The owned macOS backend (NSTextInputClient, menus, live resize in the frame's transaction,
   panels), and agents driving a real window (the inspector protocol over a channel to it).
3. Native chrome: menus and shortcuts, file dialogs, alerts.
4. Components: menus and popovers, dialogs and sheets, tabs, lists and tables, tooltips,
   sliders, progress.
5. Images: JPEG, WebP, SVG, loading and caching.
6. [Native Island](design/native-island.md): focus, accessibility joining, lazy creation.
7. Windows: WebView2 in DirectComposition; `surface` textures; Linux: WPE as a texture.
8. Text depth: direct platform text protocols, incremental layout of long paragraphs.
9. iOS, then Android.
10. Evaluation, causal tracing.

## Awaiting acceptance by hand

Checked by tests and screenshots; to be tried in a real window (`cargo run -p octara-examples
--bin window -- showcase fields` shows the "Text input" card alone):

- [ ] Multi-line notes: an input method (pinyin) on the third line puts its candidate window by
      that line; a trackpad scroll inside the field, then typing, comes back to the caret; Up,
      Down, Cmd+Left / Right and Cmd+Up / Down feel right.
- [ ] Chat composer: Enter sends, Shift+Enter breaks the line, the field grows to 4 lines.
- [ ] Password: no input method while it has focus, copy and undo do nothing, Show reveals it.
- [ ] Map (`window -- showcase map`, macOS): the map shows through the card's cutout with
      rounded corners; dragging, scrolling and pinching it move it and the status line follows;
      the + and − buttons over it zoom and are not taken by the map; in the full `showcase`,
      scrolling and resizing keep it in place with its frame, no flicker. A clicked map takes the
      keyboard (Tab back comes with native focus targets).
- [ ] Display captures with the screen unlocked: `OCTARA_CAPTURE=shot.png
      OCTARA_CAPTURE_PIXELS=display cargo run -p octara-examples --bin window -- showcase web`
      reports `pixels: display`, and the picture holds the Metal frame and the live web page,
      cropped to the content view (no title bar), at the backing scale.
- [ ] Web view (`window -- showcase web`, macOS): the page shows in the rounded box, "Other page"
      loads the second page and the status shows its title; in the full `showcase`, scrolling and
      resizing move the view with the page, without lag or a gap, and it clips at the window's
      edge; clicks and scrolling inside the page go to the page.

## To research and design

- [ ] Platform shells and native chrome: menus (`muda` / `NSMenu`), toolbars (`NSToolbar`),
      title bar and traffic lights, sidebars (system material underlay with drawn content, as
      Rune), dialogs; on iOS / Android, native navigation (navigation bars, edge-swipe back, tab
      bars): drawn to match the platform, or one Octara surface per screen inside the platform's
      navigation container; whether winit stays on each platform.

## Layout

Basis: [layout research](research/layout.md), [layout design](design/layout.md).

- [ ] A single very long paragraph (no line breaks) is re-laid out as a whole: 100 KB takes about
      16 ms / token. Rune has not solved this either; it tried "per-word caching + its own line
      breaking", which was only 1.5x faster on a cold start, and gave it up. For the append case,
      consider shaping in chunks split on whitespace within the paragraph and resuming line
      breaking; another route is for the component library's Markdown to split into multiple
      texts by block (Rune's main technique).
- [ ] Container queries (`.container()` + `.container_md(..)`) and styles by container size (the
      same mechanism as window size classes), with a feedback-loop limit (`LAYOUT_LOOP`).
- [ ] System UI font for real windows (GPUI's `.SystemUIFont`) and system fallback (Rune's CJK
      fallback).
- [ ] Tailwind palette constants (`colors::BLUE_500`, etc.), for direct use in the prototyping
      stage.

## Rendering

Basis: [rendering research](research/rendering.md), [rendering design](design/rendering.md).

- [ ] More deterministic fallback fonts: Korean, Traditional Chinese, emoji (color), Arabic, etc.,
      adding features as needed.
- [ ] Inset shadows.
- [ ] More than two color stops, radial gradients.
- [ ] Images: JPEG / WebP / SVG, async loading and caching (together with resources), mipmaps when
      scaled down, large images exceeding the atlas, re-layout after the shape changes.
- [ ] Display list updated incrementally by `Dirty::PAINT`; a layer covers only its extent (it
      covers the whole window).

## Input

Basis: [input research](research/input.md), [input design](design/input.md).

- [ ] Deferring the app's rewrites of the value during composition (Exact); touch long-press word
      selection and selection handles; the caret position within an IME composition.
- [ ] Accessibility `ScrollIntoView`, text selection (AccessKit text runs); scroll into view
      before driving; multi-finger gestures, edge back swipe; direct platform text protocols
      (NSTextInputClient, etc.).

## Async and motion

- [ ] Resource deduplication cache, deferred placeholders, forbidding real time in tests, stream
      backpressure and coalescing ([async](design/async.md)).
- [ ] Appearing and disappearing: same-key revival (Rune), `.transition(..)` (scale, slide in),
      exiting children follow the parent ([animation](design/animation.md)).
- [ ] The shell reads the system's reduced-motion setting (winit does not provide it;
      per-platform interfaces are needed).

## Agents

Basis: [agent knowledge](design/agent-knowledge.md).

- [ ] About thirty percent of public items have no docs (707 / 2221, mostly variants, fields, and
      methods): add docs.
- [ ] Tests and agents read presented values (painted opacity, color): `visual_of` is the target
      value, and the presented value exists only in the display list.
- [ ] Example ids (how an item is used in the examples), recipes and byte limits, `octara new`
      (calls `skill install`), MCP, llms.txt.
- [ ] Causal tracing in the reactive layer (the causal chain reaches the cause id of kernel
      operations).
- [ ] Agent evaluation retest: update the API docs, use harder tasks or a weaker model.

## Known limitations

From section 9 of [Reactive core](design/reactive-core.md):

- [ ] Kernel children are a `Vec`; with a hundred thousand sibling nodes insertion and removal
      are O(n) (avoided by virtualization).
- [ ] After hot reload, virtual list rows are measured again.
- [ ] On hot reload, a change of state type can only reset: add a migration function (serde or
      user-provided).
- [ ] Durability tiers (Salsa), for rarely changing state.
- [ ] Taffy 0.15.1: after a container removes its last absolutely positioned child, the hoisted
      list is not cleared, and rounding reads a deleted node. The kernel gives that container a
      fresh taffy node (`refresh_taffy_node`); report upstream and delete once fixed.

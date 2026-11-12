# TODO

What is open, by area. For phase goals see [ROADMAP](ROADMAP.md); for what is built see
[ARCHITECTURE](ARCHITECTURE.md) and the [designs](design/). When an item is done, remove it and
make the corresponding design document describe it.

## Priorities

1. The [core](design/core.md): general mechanisms and components for the AI chat reference app,
   and the app.
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
- [ ] The GPU renderer converts every item to instances each frame: keep a part's instances
      and upload only the parts that changed. A layer covers only its extent (it covers the
      whole window).

## Input

Basis: [input research](research/input.md), [input design](design/input.md).

- [ ] Deferring the app's rewrites of the value during composition (Exact); touch long-press word
      selection and selection handles; the caret position within an IME composition.
- [ ] Accessibility `ScrollIntoView`, text fields' runs and selection (AccessKit); scroll into view
      before driving; multi-finger gestures, edge back swipe; direct platform text protocols
      (NSTextInputClient, etc.).

## Platform

Basis: [platform design](design/platform.md); done with the core's platform boundary step.

- [ ] Windows: native context menus (`TrackPopupMenuEx`, opened from a posted message as Rune's)
      and alerts (`TaskDialog`), and a menu bar; drawn menus and prompts until then. Linux: a
      menu bar.
- [ ] Close requests: Quit and a window's close ask the app first (`on_window_closed`), so it
      can confirm unsaved work; Quit ends the process now.
- [ ] Windows' frame clock: a thread waiting on `DwmFlush` sending frame ticks (GPUI's
      `vsync.rs`), when Windows has native views or a measurement shows the swap chain's
      present does not pace.

## Text

Basis: [text design](design/text.md).

- [ ] Markdown: images (with image loading), syntax colors in code blocks, a long code block's
      lines virtualized, inline code sized to its text (a span size relative to the text's), a
      task box pressable (`on_task`).
- [ ] An empty text has no height: a code block's empty line shows a space (copied as one).

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
- [ ] `octara new` (calls `skill install`).
- [ ] Causal tracing in the reactive layer (the causal chain reaches the cause id of kernel
      operations).
- [ ] Evaluation by ablation ([agent knowledge](design/agent-knowledge.md#order)): a small fixed
      task set, run with and without each piece of guidance; what does not change the result
      goes.

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

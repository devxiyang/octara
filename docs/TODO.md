# TODO

What is open, by area. For phase goals see [ROADMAP](ROADMAP.md); for what is built see
[ARCHITECTURE](ARCHITECTURE.md) and the [designs](design/). When an item is done, remove it and
make the corresponding design document describe it.

## Priorities

1. Core interaction and reusable components, exercised in Showcase and the Chat reference app:
   composed keyboard/IME behavior, focus, selection, overlays and real-window evidence.
2. Extend Agent feedback from real windows: target/build identity, sampled properties and native input.
3. The owned macOS backend (NSTextInputClient, live resize in the frame's transaction).
4. Native chrome: Windows/Linux menu integration, AppKit tooltips, toolbars and title bars.
   macOS menus and alert sheets, and rfd file dialogs are implemented in the winit shell.
5. Components: richer menus, dialogs and sheets, tabs, tables, radios, sliders and progress.
   Basic lists, modal popovers and searchable comboboxes are implemented.
6. Images: JPEG, WebP, SVG, loading and caching.
7. [Native Island](design/native-island.md): focus, accessibility joining, lazy creation.
8. Windows: WebView2 in DirectComposition; `surface` textures; Linux: WPE as a texture.
9. Text depth: direct platform text protocols, incremental layout of long paragraphs.
10. iOS, then Android.

## Runtime and Agent loop

Basis: [runtime design](design/runtime-contract.md), [Agent context](design/agent-context.md),
[agent knowledge](design/agent-knowledge.md), and [the Exact comparison](research/exact-runtime-simplicity.md).
Build the loop with existing owners and inspector facilities, preserving Rust + Signals.

- [ ] Add reliable attached-build identity and stronger target validation across tree replacement,
      using existing handles and local generations where required.
- [ ] Expose useful property origins and sampled opacity/color: `visual_of` currently gives the
      target value. Semantic values, source, bounds, diagnostics and pending work already come
      from the live owners. Business-value adapters stay optional; arbitrary Rust values need no
      serialization bound.
- [ ] Qualify native focus, keyboard/IME and gestures with real OS or simulator input. Current live
      replies identify semantic/framework routes; framework dispatch does not exercise OS input injection.
- [ ] Add a live transport for Windows. Any future live clock-control operation needs explicit
      host/clock semantics; `advance` remains headless-only.
- [ ] Give pending requests an owner and honor host lifetime when window-bound; cancel or withdraw
      where supported, reject late replies, and validate content/measurement generations at asynchronous boundaries.
- [ ] Fix cross-window dirty propagation and cleanup ordering where feedback exposes stale output;
      revisit affected work rather than requiring an app-wide fixed point for every operation.
- [ ] Keep each window's native placements, drawn output and interaction geometry consistent when
      redraws coalesce; report the available capture/feedback stage honestly.
- [ ] Define reset/restart for broken internal invariants separately from ordinary unfinished work;
      live realtime settling already reports pending work when its bounded wait expires.
- [ ] Complete the public API docs, especially undocumented variants, fields and methods.
- [ ] `octara new` (calls `skill install`).

Existing diagnostics and Kernel cause information remain. Add deeper dependency tracing, generic
snapshot/watch or guidance evaluation only when a concrete debugging or development task needs it.

## To research and design

- [ ] Platform shells and remaining native chrome: Windows/Linux menus, toolbars (`NSToolbar`),
      title bar and traffic lights, sidebars (system material underlay with drawn content, as
      Rune), native tooltips; on iOS / Android, native navigation (navigation bars, edge-swipe back, tab
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
- [ ] Verify a direct native text adapter, then one mobile host pilot with real simulator input:
      composition, keyboard geometry, cancelled navigation and replies after scope disposal.

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

## Known limitations

From section 9 of [Reactive core](design/reactive-core.md):

- [ ] Kernel children are a `Vec`; with a hundred thousand sibling nodes insertion and removal
      are O(n) (avoided by virtualization).
- [ ] After hot reload, virtual list rows are measured again.
- [ ] Hot reload preserves named text values and focus, but rebuilds the editor's selection,
      composition and undo history; the caret returns to the start. Live option updates keep
      the editor mounted and do not have this limitation ([Choices evidence](../examples/CHOICES.md)).
- [ ] On hot reload, a change of state type can only reset: add a migration function (serde or
      user-provided).
- [ ] Durability tiers (Salsa), for rarely changing state.
- [ ] Taffy 0.15.1: after a container removes its last absolutely positioned child, the hoisted
      list is not cleared, and rounding reads a deleted node. The kernel gives that container a
      fresh taffy node (`refresh_taffy_node`); report upstream and delete once fixed.

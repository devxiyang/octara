# TODO

Actionable to-do list (2026-11). For phase goals see [ROADMAP](ROADMAP.md); for the design basis see [UI primitives and component layering](design/ui-primitives.md)
and [Reactive core](design/reactive-core.md). Check off an item when it is done, and update the status in the corresponding design document.

## Priorities (2026-11-12)

The architecture round comes first; then features, in this order.

1. Architecture: [windows](design/windows.md) (the window's own state into `Window`, then
   several windows, windows in the protocol); the [platform layer](design/platform.md) (traits,
   the headless platform, the winit backend behind them); CI on macOS, Windows and Linux (done)
   and what it finds there; [ARCHITECTURE](ARCHITECTURE.md) kept current.
2. The owned macOS backend (NSTextInputClient, menus, live resize in the frame's transaction,
   panels), and agents driving a real window (the inspector protocol over a channel to it).
3. Native chrome: menus and shortcuts, file dialogs, alerts, several windows.
4. Components: menus and popovers, dialogs and sheets, tabs, lists and tables, tooltips,
   sliders, progress.
5. Images: JPEG, WebP, SVG, loading and caching.
6. [Native Island](design/native-island.md) step 4: focus, accessibility joining, lazy creation.
7. Windows: WebView2 in DirectComposition; `surface` textures; Linux: WPE as a texture.
8. Text depth: direct platform text protocols, incremental layout of long paragraphs.
9. iOS, then Android.
10. Evaluation, replay, causal tracing.

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
      keyboard (Tab back is step 4).
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

## Existing Primitives

Primitives that are implemented and kept:

| Category | Primitives |
| --- | --- |
| State | `state(key, init)`; `Signal`: `get` / `with` / `get_untracked` / `with_untracked` / `set` / `update` / `read_only`; `ReadSignal` |
| Derivation and side effects | `memo`, `watch(source, callback)`, `untrack` |
| Collections | `list(key, init, key_fn)`: `push` / `insert` / `update` / `remove` / `retain`; views `filter` / `sort_by` / `group_by` (`members`, `flatten`); `Derived`, `Source` |
| Text | `LiveText`, `live_text(key, init)`: `append` / `splice` / `set` |
| Structure | `component_keyed`, `component`, `show`, `show_else`, `each`, `virtual_each` + `Viewport`; `El::overlay` + `Placement` (`BELOW` / `ABOVE` / `RIGHT`, `Side`, `Align`) |
| Elements | `el` / `row` / `col` / `spacer`; `text(fixed value \| closure \| LiveText)`; `text_input` |
| Components (`octara-kit`) | `button` / `primary_button` / `ghost_button` / `toggle_button`, `checkbox`, `switch`, `text_field`, `select`; `Model` |
| Styling | `Styled`: GPUI method names (`items_center`, `p_4`, `gap_2`, `flex_1`, `rounded_md`, `text_sm`, `line_clamp`...), arbitrary values: a number is logical pixels, `relative(..)`, `rems(..)`; `bg` / `text_color` / `border_color` / `opacity` accept a `Token`, a `Color`, or a closure |
| Theme and context | `Token`, `Theme::light()` / `dark()`, `set_theme`; `provide` / `use_context` |
| Interaction states | `hover` / `active` / `focus_visible` / `style_when(State, ..)`; `selected`, `disabled`, `focusable` |
| Semantics | `role`, `label`, `description`, `expanded`, `checked`, `value`, `a11y_hidden` |
| Events | `on_press` / `on_input` / `on_toggle`; `Event::Hover` / `Focus` / `Blur`; disabled elements reject events |
| Runtime and testing | `App` (`force_state`, `set_theme`), `Event`, `testing::Host`, `Checked`, `check_consistent` |

## This Round: Adjusting Existing Primitives

Narrow the boundary first, then add new primitives.

- [x] Move `button()` and `checkbox()` out of the core; `octara-kit` now builds them from `el` + role + events; the kernel's semantic roles are kept
- [x] `input()` stays in the core, renamed `text_input` (it needs the platform input protocol); `TextField` is wrapped by the component library
- [x] Internal implementations become `pub(crate)`: `Runtime`, `Ctx`, `Idx`, `rt()`, `binding()`, `Stage` / `Observer` / `Emitter` / `Log` / `Cursor`,
      `Slots` / `Slot` / `Parent`; `Prop` and `TextContent` are kept because they appear in public signatures, marked `doc(hidden)`; `View::Dyn` becomes an opaque `Mount`
- [x] The old style methods (`gap` / `padding` / `grow` / `color`) are replaced by the new styling system

## This Round: New Primitives

- [x] **Styling**: typed Tailwind-style methods (layout, spacing, sizing, rounding, border, text); scales use tokens, arbitrary values use `px(..)`; values can be bound to closures
- [x] **Semantic tokens and theme**: `Token` + `Theme`, light and dark; tokens are resolved in the kernel, so switching the theme re-runs no closures and repaints only the nodes whose colors changed
- [x] **Context**: `provide` / `use_context` (scope-chain lookup); the theme is now resolved by the kernel, not through context
- [x] **Interaction states**: `hover` / `pressed` / `focus_visible` / `disabled` / `selected` as style overrides, toggled by the runtime; tests can force a state
- [x] **Semantics**: `role`, `label`, `description`, `expanded`, `a11y_hidden`
- [x] **Interaction**: `focusable`; `Event` gains Hover / Focus / Blur; hover, focus, and forced states are recorded by test id and restored after hot reload
- [x] **Structure**: `children` (`Children`: a single view, a tuple, a `Vec`; `.child(..)`); `overlay(open, Placement, content)`: attached to the kernel's overlay layer (the last child of the root), positioned relative to the anchor, flipped to the opposite side when it does not fit, then clamped back into the window; positioning methods such as `absolute` / `top` / `left`
- [x] **Component library `octara-kit`**: `button` / `primary_button` / `ghost_button` / `toggle_button`, `checkbox`, `switch`, `text_field`, `select` (including the overlay); controlled components take a `Model` (a `Signal` or a getter/setter); built-in accessibility semantics; disabling is handled by the core rejecting events
- [x] The example app is rewritten in the component-library style: 526 → 509 lines; the acceptance, consistency, and diagnostic tests all pass; adding a todo 4.0 → about 5 µs (see section 6 of [UI primitives](design/ui-primitives.md))

## This Round: Layout

Basis: [layout research](research/layout.md), [layout design](design/layout.md).

- [x] Adjustable viewport: `App::resize`, `App::size`, `viewport()` (Signal); the root node fills the window
- [x] Text measurement through Parley (with Inter bundled, no system fonts read, so results are machine-independent); `TextEngine` is replaceable, and `MonoEngine` is used to isolate cost
- [x] Text wraps by width by default; `nowrap`, `truncate` (including `min_w_0`), `max_lines`; Tailwind font sizes and line heights, `leading_*`; `Kernel::text_metrics`
- [x] Streaming text: cached per paragraph; the kernel tells the text engine that "the first N bytes are unchanged", so unchanged paragraphs are no longer compared. Body text with paragraphs costs about 42 µs per token (1 KB),
      99 µs (100 KB), depending only on the length of the last paragraph (Rune does the same: by block and by paragraph, with no incremental shaping)
- [ ] A single very long paragraph (no line breaks) is still re-laid out as a whole: 100 KB takes about 16 ms / token. Rune has not solved this either; it tried "per-word caching + its own line breaking", which was only 1.5x faster on a cold start, and gave it up.
      For the append case, consider shaping in chunks split on whitespace within the paragraph and resuming line breaking; another route is for the component library's Markdown to split into multiple texts by block (Rune's main technique)
- [x] Responsive: styles per window size class are resolved by the kernel (no closures run); `breakpoint()`; `App::at_sizes`
- [ ] Container queries (`.container()` + `.container_md(..)`); the element size source `size_of(id)`; a feedback-loop limit and `LAYOUT_LOOP`
- [x] Style API aligned with Tailwind: arbitrary values (a number is pixels, `pct`, `sp`), `px` / `py`, margins, `max_w_*`, `gap_x` / `gap_y`, `basis`, `flex_1`,
      `hidden`, `aspect_*`, font weight and `font_mono`; half steps and fractions use GPUI notation (`p_0p5`, `w_1_2`); `.map(f)` reuses styles
- [x] Theme as data (Tailwind v4 `@theme`): custom tokens, spacing unit, radius / font size / container steps, breakpoints, font families; resolved by the kernel, switching runs no closures;
      missing tokens are reported as `missing_tokens`; IBM Plex Mono is bundled, `App::register_font`
- [x] Aligned with Rune and GPUI (targeting desktop and mobile, not the web): `el()` / `row()` / `col()` no-argument constructors + `.child` / `.children(iter)`;
      the root node is a column; boxes do not shrink by default (same as Rune / Yoga), text can shrink; `text_*` sets only the font size, with a default line height of 1.25 (same as GPUI / Rune), and text sizes are rounded;
      method names follow GPUI (`flex_grow`, `flex_shrink_0`, `whitespace_nowrap`, `line_clamp`, `text_ellipsis`, `line_height`, `relative`, `rems`,
      `map` / `when` / `when_some`, `focus`); breakpoints become Material window size classes (`when_medium`, etc.); virtual lists anchor to measured rows
- [ ] System UI font for real windows (GPUI's `.SystemUIFont`) and system fallback (Rune's CJK fallback)
- [ ] Tailwind palette constants (`colors::BLUE_500`, etc.), for direct use in the prototyping stage
- [x] Layout diagnostics: `App::layout_issues()`, `OVERFLOW` (summarized by container and edge, as in Flutter), `COLLAPSED`, `TEXT_TRUNCATED` (Info),
      `OVERLAY_CLAMPED`, `MISSING_TOKEN`; each carries the test id, path, creation site, and fix
- [x] Scroll containers (Tailwind `overflow-*`: `overflow_y_scroll` / `_auto` / `_hidden` / `_clip`): can shrink below their content, clip, `Event::Scroll` /
      `ScrollTo`, `scroll_offset` / `client_size` / `scroll_size`; hot reload keeps the scroll position; diagnostics do not count the scroll axis as overflow
- [x] Virtual lists inside scroll containers: automatically follow the container's offset and height; after the estimate, measured row heights are filled back in; the first visible row is the anchor (the Rune / Exact approach);
      `VIRTUAL_LIST_NOT_SCROLLABLE` is reported when not inside a scroll container; post-layout callbacks and a `settle` loop (more than 4 rounds reports `LAYOUT_LOOP`)
- [x] The example apps' lists are placed in scroll containers; acceptance: every app has no layout issues at both 375×667 and 1280×800
- [x] Text style inheritance (same as GPUI): color, font size, line height, font weight, font family, wrapping, and `line_clamp` on a container apply to descendant text, and children can override; propagates downward only when the inherited value changes
- [x] Overlays: 8 px from the window edge; with `fit`, limited to the remaining space and scrolled (same as Rune's `anchored_fit`), on by default for `BELOW` / `RIGHT`
- [x] Grid (names and semantics as in GPUI): `grid()`, `grid_cols(n)` / `grid_rows(n)` (`repeat(n, minmax(0, 1fr))`), `col_span(n)`, `col_span_full()`, `col_start` / `col_end`, `row_*`
- [x] `size_of(id)`: element size as a reactive source (same as Rune's `layout_with`, used with a memo so it re-runs only when the condition flips); self-feedback is cut off by `settle` and reported as `LAYOUT_LOOP`
- [ ] Styles by container size (the same mechanism as window size classes)

## This Round: Rendering

Basis: [rendering and platform layer research](research/rendering.md), [rendering design](design/rendering.md).

- [x] Display list `octara::paint`: `Quad`, `GlyphRun`, `PushClip` / `PopClip`, in paint order, in logical pixels, recording the source node; `App::display_list`
- [x] CPU reference renderer `octara-render`: rounded-rectangle SDF coverage, borders, a rounded-corner clip stack, swash glyphs (1/4 px subpixel in x), color glyphs;
      deterministic PNG: `screenshot(&app, scale)`; the `screenshot` tool in `examples` renders all examples
- [x] wgpu renderer: one set of WGSL instanced SDF shaders, a single draw; R8 and RGBA glyph atlases (the same rasterization as the reference renderer); offscreen rendering and readback;
      differential testing against the reference renderer: at most 1 color level of difference in each scene (≤ 2 required)
- [x] Input by position and by key (core `App`, testable headless, usable by Agents too): `pointer_move` / `pointer_down` / `pointer_up` / `click_at`, `wheel`,
      `key(Tab / BackTab / Enter / Space / Backspace / Escape)`, `type_text`; `Kernel::hit_test`, `focus_order`; interaction state is recorded per node
- [x] Desktop window `octara-shell` (winit 0.30.13): GPU rendering to the window surface, scale factor, size changes, mouse, wheel, keyboard, text committed by the input method, redraw on demand,
      waiting when occluded; `cargo run -p octara-examples --bin window -- todo`. **To be verified on a local screen** (in this session's environment the window was judged not visible)
- [x] IME composition: preedit text is shown underlined and replaced on commit; a caret is drawn when focused; `App::caret` gives the candidate window position (`set_ime_cursor_area`)
- [x] System fonts: the `system-fonts` feature and `ParleyEngine::system()` (bundled fonts plus system fonts and fallback, CJK, etc.); `App::use_system_fonts()`:
      used by default by the window and the screenshot tool (`system-ui`); headless tests still use only the bundled fonts by default
- [x] Deterministic CJK: the `cjk` feature of the `octara-fonts` crate (a test dependency, not in applications): Noto Sans SC 400 / 600 subsets (GB2312 level 1, 3755 characters,
      CJK punctuation, kana, full-width forms, about 1.26 MB each, OFL), `cjk::install(&mut app)` registers them as the fallback per writing system; `App::register_fallback_font`;
      `tools/subset_cjk.py` can regenerate them; the screenshot tool's `--bundled` uses them
- [ ] More deterministic fallback fonts: Korean, Traditional Chinese, emoji (color), Arabic, etc., adding features as needed
- [x] Accessibility: `Kernel::accessibility_tree` / `App::accessibility_tree` (role, name, value, checked, expanded, selected, disabled, bounds, test id, Click / Focus actions;
      decorative nodes do not appear), `App::accessibility_action`; the window integrates `accesskit_winit` 0.34.2 (macOS / Windows / Linux adapters)
- [x] Ellipsis for truncated text: `truncate` / `text_ellipsis` and the last line of `line_clamp` end with "…" inside the box (laid out and cached in the same style, with the space before it removed)
- [x] Shadows: `shadow_2xs` ... `shadow_2xl` / `shadow_none` (GPUI names, Tailwind v4 values, changeable in theme data) and `shadow([BoxShadow])`;
      `Item::Shadow`, blur radius per CSS (= 2σ, same as Rune); both renderers use the same formula (Evan Wallace: closed-form erf in x, 4 samples in y, same as GPUI), difference ≤ 1;
      the component library's dropdown popup carries `shadow_lg` and becomes a `focus_group` + `modal` (arrow keys, Escape / click outside to close, focus returns to the dropdown)
- [ ] Inset shadows
- [x] Gradients (GPUI's API): `.bg(linear_gradient(angle, linear_color_stop(color, position), ..))`, angle as in CSS (0 up, 90 right),
      colors can be tokens (resolved with the theme), and usable in state overrides and closures; CSS gradient line length, premultiplied interpolation; `Quad::gradient`, both renderers use the same formula, difference ≤ 1
- [ ] More than two color stops, radial gradients
- [x] Images (Rune's model, GPUI's names, confirmed): a shared handle `Image` (`rgba` / `decode` PNG / `set` to swap pixels, taking effect next frame without re-running views);
      `.img(&image)` on any element, `.object_fit(Fill / Contain / Cover / ScaleDown / None)` (stretch by default, same as Rune); when only width or height is set the other side follows the aspect ratio,
      and when neither is set the image size is used; clipped to the element's rounded corners; both renderers use the same sampling formula (bilinear, premultiplied, edge anti-aliasing), the GPU uses an image atlas, difference ≤ 1;
      the output type of `octara-render` is renamed `Bitmap`
- [ ] Images: JPEG / WebP / SVG, async loading and caching (together with "resources"), mipmaps when scaled down, large images exceeding the atlas, re-layout after the shape changes
- [x] Group opacity (same as Rune): when opacity < 1 and the subtree draws more than one thing, `PushLayer { opacity }` ... `PopLayer`, drawn opaque inside the layer;
      when it draws only one thing, it is faded directly. The reference renderer uses a separate buffer (quantized to 8 bits), the GPU draws into an offscreen texture in per-layer passes and then composites per pixel, difference ≤ 1
- [ ] Display list updated incrementally by `Dirty::PAINT`; a layer covers only its extent (currently the whole window size)

## Input

Basis: [input research](research/input.md), [input design](design/input.md). Confirmed: single target + capture; commands + keymap data; the runtime owns text editing state.

- [x] Pointer: `PointerEvent { id, kind: Mouse / Touch / Pen, phase: Down / Move / Up / Cancel, button, modifiers }`; captured on press;
      moving out releases the press, moving back restores it; scrolling while held or `Cancel` cancels; `on_double_press`, `on_secondary_press` (right click, macOS Ctrl+click, touch long press,
      accessibility ShowContextMenu), `on_long_press`, `on_drag(Drag)`; touch pan scrolling and inertia (iOS decay); hover updates as content moves, touch does not hover
- [x] Virtual clock: `advance` / `advance_to` / `now` / `next_deadline`; tunable parameters `Tuning`; `Platform`
- [x] Focus ring follows the input method: `FOCUSED` and `FOCUS_VISIBLE` are separate, `.focus(..)` / `.focus_visible(..)` as in GPUI
- [x] Per-element driving: `tap` / `tap_with` / `double_tap` / `secondary_tap` / `long_press` / `drag` / `swipe`; by test id and then by accessibility name,
      with errors on duplicate names, occluded targets, and targets outside the window
- [x] shell: three mouse buttons, touch, cancel on focus loss, clock and timed wake-ups
- [x] Focus: `tab_index` (positive first, negative skipped), `focus_group` (arrow keys, Home / End, menus and lists only up and down, lists stop at the ends, remember the last item,
      Tab stops only once), `autofocus`, `modal(click_outside, dismiss)` (background inert, Tab confined, Escape / click outside to close, focus returns to where it was before opening;
      marked modal for accessibility); the driver rejects targets behind a modal
- [x] Keyboard: `KeyEvent` (logical key, physical key, modifiers, repeat); chords `"mod+shift+k"` (`Chord`, `mod` per platform); keymap data `Keymap`
      (deeper context wins, same depth: later added wins, `None` unbinds); `on_key`, `.on_command`, `on_command` (application level), `key_context`;
      built-in commands (`focus.*`, `press`, `menu`, `edit.delete_backward`); switching keyboard layouts matches by physical key;
      during IME composition Enter / Escape belong to the input method, and shortcuts commit the composition first; `invoke`, `commands`, `press_key`
- [x] Mechanism fix (confirmed): text field submission becomes `on_submit` (Enter → `submit` command, falling through to `press` when unhandled), and `on_press` is synonymous for all elements;
      the pointer behavior of built-in elements goes through the `Behavior` interface (no element-type checks remain in the recognizer); multi-clicks are counted on press
- [x] Text editing (owned by the runtime; the app is still `value` + `on_input`): selection; movement, selection, and deletion by grapheme and Unicode word; select all, copy, cut, paste
      (`Clipboard`, the shell connects arboard 3.7.0); undo / redo (consecutive typing or deletion merges into one step, 128 steps, cleared when the app rewrites the value); IME composition is shown at the caret
      and replaces the selection; click, Shift+click, double-click to select a word, triple-click to select all, drag to select; long lines scroll horizontally to keep the caret visible; platform protocol `text_field` / `replace_text` /
      `set_selection` / `set_composition` with UTF-16 conversion; standard per-platform keybindings (Apple uses Alt by word, Cmd+arrow to the ends); accessibility `SetValue`
- [x] Multi-line text fields; password fields ([text input](design/text-input.md))
- [ ] Deferring the app's rewrites of the value during composition (Exact); touch long-press word selection and selection handles; the caret position within an IME composition
- [ ] Accessibility `ScrollIntoView`, text selection (AccessKit text runs); scroll into view before driving; multi-finger gestures, edge back swipe; direct platform text protocols (NSTextInputClient, etc.)

## Later

- [x] Async ([Async](design/async.md), confirmed: a custom executor, typed named channels, scope-owned subscriptions): `spawn` / `Task` (cancelled when the scope is destroyed, `detach`),
      `sleep` / `sleep_until` / `interval` / `now` (application clock), `background` (4-thread pool), `channel` (`Sender` across threads, `Receiver::on` / `recv`,
      `named` for tests and the proxy `App::send`), `resource` (refetch by key, stale answers do not land, keeps the last value, `refetch`); woken tasks run in spawn order;
      `App::poll` / `settle` / `run_until_idle` / `pending`; the clock moves into the scheduler, with time logic in `app/time.rs`; `on_external` / `Event::External` are removed,
      replaced by `Event::Send`; the shell wakes through an event-loop proxy
- [ ] After async: resource deduplication cache, deferred placeholders, record and replay at the loopback, forbidding real time in tests, stream backpressure and coalescing
- [x] Collection operators `map`, `join` ([design](design/map-join.md)); flattening keeps its own entry per item (a known limit there)
- [x] `icon`, `octara-icons` (Lucide), vector paths and `canvas` ([vector graphics](design/vector-graphics.md))
- [x] Animation step 1: the `motion` module — `Spring::new(duration, bounce)` (the SwiftUI / Rune mapping) and named presets, analytic tracks (same value at any frame rate,
      retargeting keeps position and velocity, the rest time is computed when a segment is built), `PropClass` (rest threshold and reduced motion)
- [x] Animation step 2 ([Animation](design/animation.md), confirmed: declared on the element, inherited by the subtree): `.motion(..)` / `motion_none` / `color_motion` /
      `opacity_motion` / `layout_motion`; colors (premultiplied OKLab), opacity, and shadows move along with styles, state overrides, and theme changes; nodes that have never been laid out do not animate;
      `instant(..)`, `settle_motion()`, typing and dragging are automatically instant; `App::set_reduced_motion`; while in motion `next_deadline` gives the next frame,
      `pending` lists it (`WorkKind::Motion`), `run_until_idle` runs it to completion; the component library's colors use `Spring::SNAP`
- [x] Animation step 3: layout — for nodes with `layout_motion`, position and size relative to the parent move by spring (Rune's per-edge approach, no scaling); the first layout does not animate;
      children move with the parent; scrolling does not trigger it; painting, hit testing, accessibility, and per-element driving use the presented position (`shown_rect`), while layout diagnostics and overlays still use the target
- [x] Animation step 4: appearing and disappearing (confirmed: fade in and out by default inside a motion subtree) — when the removal branch of `show` and `each` has a root with opacity motion that has been shown, it enters an exiting state:
      it leaves the tree immediately (layout, hit testing, focus, accessibility, test id), fades out in place, and is destroyed only after the fade (destroy is deferred meanwhile); new branches fade in under an already shown parent;
      first mount, hot reload, and virtual list window moves do not play it; `settle_motion` clears it immediately; a departing node loses focus and hover
- [ ] Appearing and disappearing: same-key revival (Rune), `.transition(..)` (scale, slide in), exiting children follow the parent
- [x] Animation step 5: scrolling — a programmatic `scroll_to` and `Event::ScrollTo` slide along a spring in containers with layout motion, retargeting midway keeps velocity;
      wheel and finger scrolling take over immediately; without motion it jumps straight to the target
- [ ] The shell reads the system's reduced-motion setting (winit does not provide it; per-platform interfaces are needed)
- [x] Hover (confirmed): the element under the pointer and its ancestors are all hovered (GPUI, Rune, CSS), up to the innermost `hover_alone`; `group(name)` +
      `group_hover(name, ..)` (GPUI); while a layout animation is in progress and the pointer is still, hover is held, and catches up when the animation ends (Rune); an element that leaves the tree loses hover immediately
- [ ] Give Agents the full usage of the framework ([design](design/agent-knowledge.md), confirmed: registry generated from rustdoc JSON, CLI first):
  - [x] `api.json`: the `octara-cli` crate (command `octara`), `octara api` uses nightly rustdoc JSON (format 61, `rustdoc-types` 0.61)
        to generate all public items of the five public crates — canonical paths and aliases (prelude), signatures, bounds of generic impls (`within`), docs, source files,
        implemented traits and implementors, doc links, targets of external re-exports; a custom format (`format: 1`), checked into the repo; a test regenerates and compares,
        and `octara api --check` checks the same
  - [ ] About thirty percent of public items have no docs (707 / 2221, mostly variants, fields, and methods): add docs
  - [x] Diagnostic code registry: `octara::diagnostics` (`Code`: severity, category, one-line summary, cause, fix, doc anchor; `CODES`, `code(id)`);
        `Diagnostic` and `Severity` move into that module, `Diagnostic.code` / `LayoutIssue.code` are `&'static Code` (severity follows the code);
        reports accept only registered codes; tests: anchors exist, codes appearing in source / tests / docs are all registered, and every registered code has a report site
  - [x] Usage catalog (`octara_cli::catalog`): one entry per `api.json` item, merged with kernel data — diagnostic codes (cause, fix, anchor, APIs mentioned in the fix),
        built-in commands and their default keys (macOS and Windows / Linux), light and dark colors of theme tokens, durations and bounce of named springs (new `Spring::NAMED`);
        `octara vocab <name>` (full path, aliases, `::` suffix, codes, commands; a type lists its members, compressed to `p_{0,1,2}`; with no argument it lists the prelude),
        `octara explain <code or text containing a code>` (with no argument it lists all codes), in both text and `--json` output
  - [x] `docs/start-here.md` (about 280 lines, code blocks compiled and run as `octara-cli` doctests); also fixes the `rounded` docs wrongly attached to `Styled::motion`
  - [x] The `octara` skill (confirmed: skill + always-on guidance): `octara skill install` writes `SKILL.md`, `references/index.md` (`octara index`:
        prelude by module, members of prelude types, other crates, built-in commands, diagnostic codes, `p_{N}` compression, repeated-stride sets defined once, capped at 12 KB),
        and `references/start-here.md` to the project's `.agents/skills/octara/`; when Claude Code or Kiro is detected, a relative symlink is created (copied if that fails);
        `--global` (`~/.agents/skills`, with links for the Claude Code and Amp user directories), `--agent`, `--check`; writes the always-on guidance
        between markers in the project's `AGENTS.md`, adding `@AGENTS.md` when a `CLAUDE.md` exists; does not overwrite someone else's directory of the same name; `octara skill export`; a copy in the repo's `skills/octara/`
        serves `npx skills add`, and a test checks it is not stale. This repo's `AGENTS.md` keeps only Safety and this repo's working rules
  - [ ] Tests and Agents read presented values (painted opacity, color): currently `visual_of` is the target value, and the presented value exists only in the display list
  - [x] Inspector protocol in the core (`octara::inspect`): `App::inspect()` gives the semantic tree (`octara.inspect/1`: test id,
        role, name, value, checked, expanded, states, presented bounds, actions, commands, source); `Request` / `Reply` as JSON lines
        (tap, double_tap, long_press, secondary_tap, focus, hover, key, type, invoke, advance, settle, size, theme, reduced_motion,
        inspect, issues, expect), failures carrying registered codes (`BAD_REQUEST`, `NO_SUCH_ELEMENT`, `STEP_FAILED`,
        `EXPECTATION_FAILED`); `run_script` stops at the first failure; `check(view, options)` mounts the app fresh at light/dark ×
        phone/desktop, settles it (`NOT_SETTLED`) and reports layout issues, diagnostics and accessibility (`UNNAMED_CONTROL`,
        `SMALL_TARGET` on touch sizes, `LOW_CONTRAST` per WCAG, skipped over gradients and images), merged across cells, with
        accepted findings (`why` required) set aside (`octara.check/1`)
  - [x] `octara check` / `inspect` / `run [--script]` in the CLI: the app names `[package.metadata.octara] view` (plus optional
        `sizes`, `themes`, `accept`); a harness generated in `<target>/octara/harness/` depends on the app by path and on `octara`
        exactly as the app does, starts from its `Cargo.lock` and profiles, builds into its target directory and calls
        `octara::inspect::main`; text by default, `--json`; sources relative to the workspace; exit 0 clean / 1 findings /
        2 build or run failure. The examples crate names the counter
  - [x] What `check` found in the examples, fixed: `resource` kept its state in `Signal::new` (reported `UNKEYED_STATE` in
        every component using it; now framework-internal signals); the showcase's cards did not shrink at phone width (boxes
        do not shrink by default; the cards now `flex_shrink()`); todo and contacts checkboxes got labels; the dark theme's
        accent fill is blue-600 / hover blue-700 (white on blue-500 was 3.68:1) and the light theme's muted text a shade
        darker (4.40:1 on Surface); a test keeps every text token at 4.5:1 on its surfaces in both themes
  - [x] Target sizes, as Exact does it (confirmed): no runtime enlargement; controls are sized for the platform and `check`
        flags the ones that are not. `When::Touch` / `when_touch(..)` styles apply on iOS and Android (`Platform::is_touch`;
        `set_platform` re-resolves, no closure reruns); the kit's buttons, fields and selects take 44 px on touch, and the
        checkbox and switch are a press area around their visual (hugging it with a pointer, 44 × 44 on touch). `check` cells
        name a platform (phone: `ios`, desktop: `macos`); `SMALL_TARGET` asks 44 × 44 on touch and 24 × 24 with a pointer
        unless no other control's center is within 24 (WCAG 2.5.8)
  - [ ] Example ids (how an item is used in the examples), recipes and byte limits, `octara new` (calls `skill install`), MCP, llms.txt, evaluation
- [ ] Causal tracing in the reactive layer (currently only down to the cause id of kernel operations)
- [x] Inspector protocol, window and GPU rendering, Parley text (per-paragraph caching), captures ([captures](design/capture.md))
- [ ] Incremental layout inside one long paragraph
- [x] Native Island steps 1 to 3 (macOS overlay web views, underlay maps); Canvas
- [ ] Agent evaluation retest: update the API docs, use harder tasks or a weaker model

## Known Limitations

From section 9 of [Reactive core](design/reactive-core.md):

- [ ] Kernel children are a `Vec`; with a hundred thousand sibling nodes insertion and removal are O(n) (currently avoided by virtualization)
- [x] Virtualized row heights are estimates: filled back with real layout, and scroll anchoring for variable-height rows is supported (after hot reload, measured heights need to be re-measured)
- [ ] On hot reload, a change of state type can only reset: add a migration function (serde or user-provided)
- [ ] Durability tiers (Salsa), for rarely changing state
- [ ] Taffy 0.15.1: after a container removes its last absolutely positioned child, the hoisted list is not cleared, and rounding reads a deleted node. The kernel temporarily gives that container a fresh taffy node (`refresh_taffy_node`); report upstream and delete once fixed

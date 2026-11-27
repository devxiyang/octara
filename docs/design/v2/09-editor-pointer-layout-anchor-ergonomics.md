# 09 — Core Defaults, Semantic Design System, Markdown Architecture & Builder Slimming (`0.0.4`)

> **Release target**: `0.0.4` (clean-sheet, zero backwards compatibility, zero historical baggage).  
> **Execution plan**: 3 strictly ordered, independently verifiable Waves (`Wave 1 → Wave 2 → Wave 3`).  
> **Future roadmap**: `0.0.5` (GPU element `Transform` + Enter Motion + Kit micro-interactions, Parley `InlineBox`, `ImageLoader` LRU decode cache + HiDPI hairline snapping + Emoji/CJK matrix, default editor/selection right-click menu); `0.0.6` (`FormState` lifecycle + `async_view`/`empty_state`/`error_state`, scoped `ErrorBoundary`, DevTools console diagnostics + in-app Inspector overlay + `octara dev` geometry restore).

---

## Wave 1 — Semantic Design System, Tree-Derived States, Builder Slimming & System Lifecycle

### 1.1 Tailwind v4-Inspired Semantic Token System (`octara-model`, `octara-tree`, `octara`, `octara-macros`)
1. **Layered Semantic `ColorToken` (`crates/octara-model/src/theme.rs`)**:
   - Upgrade built-in semantic slots (with WCAG-checked light & dark defaults):
     - **Surfaces**: `Background`, `Surface`, `SurfaceElevated`, `SurfaceHover`, `SurfacePressed`, `SurfaceSelected`
     - **Text**: `Text`, `TextMuted`, `TextSubtle`
     - **Borders**: `Border`, `BorderSubtle`, `BorderStrong`
     - **Interactive & Status**: `Accent`, `AccentHover`, `AccentText`, `Focus`, `Selection`, `Info`, `Success`, `Warning`, `Danger`
     - **Custom**: `Custom(&'static str)`
   - Add **`ColorScheme::{Light, Dark}`** on `Theme` (`theme.scheme`).
   - Add the **complete Tailwind CSS v4 `palette`** in `octara_model::theme::palette`: all **22 color families × 11 shades (50, 100, 200, 300, 400, 500, 600, 700, 800, 900, 950 = 242 exact Tailwind v4 sRGB colors)**: `RED`, `ORANGE`, `AMBER`, `YELLOW`, `LIME`, `GREEN`, `EMERALD`, `TEAL`, `CYAN`, `SKY`, `BLUE`, `INDIGO`, `VIOLET`, `PURPLE`, `FUCHSIA`, `PINK`, `ROSE`, `SLATE`, `GRAY`, `ZINC`, `NEUTRAL`, `STONE` (plus `BLACK`, `WHITE`, `TRANSPARENT`), exposed via `ColorScale` (`s50`..`s950`, `Shade::{S50..S950}`, `.shade()`, `.step()`, `.all()`, and `palette::by_name`).
   - Complete Tailwind v4 scale parity across `octara-model` and `tw!`:
     - **Font weights**: full 9-step scale (`thin=100`, `extralight=200`, `light=300`, `normal=400`, `medium=500`, `semibold=600`, `bold=700`, `extrabold=800`, `black=900`).
     - **Font size + paired line-height**: full 13-step `xs`..`9xl` + named line-heights (`none=1.0`, `tight=1.25`, `snug=1.375`, `normal=1.5`, `relaxed=1.625`, `loose=2.0`).
     - **Radius**: full `none`, `xs`, `sm`, `md`, `lg`, `xl`, `2xl`, `3xl`, `4xl`, `full`.
     - **Shadows**: full `none`, `2xs`, `xs`, `sm`, `md`, `lg`, `xl`, `2xl`.
     - **Container widths**: full 13-step `3xs`..`7xl`.
     - **Dynamic spacing**: `0`, `px` (1px), arbitrary spacing steps (`0.5`, `1`, `1.5`, `2`, ..., `96`), and arbitrary pixel values in `tw!` (e.g. `p-[13px]`).
2. **Lazy Theme-Aware Color Modifiers (`ColorValue`)**:
   - Extend `ColorValue` (still `Copy`, zero heap allocation) to support:
     - `ColorValue::Token(ColorToken)`
     - `ColorValue::Color(Color)`
     - `ColorValue::Alpha { base: ColorToken, alpha: f32 }` (via `ColorToken::alpha(self, alpha: f32) -> ColorValue` and `ColorValue::with_alpha(self, alpha: f32) -> ColorValue`, matching Tailwind `bg-danger/10`, `border-border/60`)
     - `ColorValue::Mix { a: ColorToken, b: ColorToken, weight: f32 }` (via `ColorToken::mix(self, other: ColorToken, weight: f32) -> ColorValue`, matching CSS `color-mix()`)
   - Resolve dynamically against the active (possibly subtree-scoped) `Theme` in `ColorValue::resolve`.
3. **Paired Typography Scale (`TextScale`)**:
   - Replace bare `text: [f32; 13]` + global `1.25` multiplier with paired `TextScale { pub size: f32, pub line_height: f32 }` per `FontSizeToken` following Tailwind v4 proportions (`Xs: 12/16`, `Sm: 14/20`, `Base: 16/24`, `Lg: 18/28`, `Xl: 20/28`, `Xl2: 24/32`, `Xl3: 30/36`, `Xl4: 36/40`, `Xl5: 48/48`, `Xl6: 60/60`, `Xl7: 72/72`, `Xl8: 96/96`, `Xl9: 128/128`).
   - Setting `.text_size(FontSizeToken::Sm)` automatically adopts the token's paired line height unless an explicit `.line_height(...)` overrides it; numeric `.text_size(14.0)` continues using `theme.line_height` multiplier unless explicit `.line_height(...)` is set.
4. **Subtree Theme Scoping (`ThemePatch` + `When::Dark` / `When::Light`)**:
   - Add `When::Dark` and `When::Light` to `octara_model::style::When`, evaluated against the effective subtree `Theme::scheme` in `StyleResolver`.
   - Add `ThemePatch` (lightweight sparse override: optional `scheme`, optional `spacing`, optional `colors: SmallVec<[(ColorToken, ColorValue); 4]>`, optional `radius` / `base_theme: Option<Box<Theme>>`) stored in a sparse `SecondaryMap<NodeId, ThemePatch>` in `octara-tree`, exposed via `El::theme_patch(ThemePatch)` / `El::theme_scope(Theme)`.
   - `StyleResolver` resolves each subtree using the effective inherited `Theme` (zero allocation when no ancestor sets a `ThemePatch`!). Changing a node's `ThemePatch` invalidates its subtree styles cleanly.

### 1.2 Unified Tree-Derived State Reconciliation (`octara-model`, `octara-tree`)
- Reorder `octara_model::style::State` cleanly to match CSS override precedence:
  `HOVER < FOCUS_WITHIN < FOCUSED < FOCUS_VISIBLE < PRESSED < SELECTED < CHECKED < EXPANDED < DISABLED < DRAG_OVER < DRAGGING < DROP_HOVER`.
- Define `State::TREE_DERIVED = State::FOCUSED | State::FOCUS_VISIBLE | State::FOCUS_WITHIN` and strip `State::TREE_DERIVED` uniformly in `RuntimeOp::SetState`.
- Centralize ancestor-chain focus synchronization in `Tree::reconcile_focus_states(...)`, called from both `Tree::change_focus` and structural `Tree::apply` commits (reparent / detach / retire), ensuring `FOCUS_WITHIN` is always strictly equal to the inclusive ancestor chain of `self.focus`.

### 1.3 Builder Slimming, Semantic Shorthands, `tw!` Macro & Trait Consistency (`octara`, `octara-macros`, `octara-runtime`, `octara-icons`)
1. **`El<C>` Structural Slimming (fixing macOS debug `__TEXT,__eh_frame > 16MB`)**:
   - Keep `El<C = ElementContent>` (preserving compile-time `TextInput` type safety) while boxing **`props: Box<ElementProps>`** so value-passed `El<C>` is register-sized and unwind cleanup collapses to ONE non-generic `drop_in_place::<ElementProps>` compiled once in `octara`.
   - Sink all mutation logic on `ElementProps` and `StylePatch` into concrete, non-generic core methods (`push_style`, `push_styles(&[StyleProp])`, `push_child`, `push_children`, `push_override`, semantic/handler setters). Multi-property helpers (`p`, `px`, `py`, `m`, `mx`, `my`, `size`, `inset`) each emit a single `push_styles(&[...])` call.
   - Measure and record macOS debug `__TEXT,__eh_frame` before and after on `octara` and `octara-kit`.
2. **Tailwind-Inspired Zero-Generic Authoring Shorthands (`Styled` & `El<C>`)**:
   - State combinators on `El<C>`: `.hover(...)`, `.pressed(...)`, `.focus_visible(...)`, `.focus_within(...)`, `.disabled_style(...)`, `.group_hover(name, ...)`, `.dark(...)`, `.light(...)`.
   - Zero-generic layout shorthands on `Styled`: `.items_center()`, `.items_start()`, `.items_end()`, `.items_stretch()`, `.justify_center()`, `.justify_between()`, `.justify_start()`, `.justify_end()`, `.flex_1()` (`grow(1.0) + shrink(1.0) + basis(0.0) + min_w(0) + min_h(0)` — eliminating the CSS `min-size: auto` overflow trap!), `.w_full()`, `.h_full()`, `.scroll_y()` (`overflow_y(Scroll) + min_h(0)`), `.scroll_x()` (`overflow_x(Scroll) + min_w(0)`), `.truncate()`.
3. **Compile-Time `tw!(...)` Macro & High-Frequency Tailwind Primitives (`octara-macros`, `octara-model`, `octara-tree`, `octara`, `octara-pipeline`, `octara-host-web`)**:
   - **Per-side borders**: upgrade `LayoutStyle.border` to `Edges<LayoutLength>` (`StyleProp::{BorderTop, BorderRight, BorderBottom, BorderLeft}`, `.border_t/r/b/l/x/y()`, mapped directly to `Quad.border: [f32; 4]`, Taffy `border`, and Web CSS).
   - **Focus ring**: add first-class `Ring` (`StyleProp::{RingWidth, RingColor, RingOffset}`, `.ring()`, `.ring_color()`, `.ring_offset()`), rendered as crisp non-blurred spread outline with optional offset on both Native and Web.
   - **Pointer-events pass-through**: add `NodeFlags::POINTER_EVENTS_NONE` + `.pointer_events_none()` / `.pointer_events(bool)`, skipped in hit-testing without disabling focus/a11y or blocking non-none descendants (matching CSS `pointer-events: none` / `auto`).
   - Provide `tw!("flex items-center justify-between gap-2 px-3 py-2 bg-surface/80 text-sm text-muted rounded-md border-b border-border/60 hover:bg-surface-hover focus-within:ring-2 focus-within:ring-focus/30 dark:bg-surface-elevated")` returning a lightweight `TwStyle` (static base `&'static [StyleProp]` + static conditional overrides + optional flags/cursor) applicable via `el.tw(tw!(...))`.
   - Full compile-time coverage of Tailwind v4 layout (`hidden`, `flex*`, `grid*`, `col-span-*`, `row-span-*`, `static/relative/absolute/fixed/sticky`, `inset/top/right/bottom/left-*`, `w/h/size/min-*/max-*` including fractions `1/2`, `1/3`, `2/3`, `1/4`, `3/4` & `full`/`auto`, `mx-auto`, `aspect-square`/`aspect-video`, `self-*`, `items-*`, `justify-*`, `overflow-*`), borders/rings/shadows/opacity/transitions (`border*`, `ring*`, `shadow*`, `opacity-*`, `transition-colors/opacity/all`), typography/interaction (`line-clamp-1..6`, `truncate`, `whitespace-nowrap`, `text-left/center/right/justify`, `font-sans/mono`, `cursor-*`, `select-none/text`, `pointer-events-none/auto`), semantic + 22×11 palette colors with `/alpha`, and all modifiers (`hover:`, `active:`, `focus:`, `focus-visible:`, `focus-within:`, `disabled:`, `selected:`, `checked:`, `expanded:`, `drag-over:`, `dark:`, `light:`, `touch:`, `group-hover:`, `sm:/md:/lg:/xl:`).
   - Parsed strictly at compile time with clear compile errors for unknown tokens, zero runtime parsing, and zero generic monomorphization.
4. **Closure & Trait Consistency**:
   - Relax `WatchEffect`, `ReactiveWorld::watch`, and `BuildCx::watch` (`source` and `effect`) from `Fn` to `FnMut` (and simplify `async_memo.rs` to drop its `Cell`/`Rc` workaround where possible).
   - Implement `PartialEq + Eq + Hash` on `octara_icons::LucideIcon` keyed on canonical `name`.
   - Audit `octara-model` / `octara` public value types (`StylePatch`, `VirtualOptions`, `Placement`, etc.) for `Clone + Debug + PartialEq` consistency.

### 1.4 System Preferences & Window Close Lifecycle (`octara-model`, `octara-runtime`, `octara-pipeline`, `octara-host-native`, `octara-host-web`)
1. **`SystemPreferences { color_scheme: ColorScheme, reduced_motion: bool }`**:
   - Add `HostReply::SystemPreferencesChanged(SystemPreferences)` and reactive `BuildCx::system_preferences() -> ReadSignal<SystemPreferences>`.
   - Sync OS appearance and Reduce Motion live on macOS (`octara-host-native/src/os/macos`), desktop (`winit` theme + platform reduced motion), and Web (`matchMedia` listeners in `octara-host-web`).
   - Wire `reduced_motion` live into `NativeEngine` (`set_reduced_motion(bool)`).
2. **Window Close Intercept (`on_close_request`)**:
   - Add `Window::on_close_request(impl Fn(&mut WriteCx<'_>) -> bool + 'static)` (returns `true` to allow closing, `false` to prevent closing so the app can prompt for unsaved changes or hide the window, then later close via `WriteCx::close_window`).
   - Wire `WindowEvent::CloseRequested` in `octara-host-native/src/shell.rs` through this hook (default behavior when unset: allow close).

---

## Wave 2 — Unified Text/Selection Engine, Scroll Policy, Overlay Scrollbars & Native Cursors

### 2.1 Unified Pointer Text Selection & Cross-Platform Keyboard Defaults (`octara-tree`, `octara-runtime`, `octara-pipeline`, `octara-host-native`, `octara-host-web`, `octara-inspect`)
1. **Unified `TextPointerIntent` State Machine**:
   - Handle **both** `NodeKind::TextInput` (editable) and `NodeFlags::SELECT_REGION` (static selectable text / Markdown) through one unified pointer state machine in `InteractionArbiter` + `AppRuntime`:
     - Single click: place caret (`TextInput`) or collapse/clear doc selection (`SELECT_REGION`); `Shift+click` extends from current anchor.
     - Drag: forward & backward selection from anchor (for `SELECT_REGION`, supports cross-node selection across descendant text nodes in tree order!).
     - Double click: Unicode word selection (`unicode-segmentation`); `secure` inputs select all `(0, len)`; double-click + drag extends word-by-word.
     - Triple click: line/paragraph selection (`0..len` for single-line input, `\n`-bounded paragraph for multiline textarea, full text node for `SELECT_REGION`); triple-click + drag extends line-by-line.
     - Scrolled single-line horizontal & multiline vertical coordinates, empty/placeholder input (`-> 0`), multi-byte UTF-8 (CJK/Emoji).
     - **IME composition**: clicking inside a composing editor settles composition (`TextEdit::Unmark`, fallback `CancelComposition`) and re-queues the pointer-down intent at the front of `routed` so geometry settles in the same turn before placing the caret.
     - **Web host parity**: native DOM `<input>`/`<textarea>` and browser `Selection` stay authoritative without synthetic conflicts.
2. **Cross-Platform Default Keyboard Editing & Selection Shortcuts**:
   - In `AppRuntime`, handle `Cmd/Ctrl+C` (copy active `SELECT_REGION` selection using `window.copy_selection`, including Markdown source copy!) and `Cmd/Ctrl+A` (select all within active `SELECT_REGION`) when no editor/app handler consumes the key.
   - Provide engine-level default editor keyboard commands (`Left`/`Right`/`Up`/`Down`, `Shift+Arrows`, word jump `Alt/Ctrl+Left/Right`, `Home`/`End`, `Backspace`/`Delete` word, `Cmd/Ctrl+A/C/X/V`) as fallback when the OS native input view (e.g. non-macOS desktop host or headless `octara-inspect`) does not handle the key natively.
3. **Inactive Window (`WindowActive(false)`) Visual Dimming**:
   - Propagate `window_active` state into `NativeEngine` / `PaintCompiler`: when inactive, hide/pause editor caret blinking and render text/editor selection highlights in neutral muted tint (`ColorToken::TextSubtle.alpha(0.28)` instead of `ColorToken::Selection`).

### 2.2 Text Box Model Alignment & Tight Glyph-Box Inline Pills (`octara-model`, `octara-pipeline`, `octara-host-web`)
1. **CSS-Consistent Leaf Text Alignment**:
   - In `TextEngine::project`, when `content.h > presentation.size.height` on `NodeContent::Text`, align `origin.y` according to the node's flex alignment (`AlignItems` / `JustifyContent` given `FlexDirection`), matching Web CSS anonymous flex items. This fixes `button()`, `tabs()`, `segmented()`, `badge()`, and all user-authored `text("...").min_h(...).align(Center)`.
2. **Line Box vs. Tight Glyph Band + `RichSpan::padding_y`**:
   - Keep `TextLine.rect` / `TextFragment.rect` at `metrics.line_height` for hit-testing, carets, and selection highlights.
   - Record per-line tight glyph band (`glyph_top`, `glyph_height` from Parley `LineMetrics` `ascent + descent`, clamped to `[line.y, line.bottom]`).
   - Add `pub padding_y: Option<f32>` to `RichSpan` and `.padding_y(f32)` to `octara::Span`.
   - Render `RichSpan` background/border quads with `[glyph_top - padding_y, glyph_bottom + padding_y]` clamped strictly within `[line.y, line.bottom]` so wrapped lines never collide.
   - Sync `padding-block` + `box-decoration-break: clone` in `octara-host-web`.

### 2.3 Unified Scroll Policy, Zero-Jump Anchoring, Overlay Scrollbars & Focus-Into-View (`octara-tree`, `octara-runtime`, `octara-pipeline`, `octara`, `octara-host-web`)
1. **Unified Deterministic Scroll Resolution (`LayoutEngine`)**:
   - Single explicit precedence order:
     `ProgrammaticRequest` > `FollowTail (active)` > `EditorCaretReveal` > `FocusedNodeReveal` > `ContentAnchor` > `Clamp`.
   - **`FocusedNodeReveal`**: when keyboard focus (`FOCUS_VISIBLE`) moves to a node inside an ancestor `Overflow::Scroll` container, automatically reveal the focused node's bounds within its scrollable ancestors.
   - **`MAINTAIN_SCROLL_ANCHOR`**: add `NodeFlags::MAINTAIN_SCROLL_ANCHOR`, `El::maintain_scroll_anchor(bool)`, and `VirtualOptions::maintain_scroll_anchor: bool`.
     - When enabled (or when a `follow_tail` container is temporarily scrolled up/detached!) and `FOLLOW_TAIL_ACTIVE` is not active, `LayoutEngine` tracks the first live non-hidden visible child `NodeId` and adjusts `scroll.y` by `new_y - prev_y` in the **same layout commit** (zero 1-frame jump).
     - In `virtual_each`, `VirtualRows::plan` pins the anchor row resident across topology/height updates and computes the window around the compensated offset, letting `LayoutEngine` apply the single exact compensation with zero double-counting.
     - Map `overflow-anchor` cleanly in `octara-host-web`.
2. **Native Overlay Scrollbars (`octara-model`, `octara-pipeline`, `octara-tree`, `octara-runtime`)**:
   - Add `ScrollbarVisibility::{Auto, Always, Hidden}` (default `Auto`) + `El::scrollbar(ScrollbarVisibility)`.
   - Paint thin rounded overlay scrollbar thumbs in `octara-pipeline/src/paint` for any scrollable container (`max.x > 0` or `max.y > 0`) when visible (shown on scroll / hover near edge / drag, smoothly fading after idle timeout in `Auto` mode; colored with `ColorToken::TextSubtle.alpha(...)`).
   - Support pointer track click (page jump) and thumb drag in `InteractionArbiter` + `AppRuntime` (updating `scroll_by` / `SetScroll` directly, releasing `FOLLOW_TAIL_ACTIVE` when dragged upward).

### 2.4 Native Cursor Affordance, Per-Corner Radius, Typography Details & Local `z-*` Stacking (`octara-model`, `octara-tree`, `octara-pipeline`, `octara-render`, `octara-host-native`, `octara-host-web`)
1. **Native Cursor Affordance**:
   - Define unified `resolve_cursor(tree: &Tree, hit: Option<NodeId>) -> CursorStyle`:
     1. Walk ancestors from `hit`: first explicit authored `.cursor(...)` wins (unless node is `DISABLED` -> `OperationNotAllowed` or `Arrow`).
     2. Otherwise derive semantic default: `DISABLED` interactive control -> `OperationNotAllowed`; `NodeKind::TextInput` or `NodeKind::Text` inside `SELECT_REGION` -> `IBeam`; `NodeKind::Link` or interactive clickable control (`Role::Button | Link | Tab | CheckBox | Switch | Radio | MenuItem | MenuItemCheckbox | Option`) -> `PointingHand`; fallback -> `Arrow`.
   - Wire `octara-host-native` (`shell.rs` + `window.rs`) to sync the resolved `CursorStyle` to `winit::Window::set_cursor(...)` whenever hover or tree state updates (deduped to avoid redundant OS calls).
2. **Typography Details (`italic`, `underline`, `line-through`, `tracking-*`, `tabular-nums`)**:
   - Add `italic: bool`, `underline: bool`, `strike: bool`, `letter_spacing: f32`, `tabular_nums: bool` to `TextStyle` / `Visual` / `StyleProp` + `Styled` + `tw!`, wired into Parley (`StyleProperty::FontStyle`, `LetterSpacing`, OpenType `tnum` feature), `TextEngine::items()` (text decoration), and `octara-host-web` CSS (`font-style`, `text-decoration`, `letter-spacing`, `font-variant-numeric: tabular-nums`).
3. **Per-Corner Border Radius (`rounded-t/b/l/r/tl/tr/bl/br-*`)**:
   - Upgrade `Visual.radius` and `Quad.radius` to per-corner `[f32; 4]` (top-left, top-right, bottom-right, bottom-left) across `octara-model`, `octara-pipeline`, `octara-render` (WGSL SDF + CPU oracle), `octara-host-web`, `Styled`, and `tw!`.
4. **Local Stacking Order (`z-*`)**:
   - Add `z_index: i32` (`StyleProp::ZIndex(i32)`, `.z(i32)`, `tw!("z-0 z-10 z-20 z-30 z-40 z-50")`) honored in sibling paint order, hit-test order, and `octara-host-web` CSS `z-index`.

---

## Wave 3 — Open `octara-kit` Parts/Localization, Component Default Polish & Four-Layer Markdown Architecture

### 3.1 Open `octara-kit::parts`, Shared `PopupOptions` & `KitLocale` (`octara-kit`)
1. **Export `octara_kit::parts`**:
   - Publish Kit's styled building blocks (`button_part`, `popup_surface`, `editor_style`, `option_list`, `option_row`, `focus_outline`, `input_outline`) so apps can freely compose `octara-headless` + Kit visuals without waiting on wrapper options.
2. **Shared `PopupOptions` & `ComboboxOptions`**:
   - `PopupOptions { placement: Placement, width: Option<Length>, min_width: Option<Length>, max_height: Option<Length>, match_trigger_width: bool, surface_style: Option<StylePatch> }` with fluent builders, reused across `select`, `combobox`, `combobox_input`, `popover`, `menu_button`, `context_menu`.
   - `ComboboxOptions { picker: PickerOptions, popup: PopupOptions, placeholder: Option<String>, trigger_style: Option<StylePatch>, input_style: Option<StylePatch> }` replacing the old `PickerOptions` parameter on `combobox` (zero `Rc::new` / zero `RefCell`).
3. **`KitLocale` Localization Context**:
   - Define `KitLocale` (with `KitLocale::en()` default + `KitLocale::zh_cn()` preset + field overrides) provided via `b.provide(KitLocale::...)` and consumed by all Kit components (`copy`, `copied`, `search`, `close`, `loading`, `clear_search`, `dismiss_notification`, `search_commands`, `breadcrumb`, `remove_item`).

### 3.2 Component Default Visual & Behavioral Polish (`octara-kit`)
- `button()`, `tabs()`, `segmented()`, `badge()`: structurally clean and vertically centered on both Native & Web.
- `input` / `textarea` (`editor_style`): highlight border on `State::FOCUSED` (and composite fields on `State::FOCUS_WITHIN`), while buttons keep `State::FOCUS_VISIBLE`.
- `menu_item`: render a visible check icon (`icons::CHECK`) for checked `MenuItemCheckbox` items (and aligned spacer when unchecked in a checkable menu).
- `badge` & `callout`: use semantic tinted surfaces (`tone.alpha(0.10)` background + `tone.alpha(0.25)` border) powered by Wave 1 `ColorValue::Alpha`.
- `WindowTooltipLayer` caps the default tooltip width at 280 logical pixels.
- `dialog` & `popup_surface`: use `ColorToken::SurfaceElevated` for clear dark-mode elevation contrast.

### 3.3 Four-Layer Markdown & Typography Architecture (`octara-kit::markdown`)
1. **Layer 1 — Lossless Streaming Document Engine (`Doc` + `ParseOptions`)**:
   - Preserve incremental tail-repairing streaming parser, `BlockCache` (`Arc<Block>` prefix identity), and cross-block Markdown source copy.
   - Upgrade AST:
     - `Kind::Code { lang: String, info: String, text: String }`
     - `Kind::Quote { callout: Option<CalloutKind>, blocks: Vec<Block> }` (`Note`, `Tip`, `Important`, `Warning`, `Caution`)
     - `Kind::List { start: Option<u64>, tight: bool, items: Vec<Item> }`
     - `Kind::Image { url: String, alt: String, title: String }` (and `Style::Math`, `Style::FootnoteRef`)
     - `Kind::Math(String)`
   - Add `ParseOptions { tables, strikethrough, tasklists, gfm_alerts, math, hard_breaks }`.
2. **Layer 2 — Semantic `MarkdownTheme` & Contextual Typography Rhythm**:
   - Curated presets: `MarkdownTheme::document()`, `MarkdownTheme::chat()`, `MarkdownTheme::compact()`.
   - Contextual spacing: `block_gap`, `tight_list_gap`, `loose_list_gap`, per-level `HeadingStyle { size, line_height, weight, color, top_spacing, bottom_spacing, border_bottom }`.
   - Complete inline & block styling: `strong_weight`, `strong_color`, `emphasis_color`, `strike_color`, `inline_code` (including tight glyph-box `padding_y`), `link`, `math`, `quote` (including callout tones), `rule`, `list` (customizable marker style), `code_block` (info title display, `"Copy"` → `"Copied"` transient feedback via `KitLocale`, optional line numbers, wrap, max_height), `table` (**intrinsic/content-aware column widths + horizontal scroll**, removing forced equal-width `grow(1.0).basis(0.0)`).
   - Reactive theme/density updates (no stale one-time mount snapshot).
3. **Layer 3 — Open `trait MarkdownRenderer` + `Context` Sharing**:
   - Export `pub trait MarkdownRenderer: 'static` with complete default implementations (`render_block`, `render_paragraph`, `render_heading`, `render_code_block`, `render_quote`, `render_list`, `render_table`, `render_rule`, `render_image`, `render_math`, `style_inline`, `on_link`, `on_task_toggle`, `highlight_code`) plus exported default part helpers (`default_code_block`, `default_heading`, `default_quote`, `default_table`, etc.).
   - Share ONE `Arc<dyn MarkdownRenderer>` across an entire pane/app via `b.provide(renderer)` (`Context`); `markdown(b, source)` automatically inherits it (zero per-message closure allocations in virtual chat lists!).
   - Provide fluent `MarkdownConfig` implementing `MarkdownRenderer` for quick inline configuration (`markdown_with(b, source, MarkdownConfig::chat().on_link(...))`).
4. **Layer 4 — Deep Integration with Wave 1 & 2**:
   - Tight glyph pills (`padding_y`), unified `SELECT_REGION` pointer selection + `Cmd/Ctrl+C` source copy, and automatic adaptation inside subtree `ThemePatch` (e.g. user chat bubbles!).

---

## Release & Verification Gates (`0.0.4`)

Each Wave must pass `check_layers.py`, `cargo test --workspace`, and `cargo clippy --workspace --all-targets -- -D warnings` cleanly before starting the next Wave. After Wave 3, bump workspace to `0.0.4`, regenerate `skills/octara/`, and verify all 8 gates:
1. `python3 tools/check_layers.py`
2. `cargo test --workspace`
3. `cargo test -p octara-cli --doc`
4. `cargo clippy --workspace --all-targets -- -D warnings`
5. `cargo check -p octara-host-web --target wasm32-unknown-unknown`
6. `cargo check -p octara-host-web --target wasm32-unknown-unknown --features webgpu`
7. `cargo check -p octara-host-native --target aarch64-apple-ios-sim`
8. `cargo publish --workspace --dry-run`
- Plus strict purity checks: 0 `Rc::new` / 0 `RefCell` / 0 `todo!()` in `octara-headless` and `octara-kit`; 0 `#[cfg(target_os)]` in `octara-host-native/src/{shell,window,presentation}.rs`; before/after macOS debug `__TEXT,__eh_frame` measurements recorded.

### macOS debug size measurements

Measured on `aarch64-apple-darwin` with Rust 1.100.0. Archive measurements use the default dev profile and sum `__TEXT,__eh_frame` across the Mach-O objects in each `.rlib`.

| Revision | `size_of::<El>()` | `size_of::<TextInput>()` | `octara` archive unwind bytes | `octara-kit` archive unwind bytes |
| --- | ---: | ---: | ---: | ---: |
| `0.0.3` before Wave 1 | 2,264 B | 2,264 B | 126,752 | 389,248 |
| `f55a2c2` after Wave 2 | 8 B | 8 B | 131,840 | 387,504 |
| `0.0.4` after Wave 3 | 8 B | 8 B | 132,992 | 471,992 |

Use the shared `CARGO_TARGET_DIR` and run the same build and section measurement at each revision.

```sh
cargo build --locked -p octara -p octara-kit --lib
for crate in octara octara_kit; do
    xcrun size -m "$CARGO_TARGET_DIR/debug/lib$crate.rlib" |
        awk -v crate="$crate" '/Section \(__TEXT, __eh_frame\):/ { bytes += $NF } END { print crate, bytes }'
done
```

Linked unit test executables include unwind data for dependencies and generic code instantiated in the harness.

| Revision | `octara` test executable unwind bytes | `octara-kit` test executable unwind bytes |
| --- | ---: | ---: |
| `0.0.3` before Wave 1 | 716,220 | 160,324 |
| `f55a2c2` after Wave 2 | 775,640 | 161,168 |
| `0.0.4` after Wave 3 | 776,432 | 179,184 |

Build the harnesses with `cargo test -p octara -p octara-kit --lib --no-run --message-format=json`. Use `xcrun size -m` on the two `executable` paths in the Cargo `compiler-artifact` messages and read `Section __eh_frame` under `Segment __TEXT`.

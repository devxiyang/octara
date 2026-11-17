# UI Primitives and Component Layering

Status: a settled direction. Follows Rune's layering ([research](../research/rune.md)) and Exact's component interface ([research](../research/exact.md)).
Parts marked "implemented" are in `crates/octara`; the rest are next steps.

The [2026-11-16 reduction audit](primitive-reduction.md) distinguishes actual removal candidates,
specialized APIs, and internal exports. It is a proposal; the implemented inventory below has
not yet changed.

## 1. Layers

| Layer | Package | Responsibility |
| --- | --- | --- |
| Core | `octara` | Reactivity, elements, layout, interaction states, accessibility, focus, popups, runtime, test tools. No branding; it knows only scales and token names |
| Component library | `octara-kit` | Button, Switch, TextField, Select…: unified variants, sizes, states and accessibility semantics; reads only semantic tokens; uses only the public primitives of `octara` |
| Theme | standalone crate | Token values, provided as data; light/dark and brand are each independent |

Dividing principle: only things that need runtime or platform support go into the core. Button, Checkbox and Switch do not belong in the core; the component library composes them from `el` plus a role and events.

Component interface principles taken from Exact: props are typed, and a missing required item fails to compile; theme and shared dependencies pass through context; a single `children` slot;
errors give the correct way to write it. Behavior stays native (text input, accessibility, scroll inertia), and appearance is decided by the theme (consistent with Rune; Exact's platform-native appearance is not adopted).

## 2. Core primitives

| Category | Primitives | Status |
| --- | --- | --- |
| Reactive | `state`, `memo`, `watch`, `untrack` | Implemented |
| | `state_list` and `filter` / `sort_by` / `group_by` / `map` / `join` / `flatten` | Implemented ([map and join](map-join.md)) |
| | `TextBuffer`, `text_buffer`, `paced` | Implemented |
| | Context `provide` / `use_context` | Implemented |
| | Tasks, subscriptions, resources, `clock` | Implemented ([async](async.md)) |
| Structure | `component_keyed` / `component`, `show` / `show_else`, `keyed` (a view built anew when its key changes), `each`, `virtual_each` | Implemented |
| | `.child(..)` / `.children(iter)`, `El::overlay`, modals (Escape, press outside, focus kept inside) | Implemented |
| Elements | `el`, `row`, `col`, `spacer`; `text(fixed value \| closure \| TextBuffer)` | Implemented |
| | `text_input`: single line, `.multiline()`, `.rows(min, max)`, `.submit_on_enter()`, `.secure()` / `.revealed(..)`, selection, undo, clipboard, IME composition | Implemented headless and through the shell, including macOS `NSTextInputClient`; other direct platform protocols remain ([text input](text-input.md)) |
| | Scroll containers (`overflow_*_scroll`, `scroll_to`, `follow_tail` / `following_tail` / `scroll_to_end`), images (`img`, `object_fit`) | Implemented |
| | Vector paths (`Path`, fills, strokes); `icon` with the Lucide set (`octara-icons`), `Icon::svg` | Implemented ([vector graphics](vector-graphics.md)) |
| | `canvas` (declarative recording: paths, rects, text, images, transforms, clips) | Implemented; labeled regions to do |
| | Native Island: `native_view(kind)`, `web_view(url)`, slots, placements, the host protocol, headless host | Implemented headless and on macOS (web views and maps); Windows/Linux hosts to do ([Native Island](native-island.md)) |
| Style | `Styled` (and `style_with` for bound styles): GPUI method names, 4 px scale, `px` / `relative` / `rems`; semantic `Token` and `Theme`; state, window size class, touch and group-hover overrides; `absolute` / `top` / `left`; shadows, gradients, opacity layers; motion | Implemented |
| Semantics | `role`, `label`, `description`, `required`, `invalid`, `expanded`, `checked`, `value`, `a11y_hidden`, `focusable`; test id is the semantic id | Implemented; descriptions and form validity are shared by Inspector and AccessKit |
| Interaction | `on_press` / `on_input` / `on_toggle` / `on_submit` / `on_focus` / `on_blur`; `tooltip`; hover chain and groups; disabled elements reject activation; retained focus retirement still reports blur | Implemented |
| | `El::behavior` / `Behavior`: typed component event layers before replaceable application callbacks, with shared scope-retirement checks | Implemented ([composition](input.md#component-and-application-callbacks)) |
| | `on_key`, gestures (tap, double, long press, secondary, drag, swipe, momentum), focus order and groups, keymap and commands | Implemented ([input](input.md)) |
| Agent | Structured diagnostics with registered codes, `App::set_check_consistency`, `Window::force_state` | Implemented |
| | Inspector protocol, `octara check` / `inspect` / `run` | Implemented ([agent knowledge](agent-knowledge.md)) |

## 3. Naming

- **Containers use `el` / `row` / `col` / `spacer`, not `div`.** `div` carries HTML block layout and DOM semantics; native UI frameworks generally use row / column plus a plain box
  (Rune, Exact, SwiftUI, Flutter, Compose). The four return the same element type, only with a different preset direction. `box` is a Rust reserved word, hence `el`;
  `col` matches Tailwind's `flex-col`, with `doc(alias = "column")`.
- **Text is only `text(..)`.** The type of the value passed in decides how it updates: a string is a fixed value, a closure is a binding, and `TextBuffer` updates incrementally by edits.
  The value type is called `TextBuffer` to avoid the repetition of `text(Text::new(..))`.

### Element content

`El` has one content: a plain box, text, input, image, icon, canvas or native view. Its Kernel
kind is derived from that content; there is no independent kind flag or competing payload.
Input settings live with the input; native slot/props/event callback live with the native view;
image fit and icon stroke width live with their respective content.

Use `text_input().rows(..).secure()` for input configuration, `native_view(..).slot(..).props(..)`
for native configuration, `icon(..).stroke_width(..)` for icons, and
`el().img(&image).object_fit(..)` for images. A second `.img(..)` replaces an existing image and
keeps its fit. It cannot overwrite text, input, icon, canvas, native content or a box with declared
children. Only the box content owns application children; text/input/image/icon/canvas/native
content cannot hold them. Put a caption, badge or input affix alongside the atomic content inside
an external container. Styles, handlers and semantics remain shared; `.value(..)` still sets
accessible string values on non-input nodes. Anchored overlays remain separate window-layer
content, so atomic elements can still own tooltips and popovers.

An incompatible content-specific call panics with `ELEMENT_CONTENT` at the builder caller,
before mounting or running its binding. Builders can be assembled without a current App, so
this failure is reported directly in the panic, not retained in `App::diagnostics()`. There is
no ignored configuration, deferred invalid-content variant or compatibility conversion.
`child` validates before converting its argument, and `children` before consuming its iterator,
including an empty iterator. A declared child counts even if it currently mounts no nodes;
`.img(..)` cannot silently discard an empty/dynamic child declaration.

## 4. Style

Aimed at desktop and mobile, not web standards: **method names follow GPUI** (its vocabulary comes from Tailwind, which AI knows best), **semantics follow native**, mainly Rune.
For layout semantics see [layout](layout.md).

- Containers: `el()` / `row()` / `col()` / `spacer()`, constructors with no arguments; `.child(..)` (an element, view or string), `.children(iter)`;
  only containers accept application children; boxes do not shrink by default, and text wraps by default.
- Method names as in GPUI: `p_4`, `gap_2`, `items_center`, `flex_1`, `flex_grow`, `flex_shrink_0`, `w_full`, `size_full`, `rounded_md`,
  `text_sm`, `line_height`, `whitespace_nowrap`, `truncate`, `text_ellipsis`, `line_clamp`, `overflow_y_scroll`;
  half steps and fractions: `p_0p5`, `w_1_2`; `.map(f)`, `.when(cond, f)`, `.when_some(opt, f)` (GPUI's FluentBuilder).
- Arbitrary values: a number is logical pixels (`.p(13.0)`, `.w(320)`), `relative(0.5)` is a fraction, `rems(1.5)` is a multiple of 16, and `sp(7.0)` is an arbitrary step of the theme spacing.
- Color is a `Paint`: a semantic `Token` (`Surface`, `Text`, `Accent`…, extensible with `Token::custom("brand")`) or an explicit `Color`.
- **Scales and colors are resolved by the kernel according to the theme**: `p_4` is stored as "step 4"; a theme can change colors, the spacing unit, corner radius / font size / container steps, line height multiples, window size class boundaries,
  and font families. Switching themes (dark, compact density, brand) runs no closures. A custom token missing from the theme is painted magenta and reported in diagnostics.
- Base styles follow declaration order, including fixed values, Signal properties and `style_with`.
  Updating a binding changes its value, never its precedence. A `style_with` run replaces its
  entire patch; properties it omits fall back to earlier declarations or defaults. Only bindings
  whose dependencies changed rerun. Reusing a style is just a Rust function: `el().map(card)`.
- State overrides: `hover`, `active`, `focus` / `focus_visible`, `style_when(State::SELECTED | CHECKED | EXPANDED | DISABLED, ..)`;
  window size class overrides: `when_medium` / `when_expanded` / `when_large` / `when_extra_large`. Size class is merged first, then state.
- Fonts: Inter and IBM Plex Mono are bundled (headless tests use them for reproducible results), `App::register_font` registers more; real windows use the system UI font and fallback by default.
- The kernel invalidates by change category: `Dirty::{LAYOUT, PAINT, TEXT, SEMANTICS}`, and changing a color does not trigger layout.

```rust
const BRAND: Token = Token::custom("brand");
let theme = Theme::light().color(BRAND, rgb(0xff6600)).spacing(3.0).breakpoint(Breakpoint::Expanded, 900.0);

row()
    .child(text("Save").font_medium())
    .gap_2().px_3().h_8().rounded_md()
    .bg(BRAND).text_color(Token::AccentText)
    .hover(|p| p.opacity(0.9))
    .when_expanded(|p| p.px_4())
    .role(Role::Button).focusable()
    .on_press(save)
```

## 5. Overlays

`El::overlay(open, placement, content)`: when `open` holds, `content` is mounted onto the kernel's overlay (the root's last child, so it is painted on top).
After each layout it is positioned relative to the anchor (as in Rune's `anchored`): `Placement { side, align, gap, margin, fit }`, with presets `BELOW` (`fit`), `ABOVE`, `RIGHT` (`fit`).

- If it does not fit, it flips to the opposite side; it always stays `margin` (8 px, as in Rune) from the window edge;
- `fit` (as in Rune's `anchored_fit` and Floating UI's `size`): if it fits on neither side, take the side with more space, limit the size to the remaining space, and scroll the content inside the overlay;
- Without `fit`, if it fits on neither side, the overlay is pushed back into the window and may cover the anchor, and diagnostics report `OVERLAY_CLAMPED`;
- An overlay is a scope, state is kept while it is open; it is removed together when the scope containing the anchor is destroyed. `Kernel::placed_side` shows the actual direction.
- Nested anchors settle within the same layout update, including simultaneous mount, hot reload
  and window resize. Placement/layout passes are bounded by the number of anchored surfaces.

## 6. Component library (`crates/octara-kit`)

- A component is a function returning `El` (or `View`), and the caller continues to chain `id`, styles, `disabled` and events; no builder structs, which saves forwarding all of `El`'s methods;
- A controlled component takes a `Model<T>`: a `Signal<T>`, or `(getter, setter)` when the value lives elsewhere (such as a list entry);
- Components bring their own accessibility semantics (role, checked, expanded, selected; decorative nodes are `a11y_hidden`); disabling is handled by the core rejecting events;
- Components that need internal state use the id as key (`select(id, ..)`), so the open state is kept after hot reload. A `#[component]` function has no scope of its own: one returning `El` keeps no named state (what is open, selected or how wide comes from the caller's `Model`).

| Component | Semantics | Event |
| --- | --- | --- |
| `button` / `primary_button` / `ghost_button` / `toggle_button(label, selected)` | `Button` | Press |
| `checkbox(model)` | `CheckBox` + checked | Toggle |
| `switch(model)` | `Switch` + checked (the thumb moves with `CHECKED`) | Toggle |
| `text_field(model)` | `TextInput` + value | Input |
| `field(id, label, control, info)` | Applies name, combined help/error description, required and invalid to the retained control; supporting error is a live region | The control's events; label press focuses it |
| `radio_group(id, label, model, options)` | `RadioGroup` with stable `RadioButton` children + checked; one Tab stop | Press, Space/Enter, arrows/Home/End choose |
| `dialog(id, title, open, options, content)` | Named `Dialog`, description and heading; bounded body | Existing modal stack, Escape/outside/close, focus restoration |
| `tabs(id, label, model, options)` | Stable `TabList`/`Tab` + selected; application-owned panels | Manual activation: arrows focus, Enter/Space choose |
| `segmented(id, label, model, options)` | Compact `RadioGroup`/`RadioButton` + checked | Radio keyboard behavior |
| `popup_list(id, label, open, editor, items, choose)` | Editor's expanded/active-descendant; stable `ListBoxOption` rows | Editor retains focus, Up/Down/Enter, IME precedence |
| `slider(id, model, min, max, step)` | `Slider` with numeric range | Pointer track/drag, arrows/Home/End/Page, numeric setter |
| `number_field(raw, min, max, step)` | Text editor with `SpinButton`, invalid and numeric range | Raw edits, commit on Enter/blur, Up/Down, numeric setter |
| `progress(value, max)` | Read-only `ProgressIndicator`; optional numeric value | None means unknown; no setter |
| `menu_button(id, label, open, menu)` | Named trigger and `Menu`/`MenuItem`/checked items | Shared drawn menu surface, submenus, typeahead, deferred command |
| `select(id, model, options)` | The trigger is `ComboBox` + value + expanded; `{id}-list` is `ListBox`, and the options `{id}-option-{i}` are `ListBoxOption` + selected | Press |
| `combobox(id, label, model, options)` | A named `ComboBox` and searchable `ListBox`; the input's `active_descendant` identifies the highlighted option independently of the selected value | Press, input, Up/Down, Enter |
| `popover(id, open, placement, trigger, content)` | An anchored modal surface; the trigger exposes `expanded`, and focus stays inside until it closes | Press, Escape, press outside |
| `icon_button(icon, label)` | `Button` named `label`, which is also its tooltip | Press |
| `spinner()` | `ProgressIndicator` named "Loading"; its arc steps round on the app's clock, every spinner in one phase | — |
| `chip(label)` / `removable_chip(label, remove)` | A small label; the second has a button named "Remove {label}" | Press (remove) |
| `collapsible(id, open, header, content)` | A `Group`; its trigger `{id}-trigger` is a `Button` with `expanded`; the content `{id}-content` is built while open | Press |
| `list()` / `list_item(content)` / `list_header(label)` | `ListBox` (a focus group: arrow keys move between items) / `ListBoxOption` with hovered, focused and selected looks / a `Heading` | Press, secondary press |
| `split(id, width, leading, trailing)` | Two panes; the divider `{id}-divider` is a `Splitter` whose value is the width; drag or arrow keys (Shift: by 40) change it, at least 120 | Drag, keys |
| `callout(level, message, detail)` | A live region; `Alert` for a warning or a critical one, else `Group`; the level's icon and color (`Accent`, `Warning`, `Danger`); actions chained as children go after the text | — |

```rust
primary_button("Save").id("save").disabled(move || busy.get()).on_press(save)
switch(sync).id("sync").label("Sync")
checkbox((move || t.with(|t| t.done), move |v| todos.update(&id, |t| t.done = v)))
select("model", model, [(Model::Fast, "Fast"), (Model::Best, "Best")])
```

### Searchable choices and popovers

`popover` takes an ordinary `El` trigger and controlled `Model<bool>`. It returns the trigger so
callers can keep chaining styles and `disabled`. Its `{id}-popover` surface supplies padding,
border and shadow; the content is ordinary application UI. This first popover is modal: it
uses the existing core focus stack and outside-press dismissal. A popup opened inside a dialog
becomes the top modal; closing it restores the previous focus and leaves the dialog open.
Nonmodal hover cards and portal-wide event inheritance are outside this API's current contract.

`combobox` takes a `Model<Option<String>>` containing the selected option ID and a tracked
closure returning `Vec<ComboboxOption>`. Each option has a stable `id`, `label` and `disabled`.
Changes reconcile by ID, preserving the search field and unaffected option nodes. Search is
case-insensitive substring matching over labels. This is an ordinary retained list for finite
menus; asynchronous search, fuzzy ranking and virtualized results are not part of this component.

- Opening starts on the selected enabled option, or the first enabled option. Query edits start
  on the first enabled match; Up/Down loop through enabled results while the input keeps focus.
- Enter chooses the highlighted option. The core gives composition Enter/Escape to the IME
  before component handlers. Modified editing keys remain with the text editor.
- Highlight is `active_descendant`, selection is `selected`, and keyboard focus remains on
  `{id}-search`. Inspector and AccessKit expose the resolved active option; missing or hidden
  targets do not produce dangling references.
- `scroll_into_view(id)` reveals the option after layout without moving focus. It uses existing
  scroll containers, including the popup's constrained viewport.
- Removing a selected option does not rewrite the application's value. The trigger falls back
  to its label. Closing and reopening clears the query; hot reload retains named state and
  restores the modal stack's opener addresses against the rebuilt tree.

The layering follows Rune's [Dialog](https://github.com/anomalyco/rune/blob/f1f54ce8bf485ee3a95804deace315f53a8970f6/crates/rune-kit/src/dialog.rs)
and [Combobox](https://github.com/anomalyco/rune/blob/f1f54ce8bf485ee3a95804deace315f53a8970f6/crates/rune-kit/src/combobox.rs):
focus and dismissal belong to the runtime, while query, highlight and choice belong to the
component. Octara retains Signals and ordinary function-based components. Native-input evidence
is recorded separately from headless/framework input, following Exact's
[testing distinction](https://github.com/expo/exact/blob/4c16fa59f74b1920af7dbc861a15a8105ed0ad13/docs/testing.md).

### Fields and radio groups

`FieldInfo` is plain data computed from application state: description, optional error and
required. `field` retains its control and binds these semantics to that control, including the
combined help/error description. A blank error is treated as absent. The same description
resolver serves Inspector and AccessKit, preferring an explicit description over a tooltip;
a tooltip that repeats the label is omitted. No validation engine or automatic submission is
introduced: the application's handler decides whether and what to save. The Showcase form
keeps raw editing text even when its saved value is trimmed.

`radio_group` takes `Model<Option<String>>` and tracked `Vec<RadioOption>` data. Options have
stable IDs, labels, descriptions and disabled flags. The retained rows follow those IDs
through reordering and renaming. A removed selected ID is left in the model, with no checked
row. Focus and selection remain separate: Tab or a semantic focus action only focuses;
arrows, Home and End choose via the existing press handler. A radio group uses its checked
enabled member as its entry point when focus is outside. Navigation skips disabled choices
and rechecks ownership and modal eligibility after focus callbacks before pressing.

This follows the thin composition in Rune's
[Field](https://github.com/anomalyco/rune/blob/38e3535b797377c38a34654ec0e98694e7e46696/crates/rune-kit/src/field.rs)
and [RadioGroup](https://github.com/anomalyco/rune/blob/38e3535b797377c38a34654ec0e98694e7e46696/crates/rune-kit/src/radio.rs),
while retaining Octara's ordinary functions, Signals and stable option IDs. Exact's
[number-field recipe](https://github.com/expo/exact/blob/814d5e8821c1a1aace2f027770fc4f95c9c3fe0b/docs/recipes/number-field.md)
motivates preserving raw text during editing. Its
[tablist regression](https://github.com/expo/exact/blob/814d5e8821c1a1aace2f027770fc4f95c9c3fe0b/issues/closed/20261110-tablist-arrows-do-not-rove-on-painted-hosts.md)
motivates explicit keyboard and modal composition tests, rather than inferring behavior from
a role alone. Verification and limits are in [Forms](../../examples/FORMS.md).

### Composed interaction and numeric controls

Dialog is a thin composition of `overlay_window` and the existing modal stack. Its content is
scoped to each opening; application state owns a draft that must survive closing. The window
overlay has a viewport-sized wrapper, so it works inside clipped or scrolled parents. An
anchored, nonmodal suggestion popup belongs to its anchor's modal interaction scope without
changing physical containment used for layout and pointer hit testing.

Tabs use manual activation: changing focus does not switch a panel. The application chooses
whether to retain hidden panels with display bindings or dispose them with `show`. This keeps
requests, drafts and scroll lifetime explicit. Segmented choices use radio semantics and
select with arrows. Both retain option identities through live reordering and renaming.

PopupList owns no text model or search service. The caller supplies the editor, filtered
items and a choose callback. `preserve_focus_on_press` keeps editor selection when a suggestion
is clicked. An active composition owns candidate-navigation keys; the guarded Inspector path
also respects handled Space instead of synthesizing an extra committed space. Neither pointer
suggestion choices nor numeric setters replace marked text during composition.

`NumericRange` and one numeric setter share values/bounds/steps between Inspector and AccessKit.
Slider, NumberField and Progress compose that contract. NumberField retains the raw draft until
commit, including empty strings, signs and incomplete exponents. Its component behavior commits
before application submit/blur callbacks; invalid drafts remain unchanged and still notify. Unknown progress has no numeric value.
This adds no form schema or numeric state manager.

Popover and MenuButton register their trigger behavior through `El::behavior`; application
press/key callbacks preserve opening and navigation. Popup mounting follows the open-model
notification at the normal update boundary.

Drawn context menus and kit menus share one surface. A menu snapshots finite items per opening;
action IDs are stable, while index-based submenu IDs are valid only for that snapshot. The
window-owned `invoke_after_update` queue routes commands after pending updates, modal restoration
and explicit focus requests. It is bounded and reports unhandled commands or cycles through
registered diagnostics; it does not require a timer to guess when focus has returned.

These boundaries follow Rune's
[Dialog](https://github.com/anomalyco/rune/blob/80022c59736b9ef48c2b6954ba29bafbc78ae428/crates/rune-kit/src/dialog.rs),
[PopupList](https://github.com/anomalyco/rune/blob/80022c59736b9ef48c2b6954ba29bafbc78ae428/crates/rune-kit/src/popup_list.rs)
and [tab strip](https://github.com/anomalyco/rune/blob/80022c59736b9ef48c2b6954ba29bafbc78ae428/crates/rune-kit/src/shell/tab_strip.rs),
with Octara's Signals and string IDs. Exact's
[edit-sheet example](https://github.com/expo/exact/blob/32f79aa86ce5ea5a528f69ba52636df72320be79/docs/recipes/edit-sheet.md)
illustrates draft ownership; its
[number field](https://github.com/expo/exact/blob/814d5e8821c1a1aace2f027770fc4f95c9c3fe0b/docs/recipes/number-field.md)
illustrates raw editing text. See [the component verification record](../../examples/PRIMITIVES.md)
for tested behavior and the distinction between framework input and native OS input.

Planned, by kind. Names say what a
component is, not where it is used (no sidebar, composer or message components: an app composes
those), take the name most frameworks share (Zed's `ui`, Rune's kit, SwiftUI / AppKit), avoid one
platform's jargon and names Octara already uses for something else (`prompt` is a platform alert), and a
variant is a chained method, not a new name (`text_field(m).multiline().rows(1, 8)`).

| Kind | Components |
| --- | --- |
| Collections | `tree`, `table` |
| Disclosure and navigation | Native navigation stacks and tab bars |
| Status | `badge`, `keybinding` |
| Overlays | `toast`, sheets, native tooltips |
| Layout | `divider` |

Adding one todo in the example app (`examples/src/apps.rs`) costs about 5 µs: each component adds a few style and model closures; the fixed styles of a new node are resolved once.

## Activation and lifecycle

Direct semantic, pointer/key activation and accessibility click/value actions share the same retained-target
checks: shown, enabled and inside the current modal. Focus also requires focusability; pointer
input still requires a reachable hit. Disabling/hiding a focused control or opening a modal
retires its composition and focus while still delivering `on_blur` once. Modal dismissal remains
available when its container is disabled. Disposed/leaving controls use scope cleanup instead of
revived notifications. Application commands can intentionally cancel work behind a modal; a
semantic press cannot bypass the modal boundary to activate its background UI.

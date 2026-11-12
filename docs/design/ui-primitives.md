# UI Primitives and Component Layering

Status: a settled direction. Follows Rune's layering ([research](../research/rune.md)) and Exact's component interface ([research](../research/exact.md)).
Parts marked "implemented" are in `crates/octara`; the rest are next steps.

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
| | `list` and `filter` / `sort_by` / `group_by` / `map` / `join` / `flatten` | Implemented ([map and join](map-join.md)) |
| | `LiveText`, `live_text` | Implemented |
| | Context `provide` / `use_context` | Implemented |
| | Tasks, subscriptions, resources | Implemented ([async](async.md)) |
| Structure | `component_keyed` / `component`, `show` / `show_else`, `each`, `virtual_each` | Implemented |
| | `.child(..)` / `.children(iter)`, `El::overlay`, modals (Escape, press outside, focus kept inside) | Implemented |
| Elements | `el`, `row`, `col`, `spacer`; `text(fixed value \| closure \| LiveText)` | Implemented |
| | `text_input`: single line, `.multiline()`, `.rows(min, max)`, `.submit_on_enter()`, `.secure()` / `.revealed(..)`, selection, undo, clipboard, IME composition | Implemented headless and through the shell; direct platform text protocols to do ([text input](text-input.md)) |
| | Scroll containers (`overflow_*_scroll`, `scroll_to`), images (`img`, `object_fit`) | Implemented |
| | Vector paths (`Path`, fills, strokes); `icon` with the Lucide set (`octara-icons`), `Icon::svg` | Implemented ([vector graphics](vector-graphics.md)) |
| | `canvas` (declarative recording: paths, rects, text, images, transforms, clips) | Implemented; labeled regions to do |
| | Native Island: `native_view(kind)`, `web_view(url)`, slots, placements, the host protocol, headless host | Implemented headless; platform views in the shells to do ([Native Island](native-island.md)) |
| Style | `Styled`: GPUI method names, 4 px scale, `px` / `relative` / `rems`; semantic `Token` and `Theme`; state, window size class, touch and group-hover overrides; `absolute` / `top` / `left`; shadows, gradients, opacity layers; motion | Implemented |
| Semantics | `role`, `label`, `description`, `expanded`, `checked`, `value`, `a11y_hidden`, `focusable`; test id is the semantic id | Implemented |
| Interaction | `on_press` / `on_input` / `on_toggle` / `on_submit`; hover chain and groups; disabled elements reject events | Implemented |
| | `on_key`, gestures (tap, double, long press, secondary, drag, swipe, momentum), focus order and groups, keymap and commands | Implemented ([input](input.md)) |
| Agent | Structured diagnostics with registered codes, `App::set_check_consistency`, `Window::force_state` | Implemented |
| | Inspector protocol, `octara check` / `inspect` / `run` | Implemented ([agent knowledge](agent-knowledge.md)) |

## 3. Naming

- **Containers use `el` / `row` / `col` / `spacer`, not `div`.** `div` carries HTML block layout and DOM semantics; native UI frameworks generally use row / column plus a plain box
  (Rune, Exact, SwiftUI, Flutter, Compose). The four return the same element type, only with a different preset direction. `box` is a Rust reserved word, hence `el`;
  `col` matches Tailwind's `flex-col`, with `doc(alias = "column")`.
- **Text is only `text(..)`.** The type of the value passed in decides how it updates: a string is a fixed value, a closure is a binding, and `LiveText` updates incrementally by edits.
  The value type is called `LiveText` to avoid the repetition of `text(Text::new(..))`.

## 4. Style

Aimed at desktop and mobile, not web standards: **method names follow GPUI** (its vocabulary comes from Tailwind, which AI knows best), **semantics follow native**, mainly Rune.
For layout semantics see [layout](layout.md).

- Containers: `el()` / `row()` / `col()` / `spacer()`, constructors with no arguments; `.child(..)` (an element, view or string), `.children(iter)`;
  every element is a flex container, boxes do not shrink by default, and text wraps by default.
- Method names as in GPUI: `p_4`, `gap_2`, `items_center`, `flex_1`, `flex_grow`, `flex_shrink_0`, `w_full`, `size_full`, `rounded_md`,
  `text_sm`, `line_height`, `whitespace_nowrap`, `truncate`, `text_ellipsis`, `line_clamp`, `overflow_y_scroll`;
  half steps and fractions: `p_0p5`, `w_1_2`; `.map(f)`, `.when(cond, f)`, `.when_some(opt, f)` (GPUI's FluentBuilder).
- Arbitrary values: a number is logical pixels (`.p(13.0)`, `.w(320)`), `relative(0.5)` is a fraction, `rems(1.5)` is a multiple of 16, and `sp(7.0)` is an arbitrary step of the theme spacing.
- Color is a `Paint`: a semantic `Token` (`Surface`, `Text`, `Accent`…, extensible with `Token::custom("brand")`) or an explicit `Color`.
- **Scales and colors are resolved by the kernel according to the theme**: `p_4` is stored as "step 4"; a theme can change colors, the spacing unit, corner radius / font size / container steps, line height multiples, window size class boundaries,
  and font families. Switching themes (dark, compact density, brand) runs no closures. A custom token missing from the theme is painted magenta and reported in diagnostics.
- For the same kind of property, the one written later wins; reusing a style is just a Rust function: `el().map(card)`.
- State overrides: `hover`, `active`, `focus` / `focus_visible`, `style_when(State::SELECTED | CHECKED | EXPANDED | DISABLED, ..)`;
  window size class overrides: `when_medium` / `when_expanded` / `when_large` / `when_extra_large`. Size class is merged first, then state.
- Fonts: Inter and IBM Plex Mono are bundled (headless tests use only them, so results are reproducible), `App::register_font` registers more; real windows will use the system UI font.
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

## 6. Component library (`crates/octara-kit`)

- A component is a function returning `El` (or `View`), and the caller continues to chain `id`, styles, `disabled` and events; no builder structs, which saves forwarding all of `El`'s methods;
- A controlled component takes a `Model<T>`: a `Signal<T>`, or `(getter, setter)` when the value lives elsewhere (such as a list entry);
- Components bring their own accessibility semantics (role, checked, expanded, selected; decorative nodes are `a11y_hidden`); disabling is handled by the core rejecting events;
- Components that need internal state use the id as key (`select(id, ..)`), so the open state is kept after hot reload.

| Component | Semantics | Event |
| --- | --- | --- |
| `button` / `primary_button` / `ghost_button` / `toggle_button(label, selected)` | `Button` | Press |
| `checkbox(model)` | `CheckBox` + checked | Toggle |
| `switch(model)` | `Switch` + checked (the thumb moves with `CHECKED`) | Toggle |
| `text_field(model)` | `TextInput` + value | Input |
| `select(id, model, options)` | The trigger is `ComboBox` + value + expanded; `{id}-list` is `ListBox`, and the options `{id}-option-{i}` are `ListBoxOption` + selected | Press |

```rust
primary_button("Save").id("save").disabled(move || busy.get()).on_press(save)
switch(sync).id("sync").label("Sync")
checkbox((move || t.with(|t| t.done), move |v| todos.update(&id, |t| t.done = v)))
select("model", model, [(Model::Fast, "Fast"), (Model::Best, "Best")])
```

Adding one todo in the example app (`examples/src/apps.rs`) costs about 5 µs: each component adds a few style and model closures; the fixed styles of a new node are resolved once.

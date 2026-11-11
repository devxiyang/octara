# Input research

2026-11. Compares Rune (source read), GPUI (Zed `main` 985bbf4 source), Expo's Exact (`main` 08c889a9, docs read) and native / Rust
frameworks (Flutter, SwiftUI, Compose, React Native, Slint, Iced, Floem, Masonry, egui) and each platform's text input protocol.
Rune, GPUI and Exact are first-hand conclusions; the other frameworks are compiled from official documentation, and details should
be checked against the documentation for the relevant version.

## Pointer

| | Rune | GPUI |
| --- | --- | --- |
| Hit testing | One flat list per frame, `(NodeId, clipped rect)`, in paint order; walks backward to find the first "interactive" element (has a handler, text input, focusable, modal, selectable text) | Only elements with interaction insert a `Hitbox`; walks backward collecting **all** hitboxes containing the point, stopping at `occlude()` (BlockMouse) |
| Target | **A single target**: no bubbling, no capture phase, no `stop_propagation`; selectable text forwards the press to the nearest ancestor with a handler | One listener list per window, Capture (back → front) then Bubble (front → back); `stop_propagation` |
| Hover | Recomputed on move and after layout changes; `hover_delay` state machine (enter / leave delay) | Recomputed after every paint; ancestors hover automatically (their hitboxes also contain the point); `group_hover` |
| Click | Fires on release, provided the pointer is still inside the element and no text is selected; moving out becomes `Outside`, moving back restores `Inside` (AppKit style); scrolling while held → `Cancelled` (UIScrollView style) | Not a primitive: bubble-phase down records `pending_mouse_down`, capture-phase up keeps it only if still hovered, bubble fires `on_click`; `ClickEvent` has Mouse / Keyboard / Touch variants |
| Capture | `Press::Held`: subsequent moves and the release go to the pressed path | `capture_pointer(hitbox)`: released automatically on release |
| Multiple clicks | Times them itself, 4 pt slop, `clicks` = 1/2/3; `on_double_press` | macOS uses the system `clickCount`; touch uses a recognizer |
| Right click | A separate `secondary` held state; Ctrl+click, the Menu key and Shift+F10 go through the same handler; native menu | `on_aux_click` / `on_mouse_down` with a `button` |
| Drag | `on_drag(Drag{phase, x, y, dx, dy, clicks})`, in the element's own coordinates; cancellation via `Drag::cancelled()` | `on_drag(value, preview view)`, starts beyond 2 px; `on_drop::<T>` by type; `drag_over` style |
| File drag in / out | `on_drop_files`, accepts by extension; drag out is macOS only | System file drag-in is converted to the same mechanism (`ExternalPaths`) |
| Cursor | — | Requested per frame by hitbox; the last hovered request wins |

How native frameworks do it:

- Flutter fixes the target path when the press happens and does not change it afterwards; Compose has three passes,
  `Initial / Main / Final`; in Masonry the framework owns hover, capture and focus, and elements change them through `EventCtx`'s
  `set_handled` / `capture_pointer` / `request_focus`.
- React Native's `Pressable` is a press state machine: press in, press out, long-press delay, hit slop, retention region.

## Keyboard, focus and shortcuts

| | Rune | GPUI |
| --- | --- | --- |
| Focus | Explicit state machine: `None{from}` / `On{path}`, recording the input modality (pointer / keyboard) → the focus ring shows only after keyboard interaction | `FocusHandle`; `last_input_was_keyboard()` decides the focus-visible style; input modality Keyboard / Mouse / Touch |
| Tab | Built in: `tab_index`, `focus_group()` arrow-key roaming, Tab cycling and scrolling into view | **No default Tab binding**; the app binds `focus_next` itself |
| Modal | Focus stack: only the topmost modal captures focus, in-progress gestures are cancelled on open, focus is restored on close; Escape closes | No focus trap |
| Shortcuts | Chords are data (`"mod+shift+t"`, parsed per platform); **no central keymap**, conflicts are resolved by dispatch order | `actions!` typed actions + `on_action`; the keymap matches against the `key_context` stack, depth first, later additions take priority; multi-key sequences, 1 s timeout |
| Dispatch order | Return / Escape during IME composition → focused element `on_key` (inside out) → default button → app `on_key` → text input → focus navigation → text editing | Interceptors → keymap bindings → `on_key_down` (capture, bubble) → platform hands text to the input handler |

Native frameworks: Flutter's `Shortcuts → Intent → Actions` trio (verbose); Compose `onPreviewKeyEvent` / `onKeyEvent`; SwiftUI
`.keyboardShortcut` is menu-centric; all platforms decide the focus ring by the "last input modality".

## Text input

| | Rune | GPUI |
| --- | --- | --- |
| Model | The app holds a `TextEdit`: `String` + anchor / focus + snapshot undo (128 steps, 1 MiB); grapheme cluster movement, ICU word segmentation | The framework provides only the protocol; the editor (Zed's `editor`) implements everything itself; `examples/input.rs` demonstrates this in 912 lines |
| IME | **Still goes through winit in practice** (`Ime::Preedit / Commit`, `set_ime_cursor_area`); the direct NSTextInputClient, TSF and text-input-v3 integration written in the PLAN is not yet implemented | `InputHandler` trait, UTF-16 ranges, mapping one-to-one onto NSTextInputClient: selection, marked range, `replace_text_in_range`, `replace_and_mark_text_in_range`, `bounds_for_range`, `character_index_for_point`; mobile adds `set_selected_text_range`, editable range, keyboard configuration |
| Registration | The focused text input | `handle_input(&focus_handle, ..)` at paint time; only the focused one takes effect, one per frame |
| Routing | Return / Escape during composition belong to the IME | macOS: bindings run first, and unhandled input goes to `inputContext`; while composing, the IME has priority |
| iOS | Not implemented | A hidden `GPUITextInputView` as first responder, implementing `UITextInput` |
| Long text | 12 ms per keystroke at 50 KB (whole-paragraph reflow); splitting into blocks is recommended | Zed uses a rope / SumTree |

What the platform protocols have in common: they all require the app to expose **text, selection, composition range and insertion
point rect**, and to accept **replacement by range**. NSTextInputClient, UITextInput, Android `InputConnection`, TSF and Wayland
text-input-v3 can all be mapped onto this one model; Parley's `PlainEditor` already implements text, selection, composition and
caret geometry.

## Scrolling

| | Rune | GPUI |
| --- | --- | --- |
| Target | The innermost scroll container on that axis under the pointer | **All** scrollables under the pointer receive it (no `stop_propagation`), isolated by occlusion |
| Nesting | Clamps at the edge and does not pass to the outer container | No rule |
| Axis lock | `WheelLock` locks the axis per gesture (including inertia) | Locks the dominant axis for precise deltas (1.9 ratio, 28 ms interval) |
| Inertia | System inertia on desktop; `fling` decay when headless or without system inertia | System inertia on macOS; synthesized itself for touch (iOS 0.998/ms decay, Android friction spline) |
| Custom | `on_wheel` (return true to take over), `on_magnify` | The element's own scroll listener |

## Touch and gestures

| | Rune | GPUI |
| --- | --- | --- |
| Model | Touch maps to the mouse: first finger = primary button; tap = click; drag if `on_drag` exists, otherwise pan scrolls and cancels the press; other fingers ignored | The platform sends only raw touches; **portable recognizers live in the core** (`gestures.rs`): 8 px slop, 500 ms long press, 400 ms / 16 px multi-tap |
| Tap | A press state on touch down | Down + up are synthesized only on lift (touch has no down state) |
| Long press | None | The element claims it with `prevent_default`; touch long press shows a hint |
| System gestures | macOS trackpad pinch / rotate | iOS left-edge back gesture (starts only if an element claims it); `WindowInsets{safe_area, ime}` |
| Mobile | Not started (PLAN M6/M7) | Usable on iOS |

Native frameworks: Flutter's gesture arena (tap, drag and long-press compete, adjudicated by slop and timers, scrolling also
participates); SwiftUI declarative gestures with priorities; Compose resolves scroll and drag conflicts with consumption flags.

## Testing and agents

| | Rune | GPUI |
| --- | --- | --- |
| Driving | JSON scripts (`move / down / up / tap / drag / key / type / wheel / shortcut / paste …`) with timestamps | `simulate_keystrokes("cmd-s a")`, `simulate_input`, `simulate_click(pos)`, `dispatch_action` |
| Addressing | Coordinates or element key / path (takes the center) | `debug_selector` records bounds, then clicks the center; no query by role / name |
| Time | Virtual clock, deterministic | Recognizer tests pass explicit times |

The best among native frameworks are egui_kittest, Compose and Masonry: they go through the same entry point as system input and
look up by semantics or id instead of coordinates; accessibility actions (AccessKit `Click`, `Focus`, `SetValue`) are a second
input channel that does not rely on coordinates.

## Exact

Exact's approach is "write web, ship platform": authors write DOM-style handlers, and gestures, keyboard and accessibility are left
to native controls as much as possible (AppKit / UIKit on Apple), the opposite of Octara's orientation.

- **Events:** `press` is the DOM `click`, delivered to the first node upward from the press point that accepts presses, and no
  further up; six events including `key`, `pointerdown/up` and `wheel` bubble as in the DOM, and `keycapture` goes outside-in
  first; `stopPropagation` / `preventDefault` are host commands (`docs/contract-grammar.md:1615-1669`). A pointer is implicitly
  owned by the innermost node that heard it until release.
- **Gestures:** after `pan` exceeds the slop it takes over the touch point and the press no longer fires, while a light tap still
  presses; the innermost recognizer wins (`contract-grammar.md:1875-1879`). `contextmenu` covers both right click and long press,
  and the Menu key and Shift+F10 open it at the center of the focus (`:1954-1969`). There is no long-press handler; **a generic
  gesture arena was explicitly rejected** (`rules/DEFERRED.md:106-112`). Inertia, pull-to-refresh and swipe-to-delete all use the
  native ones. Nested scrolling: after the inner one scrolls to the end, continued dragging is handed to the outer one.
- **Keyboard:** Tab order follows HTML `tabindex`; the focus ring is drawn by the platform; a modal makes the outside inert.
  Shortcuts are written on the button's `aria-keyshortcuts` and automatically become menu items on macOS; **there is no keymap or
  command registry**. Both the logical `key` and the physical `code` are provided, so shortcuts still work after switching
  keyboard layout. During IME composition only modifier keys and ⌘ chords reach the handler, and a ⌘ chord first commits the
  composition and then runs (`:2369-2382`).
- **Text:** native controls hold the editing state and the runtime holds only the controlled value; a value rewritten by the app
  during composition takes effect only after composition ends; it is recommended to normalize on commit (`change`) rather than on
  every `input`. Selection is in UTF-16 offsets. Undo, clipboard and IME differ greatly across hosts, and the documentation is full
  of per-platform exceptions (`:2216-2312`).
- **Agents:** addressing only by `testId`, "never by screen coordinates"; without a `testId`, by exact accessibility name or text,
  rejecting duplicates (`docs/contract-for-agents.md:3913-3921`). Before a tap it checks for occlusion or being off-screen and
  scrolls into view first; the reply states whether the input was delivered semantically or as a real touch (`delivery`). Virtual
  clock `clock +N`; a held key repeats after 500 ms and every 83 ms thereafter.

## Octara today

`app/input.rs`: `pointer_move / leave / down / up`, `click_at`, `wheel`, `Key {Tab, BackTab, Enter, Space, Backspace, Escape}`,
`type_text`, `set_preedit`, `caret`, accessibility `Click / Focus`. The target is the hit point or the nearest ancestor with a
handler / that is focusable (consistent with Rune's single target); a click requires press and release on the same element; the
wheel goes to the innermost scroll container on that axis; Tab cycles in tree order.

Missing: pointer ids and `Cancel`; capture after press (move out / move back, scroll cancellation); distinguishing input modality
and focus-visible (the focus ring is currently shown whenever focused); focus scopes and modal traps; multiple clicks, right click,
drag, long press, multi-finger; modifier keys and shortcuts; text selection, caret movement, undo, clipboard; platform text
protocols; virtual clock.

## Conclusions

1. **Single target + capture** (Rune) is simpler than capture / bubble (GPUI) and sufficient; GPUI's own click-cancellation bug
   (issue 24600) shows that press state in per-element closures is error-prone. Press state should be held uniformly by the
   runtime.
2. **Click is derived:** mouse, touch, Enter / Space and accessibility `Click` all land on the same `on_press` (both agree with
   Compose and Flutter).
3. **Put gesture recognition in the portable core** (GPUI), with the platform sending only raw touches and parameters adjustable;
   semantics follow Rune: scrolling cancels the press, moving out does not fire.
4. **The focus ring follows the input modality** (both agree); Tab and modal focus traps are built in (Rune; GPUI does not provide
   them).
5. **Fix the text input protocol early** (the shape of GPUI's `InputHandler`, and Rune's PLAN also wants to connect directly to
   the platform): one focus handler exposing text, selection, composition, replacement by range and geometry. (The implementation
   did not use Parley's `PlainEditor`: it carries its own layout and font context, bypassing the kernel's `TextEngine` and
   paragraph cache, and the deterministic test engine cannot be used with it; instead the runtime has its own model, with caret
   geometry given by `TextEngine::caret_stops`.)
6. **Shortcuts:** Rune has only chord data and no keymap; GPUI has typed actions and a context keymap. For agents, "the keymap is
   data" is better for enumeration and testing.
7. **Tests go through the real path**, addressed by test id (we already have this), with time on a virtual clock; when no test id
   is found, by accessibility name, and duplicates, occlusion and invisibility are all errors (Exact).
8. **Take Exact's details:** right click, long press and the Menu key share one context-menu handler; the keymap can also match by
   physical key position; Cmd / Ctrl chords during IME composition first commit the composition; the controlled value is not
   rewritten during composition.

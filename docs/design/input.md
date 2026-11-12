# Input Design

Based on [input research](../research/input.md). Designed after Rune and GPUI, taking Rune where they disagree; aimed at desktop and mobile, not the web.
Confirmed: single target + capture; commands + keymap data; the runtime owns text editing state, while the app remains controlled via `value` + `on_input`.

## Layers

```
Platform shell (winit / UIKit / Android)
  └─ Raw input: PointerEvent, Wheel, KeyEvent, text and input method (IME), accessibility actions, time
       └─ Runtime: hit testing, capture, gesture recognition, focus, keymap, text editing
            └─ Element handlers: on_press, on_drag, on_key, on_command, on_input …
```

Tests and agents call the same raw input (or convert from a test id to the element center), without bypassing the recognizers.

## Raw input and time

- `PointerEvent { id, kind: Mouse | Touch | Pen, phase: Down | Move | Up | Cancel, x, y, button, modifiers }`.
  `button`: `Primary | Secondary | Middle`; `modifiers`: `SHIFT | CTRL | ALT | META`.
- Wheel: `wheel(x, y, dx, dy)`.
- Time is a virtual clock: the shell calls `advance_to(t)` every frame, tests call `advance(ms)`. Long press, multi-click and inertia look only at this
  clock, so results are deterministic. `next_deadline()` tells the shell when it next needs to wake (long press expiry, next inertia frame).
- Input modality (Rune's modality): whether the most recent input was pointer / touch or keyboard.

## Pointer

**Target**: the topmost node at the hit point, then upward to the first ancestor that handles pointers (has a press / double / secondary / long press /
drag handler, or is focusable). Single target, no bubbling (Rune). When a modal is open, elements outside the modal do not take part in hit testing.

**Capture**: after press, this pointer's moves and release go to the pressed target until `Up` or `Cancel` (Rune `Press::Held`, GPUI
`capture_pointer`).

**Press (Rune semantics)**:

- Down: the target gets `PRESSED`; if focusable it gets focus (no focus ring shown), otherwise focus leaves.
- Moving out of the target: `PRESSED` is removed; moving back restores it. Releasing outside the target does not fire.
- Scrolling while held (wheel, touch pan): the press is cancelled. A target with `on_drag` keeps capture.
- `Cancel` (system cancels the touch, window loses focus): the press is cancelled, and the drag receives `Cancel`.
- Release inside the target: `on_press`; a second one on the same element within the double-press interval (default 500 ms) and distance (mouse 4 pt,
  touch 16 pt): additionally `on_double_press` (`on_press` fires every time, same as Rune).
- Secondary (right click, macOS Ctrl+click): the nearest `on_secondary_press` ancestor; on the keyboard, the Menu key and Shift+F10 do the same for the
  focused element, and so does a touch long press (see Touch). Three sources, one handler (Rune, Exact).

**Element behavior**: the recognizer knows no element types. Built-in elements (text field, later multi-line text and sliders) receive gestures through the
`Behavior` interface: press (with this click sequence number), whether it handles drags for this pointer itself, and drag positions. The text field uses
these to place the caret, select words, select all, and drag-select. The click sequence number is counted at press time (AppKit, Rune), and a drag ends a
click sequence.

**Drag**: `on_drag(|d: &Drag|)`, `Drag { phase: Start | Move | End | Cancel, x, y, dx, dy }`, where `x, y` are coordinates within the element and
`dx, dy` is the displacement since press. It starts when the mouse moves more than 4 pt (touch 8 pt); once started, `on_press` no longer fires.

**Hover**: only mouse and pen hover; touch does not hover (GPUI). The element under the pointer and all its ancestors hover (GPUI, Rune, CSS), up to the
innermost ancestor that wrote `hover_alone` (Rune); `group(name)` names a hover group, and descendants use `group_hover(name, ..)` to change with it (GPUI).
After a layout change it is recomputed from the last position; while a layout animation is running and the pointer is still, hover stays unchanged, and
catches up after the animation ends or the pointer moves (Rune).

## Touch (recognizers in the core, GPUI parameters)

| Parameter | Value |
| --- | --- |
| slop (tap / pan boundary) | 8 pt |
| Long press | 500 ms without leaving slop |
| Multi-tap | 400 ms, 16 pt |
| Inertia | iOS exponential decay, 0.998 per millisecond; velocity taken over the last 100 ms |

- Down is `PRESSED` immediately (Rune; GPUI synthesizes it only on release, with no pressed state).
- Past slop: if the target has `on_drag`, drag; otherwise the nearest scroll container pans and the press is cancelled; on release it enters inertia
  with the velocity.
- Long press expiry: `on_long_press`; if absent, `on_secondary_press` (the mobile context menu); after it fires, release no longer triggers `on_press`.
- Only the first finger is tracked; multi-finger gestures such as pinch come later.

## Focus

- States: `FOCUSED` (has focus) and `FOCUS_VISIBLE` (has focus and the most recent input was the keyboard). Style `.focus(..)` corresponds to `FOCUSED`,
  `.focus_visible(..)` to `FOCUS_VISIBLE` (GPUI's two names). Pointer focus does not show the focus ring; keyboard actions such as Tab afterwards do.
- Tab / Shift+Tab are built in and cycle in tree order; `tab_index(n)`: positive values come first, negative values stay out of the Tab order (Rune).
- `focus_group()`: arrow keys move among focusable descendants within the group, and Tab stops only at the current item in the group (Rune).
- `autofocus()`: focus it when a modal opens; otherwise focus the first text field in the modal, otherwise the first Tab position (Rune).
- `modal(click_outside, dismiss)`: while mounted, the background does not respond and Tab cycles only within the modal; Escape (and, when allowed, a click
  outside the modal) calls `dismiss`; after unmount, focus returns to the element from before it opened. With multiple modals only the topmost takes
  effect (Rune's modal stack).
- With no focus, Escape does nothing; with focus and not inside a modal, Escape moves focus away.
- `on_focus` / `on_blur` (GPUI's names): an element hears that it got or lost keyboard focus, the
  one losing it first. Focus moving or being cleared blurs; the window going inactive does not
  (the element keeps its focus), nor does the element leaving the tree. A field edited in place
  (a rename) commits on blur.

## Tooltips

- `.tooltip(text)` (GPUI's name): the window's one tooltip, for the innermost hovered element
  that has one. It shows once the mouse has rested 500 ms (GPUI), at once within 300 ms of
  another going (moving along a toolbar); it goes when hover leaves, and on a press, a scroll, a
  key or the window going inactive, then not again until hover leaves the element. The window
  draws it in its layer of requests, placed by the element (below, above where there is no room),
  role `Tooltip`, test id `tooltip`; its timing is the window's pointer deadline, so it is
  recorded, replayed and run by `advance`. Its text is the element's description for assistive
  technology unless it repeats the label. `Window::tooltip()` gives the text showing.
- Touch has no hover, so no tooltips; a native tooltip (AppKit's) is a later step.

## Keyboard: keymap and commands

- `KeyEvent { key, modifiers, repeat }`; `key` is a named key (`Tab, Enter, Escape, Backspace, Delete, Left, Right, Up, Down,
  Home, End, PageUp, PageDown, Space, F(n), Menu`) or a character. Text goes separately through `type_text` / the input method (IME) (text from the
  platform has already been processed for keyboard layout and dead keys).
- Chords are data, written as in Rune: `"mod+shift+t"`, where `mod` is Cmd on Apple and Ctrl elsewhere. `KeyEvent` also carries the physical key
  (`code`); a chord with modifiers is matched by physical key when the logical key is not a Latin letter, so shortcuts still work after changing keyboard
  layout (Exact).
- The keymap is data: `Binding { chord, command, context }`. `context` is a `key_context("editor")` name that
  must appear on the focus path; a deeper context takes priority, and at the same depth the later one added wins (GPUI). The app can append and
  override; `app.keymap()` lists everything.
- Commands are string ids (`"focus.next"`, `"edit.copy"`), and an agent can directly `app.invoke("edit.copy")`.
- Handling: `.on_command("save", f)` attaches to an element and is looked up along the focus path from inside out; `on_command("save", f)` (a free
  function) is app-level.
- Dispatch order:
  1. During input method (IME) composition, Enter and Escape belong to the input method; Cmd / Ctrl chords first commit the composition and then continue (Exact).
  2. `on_key(|k| -> bool)` on the focus path, from inside out; returning `true` consumes it (Rune).
  3. The keymap resolves a command; look for a handler along the focus path, then built-in commands, then app-level handlers.
  4. An unbound key does nothing; text is delivered separately by the shell.
- Built-in commands: `focus.next / focus.prev` (Tab / Shift+Tab), `focus.dismiss` (Escape), `press` (Enter, Space),
  arrow keys moving within a `focus_group`, and text editing commands (see below).

## Text editing

- The app still writes only `text_input().value(..).on_input(|v| ..)`; Enter to submit is a separate `on_submit` (Rune), and `on_press` is synonymous for
  text fields and other elements (click, Enter / Space, accessibility activation). Keys: in the context `text_input`, Enter → `submit`; with no
  `on_submit`, `submit` is not handled and Enter falls through to `press`. Selection, composition and undo records are kept by the runtime per node, and
  carried over by test id on hot reload.
- Each edit computes a new value and calls `on_input`; if the app does not adopt it (the value is unchanged), the selection is clamped to the new value's
  range. A different value written by the app during composition takes effect after the composition ends (Exact).
- Caret movement is by grapheme cluster, and word movement by Unicode word boundary (`unicode-segmentation`).
- The model is the runtime's own (value, selection byte offsets, undo stack), not Parley's `PlainEditor`: the latter brings its own layout and font
  context, bypassing the kernel's `TextEngine` and paragraph cache. Caret geometry comes from `TextEngine::caret_stops` (Parley by cluster, the
  monospace engine used in tests by character).
- Pointer: click places the caret, Shift+click extends, double click selects a word (taking the character under the pointer), triple click selects all,
  mouse drag selects (touch drag is still scrolling).
- A long line scrolls horizontally inside the box, keeping the caret visible with as little scrolling as possible.
- Editing commands (context `text_input`): left and right, Home / End, extending the selection with Shift, move and delete by word (Alt on Apple, Ctrl
  elsewhere), Backspace / Delete, select all, copy / cut / paste, undo / redo.
- Undo: snapshots (value, selection), consecutive typing merged into one step, at most 128 steps (Rune).
- Clipboard: the [platform](platform.md)'s (`Platform::read_text` / `write_text`); headless it is in memory, and the shell's backend uses the system clipboard.
- Input method (IME) composition is displayed at the caret with an underline, does not enter the value, and enters only after commit.
- Platform protocol (the shape of GPUI's `InputHandler`): text, selection, composition range, replace by range, set composition, set selection, caret
  rectangle; internally byte offsets are used, with a UTF-16 conversion provided for NSTextInputClient, UITextInput and TSF. The winit shell uses only the
  composition and commit parts.
- Single-line first; the multi-line text field comes later.

## Scrolling

- The wheel goes to the innermost scroll container for that axis at the hit point; at the edge it is not passed to the outer one (Rune).
- Touch pan likewise finds the innermost container; after release, inertia (see Touch).
- Scrolling while held cancels the press.
- A container following its tail lets go of it when scrolled away from the end and follows again
  at the end ([layout](layout.md)).

## Accessibility actions

`Click` → `on_press`, `Focus` → focus, `SetValue` → `on_input`, `ShowContextMenu` → `on_secondary_press`,
`ScrollIntoView` → scroll into view.

## Tests and agents

- By test id: `tap(id)`, `double_tap(id)`, `long_press(id)`, `drag(id, dx, dy)`, `secondary_press(id)`,
  all converted into a raw pointer sequence at the element center and passed through the recognizers. If the target is not in the test ids, it is found by
  accessibility name, and duplicate names are rejected; if the center is covered by another element or outside the window, an error is returned saying
  what blocks it (Exact).
- `press_key("mod+s")`, `type_text("héllo")`, `event(InputEvent::Composition("ni".into()))`, `invoke("edit.copy")`, `advance(ms)`.
- The coordinate versions (`pointer(..)`, `click_at(x, y)`, `wheel(..)`) remain available; they are exactly the entry points the shell calls.

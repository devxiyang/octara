# Text input: multi-line and secure fields

Based on the [research](../research/text-input.md). Status: proposed, awaiting confirmation.

One constructor, `text_input()`, with modifiers (Rune's names); the model stays `value` +
`on_input`, and the editing model stays the runtime's own (not Parley's `PlainEditor`,
[input](input.md)).

## Multi-line

- `.multiline()`: the text wraps at the field's width and Enter inserts a line break.
  `on_submit` fires on Cmd+Enter (Ctrl+Enter elsewhere). `.submit_on_enter()` turns it into a
  chat composer: Enter submits, Shift+Enter (and Option+Enter) inserts a line break.
- `.rows(min, max)`: the height follows the text between `min` and `max` lines; past `max` the
  field scrolls inside, keeping the caret in view. Without `rows` it grows with its text, and an
  enclosing scroll container keeps the caret in view.
- Motion: Up / Down move by visual line keeping a goal x (any other move, edit or click forgets
  it); at the first or last line they go to the start or end of the text. PageUp / PageDown move
  by the visible lines. Home / End go to the visual line's edges; Ctrl+Home / End (Cmd+Up /
  Down on macOS) to the text's. Each has a `select_*` twin and a keymap command. Paste keeps line
  breaks (a single-line field turns them into spaces, as now).
- Layout comes from the text engine's cached paragraphs (one paragraph per line break, broken at
  the content width), through new queries next to `caret_stops`: the caret's rectangle, the byte
  under a point, a visual line's range, selection rectangles per line fragment, and a move by
  lines with a goal x (Parley's `Cursor` / `Selection` over the cached layouts). Typing reshapes
  one paragraph. Undo records range edits instead of whole-value snapshots.
- Painting: one selection rectangle per line fragment, the caret at its real line, the
  composition underlined per fragment; the input method's candidate window follows the
  composition's rectangle.
- Accessibility: `Role::MultilineTextInput`.

## Secure

- `.secure()`: the value is shown as one `•` per grapheme. Copy, cut and drag do nothing,
  there is no undo history, word motion moves to the ends, and the input method is off while it
  is focused. `.revealed(bool)` shows the text (for a "show password" toggle) and keeps every
  other protection. On touch platforms the last typed character shows for 1.5 s on the app's
  clock. Single-line first; a multi-line secure field shows a fixed mask (Rune) later.
- The macOS shell turns on secure event input while a secure field is focused (balanced on
  blur and exit).
- Nothing leaks it: AccessKit gets `Role::PasswordInput` with a value of bullets (its contract),
  `inspect` and `check` show bullets, `Debug` of the input state prints `[redacted]`, and
  diagnostics never quote it. Tests read the real value only through an explicit test call.
  Value and composition buffers are zeroed when dropped.

## Later

- `.input_action(..)` (the return key: done, go, next, search, send) and `.content_type(..)`
  (username, new password, one-time code, email, number), mapped per platform.
- Direct platform text protocols (NSTextInputClient, UITextInput, TSF, InputConnection,
  text-input-v3), replacing winit's IME events: surrounding text, reconversion, dictation,
  per-range rectangles (the handler shape is in [input](input.md)).

## Tests

- Headless, with the monospace engine wrapping at a column width: goal x across a short line,
  Home / End on a wrapped line, PageDown by visible rows, selection rectangles across lines,
  Enter / Shift+Enter / Cmd+Enter in both modes, `rows` growing and clamping, the caret kept in
  view, a composition on line 2 placing the candidate window there, paste in both modes.
- Secure: the value never appears in snapshot JSON, accessibility, diagnostics or `Debug`;
  copy, cut and undo do nothing; reveal and the last-character timer (with `advance`).
- One real Parley test of wrapped caret rectangles.

## Steps

1. Text engine queries and the monospace wrap; multi-line layout, painting and motion.
2. `rows`, scrolling inside and into view; `submit_on_enter`; range-edit undo.
3. `secure`, `revealed`, redaction everywhere, the macOS secure input; mobile reveal.

# Real macOS keyboard and Chinese IME validation

Use this fixture to validate the path from OS keyboard events through the system input
method to Octara's text state. Inspector commands read the result. They do not supply text or
composition events for this check. Headless `Composition` events remain useful for regression
tests, but do not establish that a real input method or candidate window works.

## Prepare the window

```sh
cargo build -p octara-examples --bin ime -p octara-cli --bin octara
python3 examples/ime_window.py
```

The second command only prepares a temporary `.app` and prints a manifest. Launch its `bundle`
with the computer-use app launcher. `LSEnvironment` supplies the manifest's `socket` path to the
normal winit shell. The bundle contains one copied executable so it can be opened as an app;
only remove the printed output directory after its own window has exited.
Apple documents `LSEnvironment` as applying to
[Launch Services launches](https://developer.apple.com/library/archive/documentation/General/Reference/InfoPlistKeyReference/Articles/LaunchServicesKeys.html);
directly executing the copied binary requires supplying its environment separately.

The fixture has three fields and committed-value counters:

| Test id | Initial content | Purpose |
| --- | --- | --- |
| `ime-single` | `A😀Z` | Single-line composition, Unicode caret and selection |
| `ime-multi` | `alpha` and `beta` on separate lines | Multi-line editing and candidate anchoring |
| `ime-password` | Empty | Secure-field redaction and input-method exclusion; use test text only |
| `ime-blur` | A button | Moving focus away from a text field |

`ime-single-value` and `ime-multi-value` show the bound, committed strings. Their `*-edits`
labels count committed value changes. Preedit updates, caret motion and selection must leave
these counters unchanged. `ime-submits` counts submission callbacks. Password content is never
echoed in a label.

## Record the actual input route

Before starting, record the active input source and foreground app so both can be restored.
This workstation has enabled ABC (`com.apple.keylayout.ABC`) and Pinyin – Simplified
(`com.apple.inputmethod.SCIM.ITABC`). Check the actual source after switching; a shortcut alone
does not prove the switch succeeded. No input-source installation or global preference change
is required. Keep control of keyboard/mouse actions in computer use throughout the check.

Send individual Latin keys through the OS keyboard path while Pinyin is selected. Unicode text
insertion, clipboard paste, Inspector `type`/`key`, and synthetic framework `Composition` are
different routes and must not be reported as OS IME validation.

Read the current state using the existing Inspector connection:

```sh
target/debug/octara run --connect /absolute/path/from/manifest/ime.sock --json
```

Send `{"op":"inspect"}`. `focus` identifies the field observed in the same snapshot.
For a focused nonsecure field, `text_input` reports:

- `text`: the committed text.
- `selection` and `selection_utf16`: the selected range in committed text, measured in UTF-8
  bytes and UTF-16 code units. `reversed` records selection direction.
- `composition`: the IME's separate preedit string.
- `composition_selection` and `composition_selection_utf16`: ranges within preedit; `null`
  means the IME hides its composition cursor.
- `caret`: the candidate/caret anchor rectangle in logical window coordinates, and
  `caret_visible`: whether the caret is currently shown.

These are observations of the focused text owner, not commands that overwrite its state.
The entire `text_input` object is omitted for a secure field.

`ime_observe.py` records one read-only snapshot with optional checks after each OS action:

```sh
python3 examples/ime_observe.py --socket /absolute/path/from/manifest/ime.sock \
  --output /tmp/ime-evidence --name 01-selection --input-source com.apple.keylayout.ABC \
  --expect '{"focus":"ime-single","text_input":{"selection":[1,5],"selection_utf16":[1,3]}}'
```

It saves the unmodified snapshot as `01-selection.json` and input-source/live metadata as
`01-selection.meta.json`. The input source is sampled before and after the Inspector read.
Object expectations match recursively; arrays and scalar values match exactly, including
explicit `null`. `--absent text_input --expect '{"focus":"ime-password"}'` checks that secure
context is genuinely omitted. Each evidence name must be new; failed checks still save the
actual snapshot and return a nonzero exit code. This observer never sends an input or action.

The reply's `live.ime` metadata also reports whether the window allows IME, native focus, and
the native input context's `context_source`. Check that context alongside the TIS source samples:
TIS reports the system source, while the native context identifies the input method serving this
window. `live.window` reports visibility, occlusion, frame count and the age of the last presented
frame; use those facts when checking that a screenshot reflects current state. Marked input can
consume a key without a separate keyboard event reaching Octara, so keyboard-event metadata alone
does not establish the input route.

## Recorded native Pinyin observations

The 2026-11-14 validation recorded snapshots and capture metadata in
`/private/tmp/octara-ime-20261114-context/evidence`. These files document that local run; rerun the
acceptance sequence after changes. Input used computer-use OS key events, not a hardware keyboard.
Both TIS and the native input context identified ITABC during Pinyin input. The original source,
ABC, was restored and the fixture was closed at the end. Temporary app bundles and sockets were
removed after their processes exited; the JSON and PNG evidence was retained locally.

Early observations `01`–`15` established preedit, committed-value counters, emoji selection,
multi-line editing and secure-context omission. One useful distinction appeared in `04`: Space
produced marked `你好ao`, with preedit cursor UTF-8 `[6,6]` / UTF-16 `[2,2]`, without committing.
Escape then cleared it with zero edits; `06` separately established a final Space commitment.
In `12`–`14`, normal Return added a multi-line newline, Command+Return submitted once, and a
subsequent IME Return committed literal `ni` without another newline or submission. That last
submit baseline was observed through computer use; no intermediate JSON snapshot was saved.

The real-window run also exposed a bug that core-only checks missed. In `20`, field A had marked
`ni`; switching to field B cleared Octara's preedit in `21`, yet typing `nihao` and committing
produced `alpha\nbeta你你好` in `22`. winit had cleared its own marked storage while the native
input context retained pending Pinyin syllables. The host now calls the native input context's
`discardMarkedText` after disabling winit IME when retiring the old text owner. Repeating the
same field switch in `23`–`24` produced exactly `alpha\nbeta你好`, with one committed edit.

The final run used a fresh fixture. Edit counts below are ordered single-line / multi-line /
password; submits remained zero throughout these final observations.

| Evidence name | Observed state | Edit counts |
| --- | --- | --- |
| `23-fixed-owner-preedit` | Field A had marked `ni` and committed `A😀Z` | `0 / 0 / 0` |
| `24-fixed-owner-commit` | After switching to field B, `nihao` + Space committed exactly `alpha\nbeta你好`; no old syllable survived | `0 / 1 / 0` |
| `25-final-selection` | Emoji selected in `A😀Z`: UTF-8 `[1,5]`, UTF-16 `[1,3]`; blue selection was visible | `0 / 1 / 0` |
| `26-final-preedit-cursor` | Preedit `ni hao` replaced the selection visually; Left moved its relative cursor to `[2,2]`, while committed text and selection stayed unchanged | `0 / 1 / 0` |
| `27-final-cancel` | Escape cleared preedit and restored the original emoji selection without a committed edit | `0 / 1 / 0` |
| `28-final-numbered-candidate` | Number `2` committed `👋`, producing `A👋Z`; composition cleared, cursor UTF-8 `[5,5]` / UTF-16 `[3,3]`, and the new text was visibly painted | `1 / 1 / 0` |
| `29-undo-source-check` | Command+Z restored `A😀Z` and selection `[1,5]` / `[1,3]`; TIS and native context confirmed ABC | `2 / 1 / 0` |
| `30-final-secure` | Focus `ime-password`, redacted value `•••`, `text_input` absent, native `secure: true` and `allowed: false` | `2 / 1 / 3` |
| `31-final-blur` | Focus `ime-blur`, `text_input` absent, native `secure: false` and `allowed: false` | `2 / 1 / 3` |

Fresh visible-window painting was verified after the earlier stale-image observations.
`24-fixed-owner-display.meta.json` and `26-preedit-display.meta.json` both report
`pixels: "display"` and `renderer: "display"`, with settled captures of the actual window;
the images show the corrected committed result and the internal preedit cursor. Computer-use
screenshots also verified the blue emoji selection, cancellation, and candidate replacement in
`25`–`28`. Their window metadata reported visible, active and unoccluded, with advancing frames.

Keep the remaining visual limits explicit: `29`–`31` reported an occluded window with pending
presentation, so the secure-field and blur results establish state and redaction, without a
fresh visual claim for those steps. Candidate selection behavior passed, but the external OS
candidate popup's entries and position were not captured and verified. Application display
captures include Octara's content; the OS-owned candidate window is outside that capture and
outside the Inspector tree.

## Acceptance sequence

1. With ABC selected, OS-click the single-line field. Move to the end and then Left once.
   The `A😀Z` seed has a UTF-8 caret offset of 5 and a UTF-16 offset of 3 after the emoji.
   Shift+Left selects the emoji: ordered UTF-8 range `[1,5]`, UTF-16 range `[1,3]`.
   The bound value and edit counter stay unchanged. Exercise selection replacement, Backspace
   and Undo, checking the visible result and both range units.
2. Preserve the `A😀Z` seed, move to its end, then select Pinyin and send OS keys `n`, `i`,
   `h`, `a`, `o`. Record the visible marked text and system candidate window. During composition
   the committed `text`, value label and edit count remain at their baseline. Move within the
   marked text with Left and record the IME's actual preedit and relative selected range instead
   of assuming a particular spelling. For a separate replacement case, begin with a committed
   selection and verify that preedit leaves the selected committed text unchanged until commit.
3. Select a visible Chinese candidate with Space or its number key. The IME may convert only a
   segment and keep the result marked; this must not increment the committed edit count. Record
   this separately from final commitment. Finish the composition, then compare committed text
   with the final marked text and chosen candidate: the bound value changes once and composition
   clears. Repeat using Return: the Return which commits composition must not also increment
   `ime-submits`; a subsequent Return in the single-line field does submit.
4. Start another composition and cancel it with Escape. The original committed string and
   selection return, with no extra committed edit. Start once more, click `ime-blur`, then
   focus the multi-line field; no old marked state may appear in the newly focused field.
5. In the multi-line field, use OS arrow keys and Shift selection across lines, compose and
   choose a candidate, then press Return to insert a line break. Command+Return submits after
   composition has ended. Check that the candidate window stays near the active caret after
   moving the caret and resizing the window.
6. With ABC selected, OS-click the password field and enter disposable ASCII test text. The
   Inspector must expose no password text or preedit, and must omit `text_input`. Check the
   actual system input-method behavior separately; do not type a real password in this fixture.

Use an OS screenshot to document the candidate window: it is owned by the input method and is
not part of an Octara GPU-painted capture. GPU captures can separately document the framework's
marked text, caret and selection. Record the selected source, OS key sequence, Inspector snapshots,
and OS screenshots together. Restore the original input source and foreground app when finished.

## Current native text-client boundary

The locked winit 0.30.13 macOS implementation (`src/platform_impl/macos/view.rs`) forwards marked
text and converts the IME's selected preedit range from UTF-16 to UTF-8. It uses the rectangle
supplied by Octara for `firstRectForCharacterRange:actualRange:`. This supports the composition
and candidate-anchor path tested here, but it does not expose a complete document text client:

- `selectedRange` always returns `NSNotFound`; it does not return Octara's committed selection.
- `attributedSubstringForProposedRange:actualRange:` returns no surrounding text.
- `setMarkedText:selectedRange:replacementRange:` and `insertText:replacementRange:` ignore the
  native `replacementRange`. Octara applies a normal commit to its own current selection.

Inspector's committed text and selection provide context to the agent; they do not automatically
answer these native text-client queries. Native reconversion of an existing document selection,
arbitrary replacement ranges requested by an input method, and surrounding-text services remain
outside the guarantees of this validation. The OS owns the candidate list and candidate window;
the Inspector reports Octara's resulting preedit and committed state, not those external entries.

The setup script is deliberately preparation-only: it neither launches the app nor changes an
input source, sends events, grants permissions, or changes user preferences.

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
- `composition`: the currently marked substring, separate from the committed value.
- `composition_selection` and `composition_selection_utf16`: ranges within preedit; `null`
  means the IME hides its composition cursor.
- `display_text`: the whole displayed document, including the uncommitted draft. A partial
  native replacement can leave draft text outside the currently marked substring.
- `display_selection` / `display_selection_utf16` and `marked` / `marked_utf16`: the displayed
  selection and marked range, measured from the start of `display_text`. `null` means no
  displayed selection or no marked range, respectively.
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

`live.ime` additionally exposes native query counts, the native selection/marked ranges,
`screen_anchor`, `first_rect_failure`, `geometry_status`, `pending`, `inflight`, and
`last_interpret_inline`. Snapshot ranges are `[start,end]`; native `*_range_utf16` metadata
and `first_rect_request_utf16` are AppKit `[location,length]` pairs. `screen_anchor` uses AppKit
screen coordinates, while `text_input.caret` uses logical window coordinates.

## Earlier winit-based Pinyin validation

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
input context retained pending Pinyin syllables. That version was fixed by calling the native
input context's `discardMarkedText` after disabling winit IME when retiring the old text owner.
Repeating the same field switch in `23`–`24` produced exactly `alpha\nbeta你好`, with one committed edit.

That earlier run ended with a fresh fixture. Edit counts below are ordered single-line / multi-line /
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
outside the Inspector tree. This section records the earlier implementation; the owned native
client and its additional candidate-window evidence follow below.

## Owned-client candidate observation

The next run records evidence in `/private/tmp/octara-reconversion-20261114/evidence`.
After the user brought the fixture to the foreground, `15-synchronous-anchor.json` shows native
Pinyin preedit `ni hao` in `ime-multi`, with committed text still `alpha\nbeta`. The complete displayed document is
`alpha\nbetani hao`, its marked range is `[10,16]`, and its displayed selection is `[16,16]`.
The paired metadata reports ITABC before/after, a current native input context, a key, active,
visible and unoccluded window, and `last_interpret_inline: true`. Actual native queries include
selection, marked range, substring and five first-rectangle requests. The last rectangle request
is UTF-16 `[15,1]`, with no failure and a nonzero AppKit screen anchor of approximately
`[424.38,575.5,7.28,18]`.

`18-client-candidate/report.json` identifies a separate candidate panel, window `28062`, owned
by the **client app PID** `31513`, despite its system InputMethodKit content. Its WindowServer
bounds are `[381,386,397,28]` (top-left screen coordinates); its captured image is 794×56 at
the display's 2× scale. `18-client-candidate/window-28062.png` visibly contains candidates
`你好`, `👋`, `你好你好`, `你是`, `你`, and `你哈`, with the first highlighted. This establishes actual
candidate contents and a recorded panel position for this observation. The report's AX traversal
was incomplete, so the content evidence is the window PNG, not a complete AX candidate list.

The same run then validated expanded candidates and native reconversion. In the following table,
S and M are the single-line and multi-line committed edit counts. Submits remained zero.

| Evidence | OS action and observed result | Counts |
| --- | --- | --- |
| `20-expanded-client` / `20-expanded-state` | The actual panel expanded to 397×182 points with additional rows and sort tabs; preedit stayed `ni hao`, committed multi-line value `alpha\nbeta` | S0 / M0 |
| `21-numbered-commit` | CUA number `2` selected `👋`, committing exactly `alpha\nbeta👋`; marking cleared | S0 / M1 |
| `22-undo-abc` | CUA Undo restored multi-line `alpha\nbeta`; the subsequent snapshot focused the single-line field under ABC | S0 / M2 |
| `23-reconversion-selection` | After entering `nihao` under ABC, committed `A😀nihaoZ` had selection UTF-8 `[5,10]` / UTF-16 `[3,8]` | S5 / M2 |
| `24-reconversion-candidate` / `24-reconversion-marked` | The user chose the system Input menu's “Show Substitution Candidates”; actual candidates `你好`, `👋`, `你是` appeared without changing text, selection range or edit count | S5 / M2 |
| `25-reconversion-commit` | CUA number `2` replaced only the selected `nihao`, producing `A😀👋Z`; caret UTF-8 `[9,9]` / UTF-16 `[5,5]` | S6 / M2 |
| `26-reconversion-undo` | CUA Command+Z restored `A😀nihaoZ` and selection `[5,10]` / `[3,8]` | S7 / M2 |
| `27-zoomed-candidate` / `27-zoomed-state` | Native Window > Zoom changed the main window bounds and the new Pinyin candidate panel appeared at the corresponding new screen location | S7 / M2 |
| `28-marked-caret-candidate` / `28-marked-caret` | CUA Left moved the preedit-relative cursor to `[2,2]`; the OS retained the candidate panel's position | S7 / M2 |
| `29-cancel-restore` | CUA Escape restored `A😀nihaoZ` and the backward original selection `[5,10]` / `[3,8]` | S7 / M2 |
| `30-ime-return` | CUA `n`, `i`, Return committed literal `ni` into the multi-line field: `alpha\nbetani`, with no added newline or submit | S7 / M3 |
| `31-owner-preedit` / `32-owner-clean-commit` | Single-line preedit `ni` changed no bound value; after switching fields, `nihao` + Space committed only `你好`, giving `alpha\nbetani你好` without the old owner's syllable | S7 / M4 |
| `33-secure-native` | Ordinary Return added one newline, Command+Return submitted once, then three disposable ASCII keys in the secure field showed only `•••`; `text_input` was absent | S7 / M5 / P3; submits 1 |
| `34-restored-abc` | Focus moved to the blur button; native secure input was off, no text context was published, and both source samples confirmed the original ABC source | S7 / M5 / P3; submits 1 |

`21-numbered-commit.meta.json` contains one incorrect test expectation: it expected JSON `null`
for `composition`, whose cleared representation is the empty string. Its actual snapshot and
value label show the correct committed result, and `22` confirms Undo restored it. This assertion
error is retained in the evidence rather than counted as a passing observer check.

The reconversion popup is directly visible in `24-reconversion-candidate/window-28155.png` at
WindowServer bounds `[378,245,157,28]`. Opening it used the already committed selection; nobody
retyped `nihao` to simulate reconversion. Native `set_marked` stayed at 15 between `23` and `24`,
while selection queries rose from 15 to 28, substring queries from 29 to 43, and first-rectangle
queries from 16 to 20. The menu therefore exercised native document queries without requiring a
new preedit callback. The subsequent numeric choice exercised native insertion over that range.

After Zoom, `27-zoomed-candidate/report.json` records main window `[0,33,1470,870]` and candidate
window `28169` at `[43,215,397,28]`. The paired state reports AppKit anchor
`[86.697265625,747,7.2802734375,18]`, `first_rect_failure: null`, and an active, key, unoccluded
window. On Left in `28`, preedit selection and the displayed caret changed, but the IME made no
new first-rectangle request and kept that panel location. This records the native IME's observed
anchoring policy; it does not require the popup to move on every internal caret step.

In `33`, computer use visibly confirmed the newline, one submit and the secure field's mask.
The native metadata reports `secure: true`, `allowed: false`, no current text context, and null
source, selection, marked range, geometry and screen anchor. In `34`, moving focus away also
cleared secure mode. After confirming ABC, the fixture was quit through computer use and its
process exit was checked. Only the stopped temporary app bundles and socket were removed;
JSON and PNG evidence was retained. The complete workspace run passed 411 tests; final core
and shell unit checks, Clippy with warnings denied, and formatting checks also passed.

WindowServer and AppKit use different screen-coordinate origins; convert them before comparing
panel bounds with `screen_anchor`. The user performed the system Input-menu click for `24`;
key input, numeric candidate selection, caret motion and Undo used computer use. The CG/AX
observer only read state and captured the specified PID's windows. The candidate list remains
outside Octara's Inspector tree. A main-window-only capture also excludes this separate panel
even though WindowServer assigns it to the client PID. The native reports' incomplete AX trees
do not invalidate the individually captured panel PNGs or establish a complete AX candidate list.

When native CG/AX observation is authorized, the read-only observer can capture that panel:

```sh
swift examples/ime_candidates.swift --pid 31513 --output /tmp/ime-candidates-new
```

Replace `31513` with the current Inspector reply's `live.pid` and choose a new output directory.
Do not assume the candidate panel belongs to the SCIM or IMK service PID: this run found its
visible panel under the client app. The helper filters ownership to the explicit PID, reads only
its AX windows, and captures eligible on-screen windows only. It never activates an app, sends
input, or requests permissions. Missing existing AX/Screen Recording access is reported; an
incomplete AX observation or per-window capture error can return nonzero while retaining useful
PNG evidence. Inspect `report.json` and the individual image before drawing a conclusion.

Add `--include-offscreen-metadata` to include this PID's offscreen, zero-sized or transparent
windows in `cg_windows`; it still captures no offscreen windows. `target_window_count` and
`eligible_window_count` distinguish discovered metadata from capturable windows. With no AX
windows the helper records only application role/title and does not traverse application menus.
Output is bounded, existing paths are never overwritten, and no other PID's content is emitted.

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

The macOS shell now attaches its own non-hittable `NSView` implementing `NSTextInputClient`
(`crates/octara-shell/src/text_input_mac.rs`). winit translates framework keyboard events with
its own IME path disabled. The owned client interprets the native event within `keyDown`, serves
native queries synchronously, and queues edits for the shell's safe application boundary.

Its owned `TextDocument` keeps committed text/selection alongside a complete displayed draft,
a marked subrange and a displayed selection. `selectedRange`, `markedRange` and attributed
substring queries refer to that displayed document. Explicit replacement ranges from both
marked and committed native edits are document-global UTF-16 ranges, converted to UTF-8 byte
ranges in the core; the marked-text `selectedRange` parameter is relative to the new string.
This follows the locally checked stock-`NSTextView` behavior, including an explicit replacement
away from an existing mark. See [native IME research](../docs/research/native-ime.md) for the
Apple documentation ambiguity and the reference probe; the bridge does not guess coordinates.

Marked edits change only the draft. An insertion or `unmarkText` accepts the resulting displayed
document as a committed edit; cancellation restores the committed value and original selection.
Queued edits carry their expected document and owner, so a handler's newer edit or a focus change
can reject stale work. Retirement also discards the native composer's state. Secure fields
publish neither the document nor its geometry.

`TextGeometry` supplies range-sensitive first-line rectangles and displayed-text hit testing.
The native bridge converts these logical window rectangles to AppKit screen coordinates and
returns the adjusted `actualRange`. A native edit can precede the next application frame, so
the bridge also holds a frozen `TextInputLayout` recipe. When its geometry describes an older
document or selection, it synchronously lays out the current native document with the same
fonts, wrapping and caret scrolling, without entering window or application handlers. It keeps
that fresh geometry across intermediate older frames; reconciliation invalidates native
character coordinates and notifies a changed selection when the batch has settled. Missing
layout, a busy text engine or an invalid query is reported in `first_rect_failure`; stale
geometry is not silently returned.

These are implemented protocol paths, not blanket acceptance of every IME service. The evidence
above establishes native preedit and query traffic, visible collapsed/expanded candidates,
numbered selection, committed-Latin-selection reconversion through the system menu, Undo, Zoom
anchoring, internal preedit caret motion, cancellation, IME Return consumption, clean owner
retirement, secure-context omission, and ordinary Return / Command+Return behavior.
Other input methods and surrounding-text services require their own real-window validation.

The setup script is deliberately preparation-only: it neither launches the app nor changes an
input source, sends events, grants permissions, or changes user preferences.

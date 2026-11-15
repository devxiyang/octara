# Popovers and searchable choices

The `showcase choices` scene exercises public `octara-kit` components, their core primitives,
and the normal desktop shell. It has a city Combobox, a Popover containing an editable note,
and a second Combobox inside a dialog. Both city pickers share the selected option ID.

```sh
OCTARA_INSPECT=/tmp/octara-choices.sock cargo run --release -p octara-examples --bin window -- showcase choices
```

Use an unused socket path. The [Inspector](README.md#inspect-a-real-window) can read the same
window and capture it with `pixels: "display"`. Use OS keyboard and pointer input for native
input checks; Inspector `key` and `type` exercise a different route.

## Component contract

- `popover(id, open, placement, trigger, content)` takes a controlled `Model<bool>` and an
  ordinary `El` trigger. Its anchored surface is modal: focus stays inside, Escape and outside
  presses dismiss, and closing restores the previous focus. A pointer or keyboard opener
  normally becomes that focus; an accessibility press does not necessarily focus the opener.
- `combobox(id, label, value, options)` stores an `Option<String>` stable option ID in the
  caller's model. The tracked options closure returns `Vec<ComboboxOption>`; labels, order,
  availability and membership can change while the search field remains mounted.
- Search matches label substrings without case sensitivity. Up/Down skip disabled rows and
  wrap; Enter chooses the highlighted result. IME composition keeps its native key handling.
  `active_descendant` identifies the highlight while `selected` identifies the model value.
- Closing and reopening clears the query. Hot reload preserves named query/open state and
  restores the modal opener chain against new allocations. Removing a selected option leaves
  the caller's value unchanged. Removing the picker removes its popup and native text owner.
- Hot reload currently resets the editor's caret/selection, composition and undo history.
  This existing core limitation is separate from retaining query state and the modal focus
  chain; live option updates keep the editor mounted and preserve its selection.
- The list is retained and intended for finite menus, including hundreds of choices. This
  component does not provide virtualized results, asynchronous search or fuzzy ranking.

IDs and Showcase commands are listed in [README.md](README.md#popovers-and-searchable-choices).
The layering and Rune/Exact source references are in
[UI primitives](../docs/design/ui-primitives.md#searchable-choices-and-popovers).

## Repeatable checks

1. Open `city` with a pointer. With Simplified Pinyin active, type Latin keys individually.
   Verify preedit in `text_input.composition`, unchanged committed `text`, a native Pinyin
   context, and focus on `city-search`. Down during composition belongs to the IME; Escape
   cancels the composition before dismissing the popup. Space commits the system's chosen
   candidate, which can vary with the input method's learned ordering.
2. Search for `杭州`, select the text, and invoke Cmd/Ctrl+Shift+U. The search allocation,
   query and selection remain intact while the matching label becomes `杭州 · 已更新`.
   Search `深圳`; the highlight changes before the selected model changes. Enter chooses it
   and returns focus to the opener.
3. Exercise the native menu ordering boundary without inserting observation delays between
   keys: with query `杭州`, send Cmd+A, Cmd+Shift+U, then `s h e n Return`. Expect `shen`,
   no stray suffix, and an open picker. Then Cmd+A followed by `s h e n z h e n Space`
   should commit the selected native candidate without appending an old Latin suffix.
4. Open `choices-dialog`, then `dialog-city`, and enter a query. Record the popup bounds and
   search target. Send the target-checked Inspector `reload` request. The query, nested
   position and focus remain; the target token changes. Escape closes only the inner popup,
   returning focus to `dialog-city`; another Escape returns to `open-choices-dialog`.
5. With both layers open, request native client size 390×640 using Inspector `size`, then
   `settle`. Confirm actual window, GPU surface and layout dimensions agree, and the popup
   remains next to its current anchor within the window. This is a native-window resize
   request, not an OS pointer drag of the window border.
6. Open `details`, type a Chinese note using the OS input method, and click outside. It closes
   and returns focus to `details`. Open `city` and invoke Cmd/Ctrl+Shift+X: the control, popup,
   focused field and native IME ownership disappear. Invoke it again: a fresh closed picker
   appears with the shared selection intact.

The read-only [IME observer](IME.md#record-the-actual-input-route) records the committed and
displayed strings, UTF-8/UTF-16 selection, marked ranges, current native context and caret
geometry. Keep those JSON facts together with display capture metadata; a screenshot alone
does not establish which input route produced the text.

## 2026-11-15 macOS findings

Local evidence is retained in `/private/tmp/octara-choices-20261115`. OS input was supplied by
computer use, not Inspector text injection. The active source was Simplified Pinyin
(`com.apple.inputmethod.SCIM.ITABC`); no input-source or lock-screen preference was changed.
Phone layouts were tested headlessly; this run does not establish behavior on an iOS device.

Two actual-window failures led to core/shell fixes:

- **Nested placement after reload.** The inner popup moved from `[70, 428, 278, 90]` to
  `[17, 85, 278, 90]` while its trigger stayed at `[70, 392, 286, 32]`. Placement had used
  the outer popup's old layout. Layout now resolves nested anchors with passes bounded by
  the number of anchored surfaces. All three new regressions fail with the old single pass
  and pass with the fix. Captures `15`/`16` preserve the failure; `22`/`23` confirm unchanged
  bounds across reload, and `25-narrow-settled` confirms the 390×640 result.
- **Native menu commands overtaken by IME keys.** Fast Cmd+A, Cmd+Shift+U, `s h e n Return`
  produced `hens`: the first mark appended `s`, then the delayed Select All selected only the
  old prefix. A temporary native trace confirmed the ordering. Menu callbacks now enqueue
  intent immediately; the shell applies it at the next safe input boundary before interpreting
  the next key. Application handlers still run outside Cocoa text callbacks. Existing stale
  owner checks remain in force when a command changes focus.

Native replay of the menu fix passed after unlocking. `27-menu-fixed-shen` records the exact
previously failing sequence producing `shen`; `28-menu-fixed-shenzhen` records the next fast
Select All and Chinese commit producing `深圳`. The trace shows the first `s` mark seeing
`杭州` with UTF-16 selection `[0, 2]`, followed by `h` seeing only marked `s`, with no old suffix.

Temporary tracing was then removed and the normal release binary rebuilt. `29-final-shen`
and `30-final-shenzhen` repeat both checks on that binary. `31`/`32` confirm query, focus and
unchanged nested bounds across reload with fresh target tokens; `33-final-narrow` confirms
the native 390×640 window; `34`/`35` confirm each opener regains focus when its layer closes.
The final window was exited and its temporary app bundle and stale socket removed. JSON,
display captures, failure traces and test logs remain in the local evidence directory.

Other recorded checks: `03`–`06` preedit, native arrow handling and cancellation; `07` an
empty result without accidental selection; `08` a live option update retaining selection;
`12`/`13` highlighted versus chosen value; `14` actual pointer-opener focus restoration;
`17`/`18` nested focus restoration; `19`/`20` unmount/remount; `26` native Chinese input inside
the standalone Popover. System candidate-window contents were not separately sampled in this
run. Early failed snapshots are retained as failure evidence, not counted as passing checks.

## Automated coverage

- Core: active-descendant replacement/hiding, reveal without changing focus, modal opener
  restoration across reload/removal, and simultaneous/nested overlay placement after reload
  and resize.
- Kit: controlled popover behavior, nested dismissal, stable option identity, query and source
  changes, disabled/no results, selection versus highlight, keyboard reveal, IME precedence,
  and mount lifetime.
- Showcase: the actual scene and commands, nested reload geometry/focus, unmount, and
  phone/desktop layouts in light and dark appearance.
- macOS shell: native menu command ordering before text composition, one-time dispatch,
  FIFO order and handler reentrancy without holding the queue borrow.

Run `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`.
The CLI `octara check` matrix additionally validates the Choices scene's layout and semantics.
This run passed 499 tests (2 ignored), workspace Clippy with warnings denied, formatting and
diff checks, and the four-cell CLI check matrix. After removing tracing, the shell was checked
again and the rebuilt release binary passed the final native sequence above.

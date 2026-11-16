# Virtual row residency during interaction

Implemented and verified in the working tree, 2026-11-16. No public residency API was added.

## Ownership rule

Virtualization decides which rows need mounted resources; business membership decides whether a
row exists. Scrolling a row outside the viewport must not cancel its current text editing or
pointer interaction. Removing the row from its source, replacing its owning branch, reloading
its tree or closing its window still retires the old allocation normally.

The window's existing focus and pointer state remain authoritative. A private revision invalidates
list residency when these owners change; it is not another interaction registry. The list may retain a
resident row containing the focused editor or an active captured pointer's allocation, including
row ancestors when lists are nested. Composition and editor selection belong to that focused
allocation; there is no separate list-owned copy of either. Hover alone does not retain a row.
Overlay content follows a private logical owner relation,
independent of its physical overlay-layer parent and positioning anchor, so both anchored and
window overlays can keep their owning row resident.

Release must work without another scroll: when focus leaves or the pointer releases/cancels,
the old row becomes eligible for ordinary eviction in the same settling process. The final
pointer callback must still reach its allocation before the temporary capture hold ends.
Actual scope disposal overrides this hold; it cannot resurrect a removed entity. Element cleanup
clears focus, queued caret reveal, editing history and relevant capture before destroying nodes,
without invoking callbacks of the retiring scope. A touch pan retains its scroll-container owner,
not the obsolete inner row where the touch originally landed.

## Sparse geometry

A retained row can be thousands of rows away from the viewport. Keep that row and the ordinary
visible range, with spacers representing the intervening omitted ranges. Do not mount every row
between them. Mounted nodes remain in source order and at their actual content coordinates;
scroll height must not count a pinned row twice or compress the gap around it.

The existing weighted index continues to own estimates and measured heights. Interaction
retention is not a new measurement or reconciliation engine. The maximum resident set is the
visible/overscan rows plus rows needed by current interactions, not every previously visited row.
Durable expansion or application state that must survive later eviction still belongs to the
model, keyed by business identity.

## Validation requirements

- Focus, caret, selection, undo and Chinese composition survive scrolling beyond overscan.
- Focus transfer and blur release an offscreen row without requiring another scroll.
- A captured drag survives scrolling and receives its final End/Cancel; the row is released
  afterwards. Window deactivation cancels capture through normal input handling.
- Data removal, source replacement, branch disposal and window close retire targets and prevent
  late input from affecting a new row with the same test ID.
- Sparse row positions, source order and scroll extent remain correct; node counts return to
  their ordinary viewport bound after interaction finishes.
- Real native-window Inspector evidence is reported separately from native OS IME qualification.

## Regression evidence

`cargo test --workspace` passed: 735 tests, 3 ignored. The final overlay regression was added
subsequently; the complete `virtual_interaction::` suite then passed all 18 tests, covering both
anchored and window overlays. Workspace Clippy with `--all-targets -- -D warnings`, formatting,
and whitespace checks passed; the final examples-only Clippy check covers that last test.

The focused-editor regression reproduced lost focus before the implementation. Current coverage
includes composition, reversed selection, undo/caret reveal, simultaneous focus and drag owners,
release without another scroll, deactivation, data removal/reinsertion, source switching, bounded
history recovery, branch/window disposal, final drag/long-press callback reentrancy, nested lists,
touch pan ownership, and exact fixed-gap geometry against an unpinned control.

## Native-window evidence

The macOS `virtual_interaction` fixture has 10,000 rows. Inspector-driven native-window checks
retained the original row-0 target, reversed byte selection `5..7`, preedit `nihao` and marked
selection `1..4` after scrolling to offset 320,000 (row 5,000). The settled GPU capture showed
19 resident rows / 122 live nodes. Blur evicted the old editor; drag End released the captured
row; removing its data cleared focus; reinserting the same key produced a fresh guarded target.
Inspector reported no issues. After reset at the top, the capture showed 13 rows / 91 nodes.

Local artifacts for this run: `/tmp/octara-virtual-interaction-TDG9bF/`, including
`replies.jsonl`, `composing-far.png` and `recovered.png`. These are real native-window/GPU
checks using framework input through Inspector. Preedit was injected, not typed through the
system input method; native OS Chinese IME qualification remains separate.

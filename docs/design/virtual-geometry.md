# Virtual row measurements and reading anchors

Implemented in the working tree, 2026-11-16. This extends the existing weighted index and
[row residency](virtual-interaction.md); no public primitive or execution engine is added.

## Reading position

A reading anchor is a business key and its offset inside the row, captured before modifying
order or weights. Restore it before choosing the resident window, so a prepend or reorder does
not first evict the reader. If the key disappears, use the next surviving old neighbour, then
the previous one. Layout clamps the resulting offset to the actual scrollable extent.

At offset zero, stay at the start so newly prepended content is visible. While following the
tail, the existing scroll container owns the position; row anchoring does not compete with it.
Manual scrolling releases following; returning to the end resumes it. A newer explicit scroll
request also cancels any pending anchor restoration (including a request from a watch later in
the same flush). A private per-container request counter distinguishes that request from layout
clamping, tail retargeting and internal anchor adjustments.

An estimate can be much shorter than the paragraph being read. Carry the saved anchor through
reconciliation and mount that row until its layout is available, even if the estimated viewport
would omit it. Clamp an intra-row offset only against measured geometry, including a row gap,
never against an estimate. Scope/data removal still wins over temporary residency.

The after-layout path also accounts for changes to preceding siblings using the previous list
origin. `scroll_to_unclamped` schedules a layout when its offset is outside the current range:
a restored position must eventually be clamped even when reconciliation leaves spacer styles
unchanged. This matters when every row is already resident and a tall final row shrinks.

## Measurement validity

Keep the estimated height separately from the weighted index's current measured height.
Invalidation has two existing sources:

- Item changes arrive through the collection's private Observer path. One set of dirty keys
  coalesces updates; removals remove those keys. Only the current source registration is held;
  the upstream retains it weakly. Consumer scope and producer lifetime guards reject work during
  retirement. Subscribing prunes dead weak registrations. Ordinary `keys`, `len` and `each`
  structural consumers are not invalidated by this path.
- After layout, compare the container's content dimensions, viewport, inherited text style,
  font generation and theme generation. Changed constraints invalidate cached measurements;
  reset weights to stored estimates and then measure only resident rows.

A new snapshot or source cannot prove unchanged values (`T` needs no equality bound), so it
conservatively rebuilds estimates while preserving surviving keyed allocations and the reading
anchor. Incremental edits read only affected items. Global invalidation updates O(n) numeric
weights with the existing O(log n) tree operations, but does not construct or lay out n views.
Caches and dirty keys are bounded by collection membership; mounted rows remain bounded by the
viewport/overscan plus current interaction and pending reading anchors.

Unseen geometry is an estimate until measured. Dependencies used only inside an unmounted row
closure cannot be observed generically. App state affecting an offscreen row's geometry should
be represented in its item/source; this change does not promise exact offscreen layout or keep
all row bindings alive. No new public invalidation, generation or pin API is introduced.

## Reference choices

Local Exact's `runner/src/instance/collection/index.rs` captures anchors before changing keys
or heights, restores by key, and uses surviving neighbours on removal. Its explicit start-of-list
and tail policies inform this contract. Rune's `el/virtual_list.rs` separates keyed height
knowledge from resident rows and preserves a surviving visible key during key changes.
Octara's synchronous layout needs no equivalent of Exact's asynchronous host measurement epoch.
These references were inspected locally under `archive/reference`; no latest-upstream claim is made.

## Validation

The deterministic `virtual_geometry` suite has 16 passing tests and covers variable-height prepends/removals/moves,
removed-anchor fallback, offscreen item edits, width and font invalidation, start/tail policies,
estimates shorter than the reading offset, row gaps, short-list shrink/clamp, newer explicit
scroll requests, bounded reads/mounts and observer cleanup.
Chat's existing 1k/10k history tests now also assert exact reading key and content-relative offset
through four window resizes and composer growth.

Native-window fixture evidence: 10,000 wrapping rows, reading key 4995 at -76 logical pixels.
Prepend, remove, move, grow-above, inherited font change and widths 720/1200/900 kept that anchor
at -76; only 14–18 rows were resident. Returning to the tail and prepending there kept following.
Inspector issues were empty; GPU captures were visually checked. Local evidence is under
`/tmp/octara-geometry-live-KzsXJ2/` (JSONL and PNG files). These checks use framework/semantic
Inspector operations in a real macOS window, not OS-input or native IME qualification.

Real Chat evidence: isolated 1,000-message history, 110 streamed chunks, reading `msg-999` at
-99 logical pixels. Widths 760/1000/480/1000 retained its key, offset and guarded node target;
returning to latest resumed following through more output and Stop. The only reported layout
issue was the fixture's intentionally truncated model label. Evidence: `chat-replies.jsonl` and
`chat-reading.png` in the same temporary directory.

Final checks: `cargo test --workspace` passed 751 tests with 3 existing ignored tests. The
last explicit-scroll/watch regression was added after that suite compiled; the complete focused
suite then passed all 16 tests. Workspace Clippy (`--all-targets -- -D warnings`) passed, as did
a final examples Clippy run covering the last test, formatting and whitespace checks. Both
native scripts were repeated successfully against the final production build.

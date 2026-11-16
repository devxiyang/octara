# Bounded window updates and truthful completion

Implemented in the working tree, 2026-11-16. This is a completion/ownership correction to the
existing runtime; it does not introduce another scheduler or a public frame transaction API.

## One outcome through preparation

Layout preparation returns completed or unfinished. Completion requires both reactive propagation
and after-layout feedback to have stopped. Reaching the four-round layout budget leaves the work
unfinished; it does not establish either convergence or an infinite cycle. The existing
`LAYOUT_LOOP` diagnostic describes this uncertainty.

Every layout call in window preparation propagates that outcome, including modal/focus work,
caret reveal and deferred commands. An unfinished attempt stops before native placement and
platform frame publication. Undelivered commands/reveal requests remain owned by the window;
callbacks already delivered are not replayed. Closing the window disposes its work normally.

The window's completed generation is an `Option`: `None` means no completed preparation exists
for the current attempt. The App frame records a separate attempted outcome, so an unfinished
window is not repeatedly retried during that same frame, even if another window changes.
Completed windows still re-enter preparation when another window invalidates them. Hover repair
does not immediately repeat an unfinished input frame.

No retry deadline is created. A later explicit update boundary can continue the retained work;
input that stops the feedback can recover the window. A finite measurement chain longer than
four rounds can also finish across later attempts. Reads of pending status do not run work.

## One pending report

`App::pending` includes `WorkKind::Update` for unfinished reactive or window preparation, with no
timer deadline. The existing `settle` / `run_until_idle`, Inspector pending count and
`Window::unsettled` consume that same information. Capture therefore cannot report `settled: true`
merely because there are no asynchronous tasks or animations.

Diagnostic inspection and captures may still expose the current partial tree, with pending
reasons attached; this change is not a stored display-snapshot or rollback system. Successful
publication, a GPU capture and window-server presentation evidence remain distinct claims.
The runtime does not undo Rust callback effects or automatically retry their input.

## Reference mechanisms

Local references were read directly, without claiming to have fetched their newest upstream
revisions:

- Exact `08c889a9de21a30b7c3b308422fc588cf1a480d3`,
  `host/apple/src/layout.rs`: failed bounded provisional measurements preserve `layout_owed`
  and withheld geometry; a later inert advance can publish the recovered result.
  `control_text_tests.rs::running_session_chrome_failure_keeps_updates_but_withholds_staged_geometry`
  checks this behavior. This is the closest precedent for Octara's unfinished outcome.
- Rune `3686e94ae09545fd5f2a61e22aabfbeba1cd896f`,
  `crates/rune/src/runtime/commit.rs` and `commit/measure.rs`: completed `Measured` geometry
  precedes scene publication. Rune can force virtual rows after bounded retries; Octara does
  not copy that all-row fallback. Rune's separate `layout_read.rs` feedback policy can baseline
  size reads and quiet repeated feedback; that is a policy for arresting feedback, not proof
  that exceeding a budget means convergence.

Both repositories are under `/Users/devxiyang/code/archive/reference/`. Octara keeps its existing
Signals, scopes, weighted virtual-list index and synchronous layout ownership.

## Validation

`size_of` regressions use bounded width oscillation rather than exponential string growth.
They verify pending state, refused Inspector settlement, withheld platform frame publication,
read-only queries without retries/deadlines, input recovery, shutdown while unfinished, and a
finite chain requiring more than four rounds with matching final measurement and geometry.

The native `layout_feedback` fixture was driven through its Inspector socket. Start produced
pending updates; a zero-budget settle failed; a GPU capture reported `settled: false` with update
reasons. Stop cleared pending, settle succeeded and a recovered GPU capture reported
`settled: true`. Artifacts: `/tmp/octara-settlement-live-7WniSX/` (`replies.jsonl`, `pending.png`,
`recovered.png`). This is a real macOS window using framework/semantic Inspector operations.

Final checks: `cargo test --workspace` passed 754 tests with 3 existing ignored tests;
`cargo clippy --workspace --all-targets -- -D warnings`, formatting and whitespace checks passed.
Read-only architecture review found no remaining blocker. The previous implementation's missing
completion propagation was established by code inspection; no pre-fix test failure was recorded.

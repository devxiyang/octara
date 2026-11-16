# Collection mutation and failure admission after 4718352

Reviewed 2026-11-16. Committed baseline: 830 tests passed, 3 ignored, Clippy passed.
This is a source audit and one temporary behavioral probe, not a runtime change.

## Reproduced boundary

StateList::update clones the old row, calls Signal::update, then clones the new row
and emits the collection event. Signal mutation invalidates its reactive readers during
unwind, but an unwinding row update never reaches collection event delivery.

The probe creates row `(1, false)` and a filter selecting true rows. Inside App::run it
catches a panic from an update that sets the row to true before panicking. Afterwards:

- The source item reads true.
- The filtered keys are still empty rather than `[1]`.
- App::is_failed is false before the probe's consistency assertion.

The assertion fails. This does not demonstrate divergence during ordinary successful
updates, nor a rollback guarantee violation: reactive-core.md explicitly disclaims safe
resumption of arbitrary user state after panic. It demonstrates that the runtime does
not currently enforce that boundary when framework-owned incremental state is interrupted
and an outer application catch prevents the panic escaping App admission.

## Related source path to cover

Runtime::deliver_collections removes abandoned deliveries and resets its active flag on
unwind. This releases bookkeeping correctly, but does not mark the partially propagated
collection graph unusable. An application catching a projection panic could therefore
continue after some observers were updated and others abandoned. This second scenario
was source-reviewed, not separately reproduced in this audit.

## Mechanisms worth borrowing

Exact, local snapshot `08c889a9de21a30b7c3b308422fc588cf1a480d3`,
`kernel/src/kernel/document.rs::continue_document_commit` / `finish_document_commit`:
a failed phase marks the logical commit failed and prevents final publication. Earlier
writes may remain; no rollback is promised, and the owner must poison/reset rather than
present partial state. Borrow explicit invalidity at the interrupted operation boundary,
not its VM or document transaction implementation.

Rune, local snapshot `3686e94ae09545fd5f2a61e22aabfbeba1cd896f`,
`crates/rune/src/memo.rs` and `runtime/commit/memo_reuse.rs`: subtree identity alone is
not validity. Explicit captured versions, environment and recorded layout reads determine
reuse. This is a different memo model from Octara's Signals; the transferable lesson is
that a surviving handle/cache must not imply valid derived contents. Do not introduce
manual version arguments into automatically tracked Signal memos.

These are inspected snapshots, not latest-upstream claims. No reference runtime was run.

## Recommended next coherent change

Define one internal failure boundary for collection mutation plus propagation. If an
operation unwinds after potentially changing framework state, preserve teardown capability
but reject further derivation/publication even when application code catches the panic.
Connect this to the existing App failure policy instead of adding a parallel public error
system. Clarify how synchronous reads detect the invalid graph before the next App entry.

Do not try to roll back arbitrary Clone/Drop/PartialEq callbacks, replay started observers,
or re-run failed user projections. A source snapshot alone cannot reconstruct external side
effects or shared interior state. Keep pre-mutation validation refusals distinct from an
interrupted mutation, so harmless rejection does not unnecessarily fail an App.

Acceptance should cover caught row-update panic, clone/projection failures at the commit
boundary, sibling and composed derives, prevention of subsequent frame publication, and
successful cleanup after failure. Preserve ordinary dynamic dependencies, equality cutoff,
bounded collection history and efficient keyed updates; no new public primitive is indicated.

## Evidence

`cargo test -p octara --lib audit_caught_collection` failed on the source/filter consistency
assertion. Log: `/tmp/octara-collection-boundary.log`; probe source snapshot:
`/tmp/octara-collection-boundary.probe.rs`. Temporary code edits were removed afterwards.
No full test suite was rerun for this documentation-only audit and no native window test
was performed.

## Implemented follow-up

App and Runtime now share their failure flag. StateList mutation and synchronous collection
delivery use an unwind guard which records APP_FAILED locally. Reactive read/write and
publication boundaries reject the failed graph; task draining stops before a sibling can
execute after a caught collection failure. App teardown remains available and source values
can still be read in Cleanup, while derived evaluation and collection reads stay forbidden.

Permanent regressions cover caught row edits with no new presented frame, projection failure
with sibling/composed derivations, post-mutation Clone failure, nonterminal pre-mutation
refusals/predicate failure, task-batch interruption and successful resource teardown. The
previous delivery-unwind test now verifies refusal rather than attempting to resume a graph
whose queued deltas were abandoned. Public APIs and identity exhaustion checks are unchanged.

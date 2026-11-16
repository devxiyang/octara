# Reactive tracking boundaries after 9c193dd

Reviewed 2026-11-16. Committed baseline: 835 tests passed, 3 ignored, Clippy passed.
This audit contains a temporary behavioral reproduction, not a runtime fix.

## Reproduced watch dependency leak

`reactive/graph/signal.rs::watch` promises to track only its source. Its callback
already runs without a dependency collector. However, the source result's
`PartialEq`, replacement of the previous result, and disposal of an equal result
remain inside the observer's tracked computation.

A temporary probe used a source reading one Signal and returning a snapshot. In
one case the snapshot's `PartialEq` read a second Signal; in another its `Drop`
read that Signal. After updating the source, changing only the second Signal
recomputed the source in both cases: the invocation count increased from 2 to 3
instead of staying at 2. The callback itself read neither Signal.

These are successful updates, not panic recovery scenarios. User-defined equality
and destructors can accidentally expand the dependency set beyond the source.

## Related path to review

`memo_with` also performs its equality comparison and disposes an equal candidate
inside its tracked Compute closure. Review this alongside watch so both use the
same tracking rule. This memo case was source-reviewed, not separately reproduced.

## Useful reference mechanisms

Rune snapshot `3686e94ae09545fd5f2a61e22aabfbeba1cd896f`,
`crates/rune/src/memo.rs` and `memo/snapshot.rs`: subtree reuse checks captured
versions, environment and layout reads. Component history snapshots are anchored
to the start of a logical commit even across measurement retries. The useful
lesson is explicit validity and evaluation boundaries. Rune's subtree memo is not
Octara's reactive memo; copying its cache or manual capture model is unwarranted.

Exact snapshot `08c889a9de21a30b7c3b308422fc588cf1a480d3`,
`runner/src/runner/settlement.rs`: settlement tracks dependency provenance as well
as values. Current resource arguments and a standing result can have different
provenance while a request is in flight. Dependency meaning is explicit rather
than inferred from incidental value storage operations. This does not establish
that Exact solves Rust destructor tracking with the same mechanism we need.

These are inspected local snapshots, not claims about latest upstream versions.
Neither reference runtime was executed for this audit.

## Recommended next change

Separate the existing internal evaluation stages:

1. Run the source with its observer collector.
2. Compare results without collecting dependencies.
3. Run a watch callback using its existing handler context.
4. Retire replaced or unused results without collecting dependencies.

Removing a collector must not grant handler write permissions to equality or
destructor code. Preserve the execution context and existing failure/teardown
rules. Use a narrow internal boundary, not a new public primitive or a general
phase framework. No public manual dependency list is needed.

Permanent regressions should cover watch equality and retirement, memo equality
and equal candidate retirement, dynamic source dependencies, equality cutoffs,
self-writing watch callbacks, and restoration of tracking after unwind/disposal.

## Evidence and limits

`cargo test -p octara --lib audit_watch` failed with observations
`[(false, 2, 3), (true, 2, 3)]`, expected `[(false, 2, 2), (true, 2, 2)]`;
the boolean selects Drop versus PartialEq. Probe log:
`/tmp/octara-tracking-audit.log`; full temporary test file:
`/tmp/octara-tracking-audit.probe.rs`.

Temporary test edits were removed after reproduction. No full suite was rerun
for this documentation-only audit; no native window validation was performed.

## Implemented follow-up

`memo_with` and `watch` now sample comparison and result retirement using the
existing untracking mechanism. The closures own their candidate values, so unwind
also drops those values before restoring the collector. Watch callbacks alone
retain their explicit Handler transition.

`Runtime::run_node` encloses evaluation, publication and retirement in the node's
owner/scope and Derive/Observe context with sampling enabled. Only its Compute
invocation installs the node collector. This also protects outer observers from
old memo values or interrupted computation captures dropped after Compute returns.
No public primitive or dependency was added.

Four permanent graph regressions cover equality and both retirement paths for
watch and memo, nested observers, denied writes, comparison unwind restoring the
outer collector, and result retirement after comparison/callback panic. Existing
dynamic dependencies, memo cutoffs, self-writing watches, stop and scope disposal
remain covered by the full suite.

Validation: `cargo test --workspace` passed (839 passed, 3 ignored), including
local HTTP integration tests; `cargo clippy --workspace --all-targets -- -D warnings`,
format and diff checks passed. The first sandboxed test run could not bind loopback
listeners; the successful full run used approved execution outside that restriction.
Logs: `/tmp/octara-tracking-final.log` and `/tmp/octara-tracking-clippy.log`.
No native OS window validation was performed for this graph-only change.

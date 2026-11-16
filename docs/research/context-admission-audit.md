# Context registration admission after 3807612

Reviewed 2026-11-16. Baseline: 839 tests passed, 3 ignored, workspace Clippy passed.
This is a source review and temporary reproduction, not an implementation change.

## Reproduced behavior

`provide` delegates directly to `Runtime::provide` in `reactive/graph/scope.rs`.
That function replaces a typed context entry without checking the execution phase,
runtime failure flag, or whether the current scope remains live.

A probe registered `1u32` and evaluated a memo reading that context. A second memo
then called `provide(2u32)` and read the context. The second memo and direct lookup
returned 2, while the first memo still returned 1. No diagnostic was reported.

The issue is permission admission: pure derivation can mutate framework-owned
environment state. Context lookup is deliberately not a reactive dependency;
existing documentation tells callers to provide a Signal for changing values.
This does not establish a missing promise that arbitrary context replacement must
invalidate cached memos.

Cleanup and retired-scope registration are also unchecked in the implementation.
Those paths were source-reviewed, not separately reproduced here. Existing
`lookup` does check scope identity/liveness; provider insertion should have an
equally explicit ownership contract.

## Reference mechanisms worth keeping

Rune local snapshot `3686e94ae09545fd5f2a61e22aabfbeba1cd896f`,
`crates/rune/src/memo.rs::enter` / `memo_with_hash`: the memo cache includes a hash
of the window environment in its reuse version. Mutable non-layout captures must
be represented in the caller's version. `states.rs` separately models read-only
`StatesRef` and mutable handler `States`. Environment validity and mutation
capabilities are explicit. Octara should retain automatic Signal dependencies
rather than adopt Rune's manual version arguments.

Exact local snapshot `08c889a9de21a30b7c3b308422fc588cf1a480d3`,
`runner/src/vm.rs::Env` / `Outcome` and `runner/settlement.rs`: evaluation receives
borrowed input slices; writes and commands are represented separately in its
outcome. This makes input access and effects distinct at the evaluator boundary.
It is a useful architectural contrast, not proof that Exact has an equivalent
Rust context-provider API. No VM or command buffer is warranted for this fix.

These are inspected snapshots, not latest-upstream claims; neither reference
runtime was run.

## Recommended next bounded design

Treat context as scope-owned registration, with changing contents carried by
Signal/ReadSignal. Define provider admission explicitly, alongside the existing
execution contexts and lifecycle gates:

- Preserve component construction and App-root registration through App::run.
- Reject provider mutation from pure computations, initializers and cleanup,
  and reject failed-runtime or retired-scope registration before replacing values.
- Decide and document handler/task replacement semantics explicitly. Existing
  replacement is non-reactive; do not silently promise it invalidates consumers.
- Preserve nearest-provider lookup, registration scope restoration, and releasing
  replaced values outside internal borrows.

The next implementation should use the existing context/lifecycle policy rather
than add a public primitive, a second reactive graph, or a general capability
framework. Registration is not an ordinary Signal write: Build must remain able
to provide values, so simply calling the existing write gate is insufficient.

Validation should cover memo/watch sources and untrack, initializers, cleanup,
failed/retired owners, App/component registration, nearest-provider lookup,
replacement destructor re-entry, and reactive updates through a provided Signal.

## Evidence

`cargo test -p octara --lib audit_context_mutation` failed as expected:
observed `(2, 2, 1, 0)` versus proposed refusal behavior `(1, 1, 1, 1)`.
The tuple is writer result, direct context, cached memo, diagnostic count.
Log: `/tmp/octara-context-audit.log`; source snapshot:
`/tmp/octara-context-audit.probe.rs`. Temporary source edits were restored exactly.
No full suite was rerun for this documentation-only audit, and no native window
validation was performed.

## Implemented follow-up

Provider registration now checks the runtime failure gate and shares the existing
effect admission policy. It also requires a current scope; the shared gate checks
that scope's runtime identity, generation and live state before insertion.
Pure evaluation, initialization and cleanup are refused with the existing
diagnostics. Public caller locations are forwarded to the registration gate.

Build, App::run, handlers and tasks retain registration/replacement permission.
Replacement changes only the current scope's entry and subsequent lookups; it
does not invalidate cached memos or update previously obtained values. Consumers
use provided Signal/ReadSignal handles for reactive changes. No public primitive,
diagnostic code or dependency was added.

The replacement is installed before the previous value is dropped outside the
scope-table borrow. Reentrant destructors can look up the newly committed entry;
there is no rollback if a destructor panics. Cleanup retains read access to
existing context, while registration remains forbidden.

Six graph regressions cover phase admission through untrack, real memo/watch/
initializer calls, missing/foreign/stale scopes and slot reuse, cleanup, failed
runtime teardown, and replacement/destructor semantics. A headless integration
test drives a component handler and its spawned task, proving their replacements
stay in the child scope and previously captured values remain unchanged. Existing
App-root context/Signal tests cover inheritance through window reopen and reload.

Validation: `cargo test --workspace` passed (846 passed, 3 ignored), including
local HTTP tests. Workspace Clippy with all targets and warnings denied, formatting,
and diff checks passed. Logs: `/tmp/octara-context-final.log` and
`/tmp/octara-context-clippy.log`. This change was validated headlessly; no native
OS window validation was performed.

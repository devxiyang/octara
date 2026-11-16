# Selecting core mechanisms from Solid 2, Exact and Rune

Reviewed 2026-11-16, after Octara commit `aa81b6b`. This is a selection and next-work
record, not a proposal to replace Octara's Rust + Signals model. No additional runtime
changes are made by this review.

## Evidence boundary

Solid 2's [RC.21 release](https://github.com/solidjs/solid/releases/tag/solid-js%402.0.0-rc.21)
is marked prerelease. Its [RFC index](https://github.com/solidjs/solid/blob/next/documentation/solid-2.0/README.md)
also labels the documents prerelease/proposal-shaped. The RFC observations below describe
those documents as read today, not an assertion that a final 2.0 API is frozen.

Exact and Rune implementation evidence uses the already available local clones, pinned to
`08c889a9de21a30b7c3b308422fc588cf1a480d3` and
`3686e94ae09545fd5f2a61e22aabfbeba1cd896f` respectively. Their public repositories were
checked for context; this review does not assert these local commits are latest upstream.

## Admission rule

Adopt a mechanism only when it addresses a demonstrated Octara failure or removes repeated
application work, fits synchronous Rust access and scope ownership, and has a bounded
implementation plus an observable acceptance test. A feature name or a shorter public API
alone is insufficient. Existing capabilities count as completed work, not reasons to add
another abstraction.

## Solid 2: separate responsibilities, inspect the timing contract

The [reactivity/effects RFC](https://github.com/solidjs/solid/blob/next/documentation/solid-2.0/01-reactivity-batching-effects.md)
separates tracked computation from side effects, describes cleanup before another effect
invocation, and keeps untracking distinct from write permission. It also describes
microtask-committed writes, effect callbacks without their own equality cutoff, and
optional memo autodisposal.

Octara already separates `watch(source, callback)` tracking from untracked handler execution,
and distinguishes `untrack` from mutation permission. Preserve these choices. Its watch
also compares source values, and its memo retains a cache for the owning scope: these are
intentional differences, not missing compatibility. Do not change them to match Solid.

One difference deserves investigation: Octara runs each watch's source and callback together;
Solid describes all compute halves before effect halves. A two-phase scheduler would change
feedback behavior and capture timing. First test two watches where the first changes a
source of the second; define the desired observations before considering such a scheduler.
No defect in Octara's documented feedback semantics is established by this difference alone.

Per-invocation cleanup is useful for a timer/subscription replaced when its source changes.
Octara currently owns work by component/App scope, while `resource` explicitly replaces its
previous task. A scoped lifetime and an invocation lifetime are not interchangeable. Do not
silently make all work created in a watch disappear on its next invocation. Before adding
an API, inventory actual resource replacement sites and choose a Rust ownership expression
that handles replacement, stop, scope retirement and panic together.

Solid's [RC announcement](https://github.com/solidjs/solid/discussions/2995) embeds async
results in the reactive graph and supplies pending/reveal behavior. Octara's explicit
`Resource<Load<T,E>>` and synchronous signal reads remain appropriate for the current native
runtime. Transparent async reads would require new pending/error semantics throughout the
graph; fewer API names would not make that implementation free.

## Exact: validity before publication

Exact's [web runtime](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/web-js/rt.js)
uses lazy freshening, equality cutoff and owner cleanup. Its resource contract admits an
answer only while its request is current; cancellation alone is not the semantic proof.
Its [runner settlement](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/runner/src/runner/settlement.rs)
checks dependency readiness before taking unchanged-input shortcuts.

Octara's collection queue now enforces parent settlement before joins. Its scalar graph
already has Clean/Check/Dirty and pull-on-read validation. Keep their shared contract while
allowing different algorithms: collection deltas and dynamically tracked scalar reads do
not need one universal scheduling representation.

The most useful next audit is asynchronous publication. `task.rs::resource` cancels the
previous task and then publishes a completed load; Chat separately checks the current
answer identity. Examine whether cancellation, active-poll reentrancy, refetch and scope
retirement cover every publication path. Add a request identity guard only if the current
ownership/cancellation protocol cannot establish validity. A scheduler slot generation
protects task identity; it does not automatically define logical request freshness.

Keep Exact's distinction between observed state and completion. Octara's existing pending
work and incomplete-window capture refusal are already suitable Agent-facing evidence.
Extend those reports from runtime facts when a concrete diagnostic is missing; avoid a
parallel Agent execution state machine. Whole-action buffering and a VM are not needed for
these guarantees.

## Rune: cache the work whose validity can be explained

Rune's [memo implementation](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/memo.rs)
combines stable identity, a supplied version, environment and recorded geometry reads when
reusing a subtree. It uses ordinary Rust closures and explicit reuse boundaries.

For Octara, preserve business-key identity while checking value/layout validity separately.
The virtual geometry cache already considers item changes, width/viewport, inherited text,
font and theme generations. A stable key alone must never certify old measurements.
Future caches should list their dependencies and prove invalidation under one changed
input at a time. Do not add Rune-style whole-view version arguments to every Signals
component: that would duplicate automatic dependency tracking and burden callers.

## Next bounded core work

1. Audit resource replacement and result admission. Cover A→B→A, same-key refetch,
   late completion/wake, scope retirement, and a future changing its own request input
   during polling. Assert published values, pending-work retirement and captured-value
   release. Distinguish cancelling UI publication from aborting external side effects.
2. Review replacement cleanup across resources and watches. Reuse one internal ownership
   mechanism where semantics truly match; keep public `watch` unchanged unless a concrete
   use case and lifecycle contract justify the change.
3. Audit scalar dynamic dependency switching and equality cutoff together with watch
   feedback. Preserve immediate reads and existing structural-before-property-before-watch
   ordering. Consider graph-edge/storage optimization only after measuring a real cost.

Each step is independently reviewable. None requires new UI components, a deep Store,
transparent async signals, subscriber-count ownership, a transition engine, or a new VM.

## Repository cross-checks

- `crates/octara/src/reactive/graph.rs`: `mark`, `update_if_necessary`, `run_node`, `flush`, `watch`.
- `crates/octara/src/task.rs`: scheduler cancellation/polling, `Resource`, `resource`.
- `apps/chat/src/chat.rs`: current request identity, cancellation installation and retirement.
- [Collection propagation](../design/collection-propagation.md),
  [window completion](../design/window-completion.md),
  [virtual geometry](../design/virtual-geometry.md).

This review is source/document analysis. The preceding implementation commit passed 761 tests
with 3 existing ignored tests and Clippy. This research adds no new runtime validation claim.

## First implementation follow-up

The resource audit reproduced a concrete failure: a future changed its own key during its
final poll and its obsolete result became the retained successful value before replacement.
The follow-up uses a private memo of the existing key/refetch input and the scheduler's
existing task liveness check at publication. It does not introduce an independent request
counter or change watch cleanup semantics. See [async lifecycle](../design/async.md) for the
contract and regression coverage. Remaining watch-lifetime and scalar-graph reviews above
are separate work, not implied completed by this fix.

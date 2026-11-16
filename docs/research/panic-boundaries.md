# Failure boundaries: Rune, Exact, Solid 2 and Octara

Reviewed 2026-11-16. Rune and Exact are the local snapshots
`3686e94ae09545fd5f2a61e22aabfbeba1cd896f` and
`08c889a9de21a30b7c3b308422fc588cf1a480d3`; no latest-upstream claim is made.
Solid evidence is its current prerelease RFC, not a claim that JavaScript exceptions and Rust
panics are equivalent.

## Rune: specific recovery boundaries

- [Image decoding](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/image.rs#L128)
  catches codec panics and returns ImageError, avoiding a permanently pending image/worker.
- [Latest worker](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/latest/pool.rs#L107)
  catches a job panic, settles that attempt as failed and continues its worker.
- [Presentation worker](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/frame/present_worker.rs#L47)
  transports a panic back to the owner's thread and resumes unwinding there.
- With the hot feature, [view construction](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/runtime/commit/measure.rs#L395)
  discards failed build caches/context and rebuilds using the startup view. Its
  [patched callback fallback](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/hot.rs#L76)
  explicitly leaves prior side effects in place. This is development recovery, not rollback.
- Native callback adapters catch panics to avoid unwinding into AppKit. That ABI boundary
  does not by itself establish application-state consistency.

These are different policies at different seams, not a universal recover-every-panic rule.
Octara should not copy automatic replay of an effectful callback: a failed callback may already
have sent a request or mutated shared state.

## Exact: refused commits versus poisoned execution

The [web commit](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/web-js/rt.js#L166)
records writes and selected runtime state before settling. A failure in the rollback-covered
phase restores them and reports a refusal. A failure while updating the tree takes a different
path: it poisons execution and forgets scheduled work.

The [native conclude path](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/runner/src/runner/commit.rs#L136)
also distinguishes refused Result outcomes from newly poisoned execution. These are runner/VM
protocol outcomes, not a generic catch_unwind policy for arbitrary Rust callbacks. Its checkpoint
model cannot roll back our arbitrary in-place Rust mutations or external effects.

Adopt the distinction between rejected work and an unusable execution state. Do not import
checkpoint infrastructure merely to recover panics.

## Solid 2: handled computation errors versus halted reactivity

The [control-flow RFC](https://github.com/solidjs/solid/blob/next/documentation/solid-2.0/03-control-flow.md#error-boundary-errored)
describes Errored as a subtree boundary with a fallback and reset action. Handled errors can be
reported with owner/boundary context. It separately says an unhandled reactive error halts the
reactive system (REACTIVITY_HALTED) and reports to the platform. The
[effects RFC](https://github.com/solidjs/solid/blob/next/documentation/solid-2.0/01-reactivity-batching-effects.md#createeffect-error-handling)
allows an explicit effect error handler.

This is structured graph error handling, not a promise to recover every event-handler exception
or undo JavaScript side effects. Octara's Result and AsyncState::Failed already represent
expected asynchronous failures without failing the App. A future fallible subtree boundary
would need a reconstruction and ownership contract; catching arbitrary Rust panics is not enough.

## Octara decision for this slice

1. Expected operation failures stay Result/diagnostics; failed async results stay AsyncState::Failed.
2. Isolated adapters may contain failures only with an explicit local contract. No generic
   panic-to-E conversion or automatic retry is introduced.
3. Unwinding preserves bookkeeping and releases owned work. If a panic escapes an App/window
   entry, the App conservatively loses ordinary execution/publication rights. This does not
   assert memory corruption; it says the runtime cannot certify partially mutated application
   state. Diagnostic reads and explicit shutdown remain available.
4. A known validation panic caught entirely inside user code before crossing the entry boundary
   does not fail the App. Existing validation tests use that boundary deliberately. Foreign-runtime reads,
   state type conflicts and cycles remain programming errors, not routine refusals.
   Fallible external operations share a pre-delivery admission check and return their existing
   Result error on a failed App. Do not guess recoverability from panic text.
5. A nested entry that failed still fails the App even if its caller catches the payload.
   Subsequent publication is refused. Independent Apps remain independent.

The App-wide fallback is justified by shared App state, not by treating one widget as globally
fatal by definition. Local recovery can replace it in a proven isolated subsystem later.
[Rust's catch_unwind documentation](https://doc.rust-lang.org/std/panic/fn.catch_unwind.html)
recommends Result for regular failures and notes that aborting panics cannot be caught.

## Blocking worker follow-through

Review found that Octara's background closure ran directly in the shared pool: a panic killed
that worker before result delivery and before decrementing the in-flight counter. An awaiting
task could remain pending forever, and repeated failures depleted the pool.

The selected correction combines two narrow mechanisms from the Rune sources above: catch the
work closure so the pool survives, and transport the original panic to the awaiting thread.
It does not copy Latest's application-specific error representation. Background<T> still yields
T; callers return Result<T, E> for expected failures. Cancelled interest discards late outcomes,
while completion bookkeeping always retires a work-closure panic.

Deterministic tests run six sequential worker panics (more than the four pool threads), check
the original payload and failed owning App, then successfully run work returning a business
error in a fresh App. A separately gated worker panics only after its awaiting task is
cancelled; the task stays retired and its App remains usable. These tests need no desktop.

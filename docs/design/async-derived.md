# Async derivation: public contract

Implemented 2026-11-16. The public API is `async_memo`, `AsyncMemo` and `AsyncState`.
The previous resource API is removed without compatibility aliases. The result-admission
protocol is retained.

## Responsibility and naming

An async derivation maps reactive input to a fallible asynchronous result. The runtime
owns replacement, cancellation and publication; the caller owns the actual async operation.

Migration vocabulary:

| Current | Proposed | Meaning |
| --- | --- | --- |
| `resource(source, fetch)` | `async_memo(source, compute)` | Declare a cached async derivation of explicit reactive input |
| `Resource<T, E>` | `AsyncMemo<T, E>` | Scope-owned read handle plus explicit refresh |
| `Load<T, E>` | `AsyncState<T, E>` | The observable state, including a retained successful value |
| `refetch()` | `refresh()` | Recompute the same input, including non-network computations |

Keep ordinary `memo` synchronous. `Task` remains an execution handle and is not a value
container. The async handle is not directly awaitable: repeated reactive requests are not
one Rust Future. Reading it never blocks or implicitly suspends a component.

## Separate tracking from execution

The constructor keeps two closures, conceptually:

```text
async_memo(
    source: Fn() -> K,
    compute: Fn(K) -> Future<Output = Result<T, E>>,
) -> AsyncMemo<T, E>
```

Only `source` records dependencies. It runs synchronously and is pure. An equal source value
does not start another request unless refreshed. The compute closure receives an owned input
and its future runs through the existing task scheduler. Reads inside the future do not add
reactive dependencies, before or after await. Callers put every input that should restart
work into `source`, including tuples and ordinary Rust structs.

This explicit boundary avoids tracking only the first poll of a Rust future, silently missing
reads after an await, or subscribing a whole task while it polls. It also makes the input
used for result admission inspectable.

## State and reading

State shape:

```rust,ignore
pub enum AsyncState<T, E> {
    Pending { previous: Option<T> },
    Ready(T),
    Failed { error: E, previous: Option<T> },
}
```

`previous` always means the last successful value, never a result from a rejected request.
First load is Pending without a value. Replacement/refresh enters Pending with the last
successful value. Success becomes Ready; failure becomes Failed and retains that same value.
Refreshing after failure clears the previous error in this implementation. That is an intentional
change from the former Load, which keeps its error while loading again.

The handle provides `with` for borrowed state access, `get` when the state is Clone, and
`refresh`. The state provides `value() -> Option<&T>`, `error() -> Option<&E>`, and
`is_pending()`. Use ordinary `memo` to project a value or status for a binding. Do not add a
second family of async mapping/join operators as part of this change.

Keep output equality separate from input equality: input equality avoids restarting work;
output publication exposes the new attempt's state. The name memo means cached derivation,
not a promise to require PartialEq on arbitrary error types. A synchronous memo over a chosen
projection supplies the existing equality cutoff. Neither construction nor borrowed inspection requires Clone on T or E. Only get requires
Clone; input K remains Clone + PartialEq.

## Internal mechanisms

Reuse the reactive memo for current input, the scope for ownership, the task scheduler for
execution/cancellation, and a signal for observable state. The internal input includes the
refresh counter. No parallel dependency graph, executor, request-ID system or VM is needed.

Before publishing a completed result, require the current task to remain live and its captured
input/refresh value to match the current input. Cancel superseded tasks promptly; active polls
can finish but lose publication rights. Cancellation does not roll back external side effects.

A retained read handle does not extend scope execution lifetime. A pending state describes
this derivation; it does not by itself mean that the window has unfinished layout or that an
Agent cannot inspect it. Existing App pending-work reporting accounts for the executing task.

## Reference choices

[Selection evidence](../research/core-mechanism-selection.md) records the reference versions.
Solid 2 contributes the model of async as derivation and clear result-replacement semantics.
Its transparent not-ready reads, held writes and Loading graph are not prerequisites here.
Exact contributes checking current request validity before publishing. Rune contributes
ordinary Rust values and an explicit route from external work into application state.

## Migration and acceptance

1. Rename the public constructor, handle and refresh operation; migrate exports, tests,
   showcase, docs and generated skill references together, without old aliases.
2. Replace Load's boolean/optional fields with the explicit state and borrowed inspection.
   Keep this state transition change visible in migration notes and test retry-after-error.
3. Retain the current input/task admission protocol. Extend its tests for errors, retained
   values, unchanged input, refresh, owner retirement and late completion.
4. Run workspace tests, Clippy, doctests and skill consistency checks. UI examples should
   demonstrate first load, refresh with old data and failure without adding a new UI boundary.

Deferred: transparent async graph propagation, automatic multi-request consistency groups,
subscriber-count ownership, optimistic writes and new public lifecycle controls. Add any of
these only for a demonstrated composition problem, with a separate contract.

## Validation

On 2026-11-16, task integration tests passed 25 tests. Workspace tests passed 766 with
3 existing ignored tests, including doctests, generated-skill consistency and example layout
checks. Workspace/all-target Clippy with warnings denied, formatting and diff checks passed.
The migration adds a regression using non-Clone values/errors that exercises retained values,
failure, retry error clearing, unchanged inputs and nontracking compute/future reads. Existing
request-admission and cancellation tests run through the new API. No native-window run is
claimed for this API migration.

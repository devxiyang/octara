# Solid: keep the Signal primitive small

Reviewed 2026-11-15 using official documentation and the fixed Solid v1.9.0 source tag. The tag
pins implementation evidence; it is not a claim about the latest npm version.

## Evidence

- createSignal returns an accessor and a setter. The setter accepts a replacement or a function
  producing the next value. Reading subscribes only when a listener is active.
- The source stores the value, observer links and a comparator. Equality defaults to JavaScript
  `===`; callers can provide another comparator or disable comparison with `equals: false`.
  The docs' options table and prose differ about the default; the pinned source uses equalFn.
- A Signal can contain an object. It does not intercept changes to that object's properties.
  Mutating a property without publishing a new value does not notify; returning the same object
  is also suppressed by the default reference comparison.
- createMemo is a separate computation returning a readonly accessor. Store is a separate module:
  proxies intercept property access and lazily create internal Signals for those properties.
  Store property tracking is not responsibility embedded in createSignal.
- untrack temporarily clears Listener and restores it in finally. It leaves Owner alone. The on
  utility tracks declared sources and runs its callback through untrack. Simple Signal design
  does not imply removing this mechanism or silently disabling tracking across more contexts.

## Consequences for Octara

The earlier [Signals proposal](../design/signals-redesign.md) made field adaptation and its Model
type too central to defining a Signal. Narrow the primitive to value storage, reading, writing
and dependency notification. Keep derived computations and effects in the reactive graph; keep
owner/scoped cleanup and hot-reload keys in the runtime/state-construction layer.

Keep Rust's get/with and set/update vocabulary rather than imitating callable JS accessors or
making readers learn a tuple API just for visual similarity. These express owned versus borrowed
access and replacement versus mutation. Notification/equality and borrowing rules need explicit
contracts; JS reference equality is not equivalent to Rust PartialEq over an arbitrary struct.

Signal<User> can remain ordinary whole-value state. Field selectors and control adapters belong
above the primitive. A future deep Store is a separate decision requiring Rust-specific access
instrumentation; it is not a prerequisite for supporting structs or ordinary forms. StateList and
TextBuffer keep their specialized roles. Do not add field projection, Model, a Store and macros
all at once to solve basic state access.

The user's no-compatibility requirement stands. Separately reassess whether a public untrack is
useful; eliminating an API is not a simplification if it requires more implicit exceptions or
initialization escape routes. Preserve separation of dependency tracking, ownership and mutation
permissions regardless of that choice. Runtime implementation remains unchanged by this review.

## Primary sources

- [createSignal](https://docs.solidjs.com/reference/basic-reactivity/create-signal)
- [createMemo](https://docs.solidjs.com/reference/basic-reactivity/create-memo)
- [createStore](https://docs.solidjs.com/reference/store-utilities/create-store)
- [untrack](https://docs.solidjs.com/reference/reactive-utilities/untrack)
- [Signal, Listener, Owner and on implementation](https://github.com/solidjs/solid/blob/v1.9.0/packages/solid/src/reactive/signal.ts)
- [Store property nodes and proxies](https://github.com/solidjs/solid/blob/v1.9.0/packages/solid/store/src/store.ts)

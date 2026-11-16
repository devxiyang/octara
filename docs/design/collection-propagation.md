# Synchronous collection propagation

Collection mutations settle their affected derived collections before returning. A handler
can write a `StateList` and immediately read a derived item or a memo of its snapshot.
This is a boundary per mutation, not an atomic transaction over the whole handler.

## Why recursive delivery was insufficient

When one source feeds two maps that rejoin, recursive notification updates one map and
runs the join before the other map receives the event. Final reads can be correct while
the join projector has already observed mixed revisions. Filters also expose transient
missing partners. Observer registration order must not determine these results.

## Delivery order

The existing incremental operators retain their caches, per-item signals and equality
cutoffs. A private producer depth is one plus the maximum depth of its parents; source
collections have depth zero. Every listener records its **consumer's** depth, including
listeners on a shallow input of a deeper join.

The runtime queues notifications, drains lower depths first, and preserves FIFO order
within each depth. Nested notifications append work to the same synchronous drain.
All listeners of an emission are queued before draining starts. Thus a join runs after
its parent caches have processed the originating change, regardless of branch lengths
or registration order. No UI reconciliation or effects execute through this queue.

A producer can emit several deltas for one mutation: creating a group publishes its
header and then updates its size. A join reads the current left item, rather than pairing
an earlier event payload with the current right cache. A right-side notification also
checks the current left lookup key before using its reverse-index entry: a pending left
event may still need to repair that entry after key reassignment.

Flattening a newly created group bootstraps its current members before subscribing to
future member events. Delayed delivery of the header must not lose the first member.

## Boundaries and costs

- Public APIs and per-mutation immediate reads are unchanged. No public batch primitive,
  VM, compatibility path or replacement collection engine is introduced.
- Projectors remain pure functions of their declared inputs. Arbitrary reactive reads
  are not dependency edges; existing diagnostics still apply.
- Work remains incremental over affected events and matching join rows. Queue insertion
  is O(log D), where D is the number of queued consumer depths. Queue storage lasts only
  for the synchronous drain. No list-wide snapshots are added to normal map/join updates.
- A join may project the same final pair more than once when both inputs notify it;
  equality cutoff suppresses unchanged output. Exactly-once projection is not promised.
- Queued callbacks retain their observer until delivery, but existing owner-scope checks
  prevent delivery into retired producers. The queue is owned by its runtime.
- Rust 1.100's `std::mem::DropGuard` resets delivery state and releases abandoned closures
  on unwind, outside the queue's mutable borrow. This is resource cleanup, not rollback
  or a promise that a pipeline whose user callback panicked can safely resume.

## Reference decisions

Reviewed local Exact `08c889a9de21a30b7c3b308422fc588cf1a480d3` and Rune
`3686e94ae09545fd5f2a61e22aabfbeba1cd896f` sources.

Exact's `host/web-js/rt.js` freshening and runner `settlement.rs` reinforce the rule that
inputs must be settled before a consumer computes, with equality cutoff and owned
cleanup. Octara does not adopt Exact's buffered action writes: its collection writes
remain immediately readable.

Rune's commit/measure flow and versioned memo reuse reinforce a consistent state before
rendering and explicit reuse boundaries. Rune does not provide this collection queue
algorithm; its application-state/commit model is different from Octara's signal graph.

## Verification

`tests/integration/list_consistency.rs` checks equal and unequal depth diamonds,
filtered removal/re-entry, lookup reassignment, chained joins, and group creation with
multiple events. Assertions observe projector arguments as well as final direct/memo
reads, exercise both registration orders, and bound per-row computation counts.
Existing randomized collection/log-replay tests check composed operators against
from-scratch results. A runtime unit test checks pending callback release on unwind.

Validated on Rust 1.100.0 (2026-11-16): `cargo test --workspace`,
`cargo clippy --workspace --all-targets -- -D warnings`, formatting and diff checks passed.
This round validates the core deterministically; it adds no native-window qualification.

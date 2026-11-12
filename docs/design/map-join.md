# Collection operators: `map` and `join`

Based on the [research](../research/map-join.md). Status: confirmed; implemented.

Both are stages like `filter`, `sort_by` and `group_by`: they observe an upstream stage, keep
their own state incrementally, and are stages themselves, so they compose
(`todos.join(&tags, ..).filter(..).sort_by(..)`).

## `map`

- `source.map(|t: &T| -> U) -> Derived<K, U>` (on `List`, `Derived` and `Source`, as `filter`),
  `U: Clone + PartialEq`. Same keys, same order.
- Each key has its own output signal; an upstream insert or update calls the closure once for
  that key, and an update reaches downstream only when the new output differs (`PartialEq`).
  Moves pass through. Order and `index_of` are the upstream's: no ordering structure of its
  own, O(1) plus one call per event.
- The closure is a pure function of the item, run untracked, as `filter`'s predicate (see
  "View closures" below). A row's reactive scope stays `each`'s.
- `flatten` keeps its own entry per item: a `map` output is also one signal per key, so
  rebuilding `flatten` on it would not save the second copy (reactive core, known limitation).
  Removing it needs read signals that project another signal on read: a later design.

## `join`

- A lookup join: `left.join(&right, |l: &L| -> RK, |l: &L, r: Option<&R>| -> U)
  -> Derived<LK, U>`. `right` is any keyed stage (`List`, `Derived`, `Source`) whose key type
  is `RK`, so each left item matches at most one right item.
- Output keys and order are the left side's. A left item whose right key is missing is kept,
  with `None` (a left outer join); when the right item appears or goes, the row updates, it is
  not inserted or removed, so its UI state survives. An inner join is a `filter` on the output.
- The third closure picks what the row needs from both sides, so a right change that leaves
  that projection equal updates nothing (renaming a user's avatar does not touch rows that
  show the name only).
- Cost: a left event is O(1) (one lookup, one index update); a right event updates the m left
  rows that point at its key, through a reverse index (right key → left keys, absent keys
  included). Memory O(|left|). A right change computes the output of each of those m rows
  to compare it (`bench -- --joins`: 500 rows in 186 µs renamed, 72 µs for a field they do not
  show, release build).
- Many-to-many (todos ⋈ tags) is a link list keyed by the pair, joined to each side; no
  operator of its own yet.

## View closures

- The closures of every view (`filter`, `sort_by`, `group_by`, `map`, `join`) run untracked:
  a view reruns per item event, not when a signal read inside it changes. Reading a signal in
  one is a mistake that silently goes stale, so it is reported: `VIEW_READS_STATE` (warning,
  at the closure's call site). The fix: put the value in the item, `join` the data, or choose
  between views in a memo (`memo(move || if active.get() { open.source() } else { all })`).

## Diagnostics

- `VIEW_READS_STATE` (above).
- `JOIN_FANOUT` (warning): one right-side change updated more than 10 000 rows; the cause is a
  hot key (one tag on everything); the fix is a narrower projection, or joining after
  `virtual_each`'s window instead of before.

## Tests

- Proptest: random inserts, updates, removes and moves on both sides, compared after every
  event with the operators computed from scratch (as the existing views are).
- Consistency-checked apps (`App::set_check_consistency`) using `map` and `join`; bench rows for a right-side fan-out and a `map` over
  100 000 items.

## Steps

1. `map`, `VIEW_READS_STATE` for every view.
2. `join`, `JOIN_FANOUT`; the `inbox` example (messages joined with their senders; the contacts
   example keeps its shape for the benchmarks).

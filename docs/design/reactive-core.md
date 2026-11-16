# Reactive Core

Status: implementation baseline in `crates/octara/src/reactive/` and `crates/octara/src/view/`.
Research material is in [`docs/research/`](../research/).

The selected replacement is [Signals redesign](signals-redesign.md), with
[runtime semantics](runtime-contract.md) and a [refactor/removal inventory](core-consolidation.md).
Those documents supersede this page's target API and ownership choices, without claiming the
entire replacement is implemented. Broader primitive restructuring and stronger named-state
reload handling remain separate follow-ups. Update the baseline
sections as each implementation gate lands; historical benchmark results below are not new validation.

The first correctness refactor now implements non-Clone state/borrowed access, in-place update,
runtime-qualified handles, independent untracking, guarded execution context and captured observer
scopes. The next API slice now exposes Signal and ReadSignal from `octara::reactive` and the
prelude, with memo returning ReadSignal and watch returning a non-generic Watch stop handle.
Graph/sequence implementation modules are private. Duplicate untracked read methods and the
public Memo wrapper have been removed; explicit samples use untrack with ordinary get/with.
Signal/ReadSignal properties use the existing binding path; TextBuffer keeps incremental edits.
The App root now owns persistent named state and root work. Signal::new and Task::detach are
removed. App::shutdown provides explicit retirement while remaining handles stay inert for new work.

## 1. Conclusion

Octara uses **fine-grained Signals**: a component runs only once, each dynamic property is a binding, and a state change recomputes only the bindings that read it.
In the prototype this choice met the requirements, provided the following four things are all present:

1. **Incremental collections**: lists, filtering, sorting and grouping are all maintained incrementally by change, otherwise every modification of collection data is O(n);
2. **Structured diagnostics**: turn the silent errors typical of Signals into numbered reports with locations;
3. **Named state**: state is stored outside components by "scope path + key", so that hot reload can preserve state;
4. **Effects separated from tracking**: `watch` tracks only the source, and the callback does not track.

Rejected alternatives: a model based on rerunning the view (Rune's implementation) needs manual `memo` or compiler support in Rust;
the `view!` macro DSL brought no measurable benefit (see Section 8).

## 2. Layers

Based on Adapton's distinction between "internal incremental computation" and "external observers":

| Layer | Primitives | Can read | Can write | Tracks |
| --- | --- | --- | --- | --- |
| Input | `state`, `state_list`, `text_buffer`, external events | — | handlers, `watch` callbacks | — |
| Derived | `memo`, collection operators | inputs, derived | No (rejected and reported) | Yes |
| Observation | property bindings, `show` / `each`, the source of `watch` | inputs, derived | No | Yes |
| Effect | handlers, `watch` callbacks | anything | inputs | No |

Compared with React: a component runs only once, so `useCallback` is not needed; dependencies are tracked automatically, with no dependency arrays; state uses explicit keys, with no Hooks call-order rules;
there is no `useEffect`, and effects use `watch`.

## 3. Graph and scheduling

Signal stores a value-local borrow cell. Only get requires Clone; with borrows the value and
update mutates it in place after checking permissions and borrows. Conflicting access reports
SIGNAL_BORROW: invalid writes do nothing and invalid reads fail. Borrow callbacks cannot return
references into the value. Mutation invalidates even during unwind; this is not rollback or a
guarantee that arbitrary user state is safe to resume after a panic.

Untrack suppresses the surrounding collector, not the execution phase. Initializers sample values
without subscribing; they cannot write existing state. Watches, task starts, prompts and command
registration reject pure contexts before executing their effects. Internal guards restore phase,
collector, scope and window on return/unwind. A memo read inside untrack still tracks its own inputs.

- Push marking (direct observers Dirty, further downstream Check) + pull evaluation; a Memo is computed on demand and cuts off when the value is unchanged (Reactively; Salsa's early cutoff);
- At flush, structural bindings come before property bindings, then `watch`, with creation order within each kind. Newly queued earlier-priority work preempts the remaining batch, so an effect that removes a region retires its descendants before their queued callbacks;
- A node is marked Clean **before running**: if it is re-marked while running (a `watch` callback writes to its own source), it runs another round;
- Dependency edges are deduplicated with an epoch mark, O(1);
- In the single-threaded runtime, handles are `Copy` and `!Send`, with generations; background results are sent back through the scheduler;
- Value destructors run after the runtime's internal borrow is released, so a value can destroy the nodes it owns during its destruction (a list reclaims its entries).
- Flush limits retain unprocessed work and report it as pending; a later input can break the loop.
  A failed observer retires itself while the remaining batch stays queued. No automatic busy retry
  or rollback is implied.

### Composed reads and effects

An equal memo result cuts off downstream value delivery, not dependency replacement. Switching
between two branches that currently produce the same result must still detach the old branch
and subscribe to the new one. Reading the memo before flush freshens its value and dependencies
without running a queued watch callback. Flushing later delivers a changed value once, or cuts
off an unchanged value. This holds in both branch directions and with or without an early read.

Watches are sequential side effects, not snapshot transactions. If watch A copies `input * 10`
into a second signal and watch B reads both signals, A-before-B can deliver only `(1, 10)`;
B-before-A can deliver `(1, 0)` followed by `(1, 10)`. A memo of `input * 2` read alongside
`input` is already fresh in either callback. The temporary second-signal value is an effect
that has not executed yet, not a stale memo. Use memo for derived values rather than mirroring
them with watch. No all-computations-before-all-effects guarantee is provided.

When an earlier watch removes a branch, its newly queued structural work preempts later
queued descendant watches. This remains true if a descendant memo was already read and
freshened before flush; disposal releases the descendant callback's captures and prevents
later delivery. Already executed callbacks are not undone.

Unit regressions in `reactive/graph/tests.rs` cover four branch/read combinations, both watch
creation orders, and watch-triggered structural disposal with a freshened descendant memo.
They assert values, computation/callback counts, released captures, empty pending work and
absence of diagnostics. This coverage preserves the current scheduler and public primitives;
it is not proof of every possible user feedback graph.

## 4. Identity and state

ReadList composition (`filter`, `map`, `sort_by`, `group_by`, `join`) checks the creating runtime
before allocating nodes, subscribing to a list or calling its predicate/key/projection. Empty
lists follow the same rule. Both sides of a join must belong to the current App; a foreign handle
reports `WRONG_RUNTIME` at the composition call. Validation samples the existing version node
without adding a dependency to a surrounding memo or binding. Dropping a handle outside App
entry remains valid; it does not authorize using the handle inside another App.

Based on Nominal Adapton: first-class names determine identity, and name ambiguity is detected.

- Each stateful scope has a name path, for example `app/window1/Disclosure[account]/open` (a window's root scope is named by the window, so
  two windows' roots never share a key); state, hot reload and the Inspector all locate by name path;
- The identity of `component_keyed(name, key, ..)` is independent of sibling order; `component(name, ..)` is named by its ordinal among same-named siblings, and `AMBIGUOUS_COMPONENT` is reported when a second one appears;
- A state's lifetime is the lifetime of its scope: when a `show` branch is hidden or a list row is deleted, the state inside is destroyed with it;
- Handles identify their runtime as well as slot/generation. WRONG_RUNTIME rejects cross-App
  access rather than accidentally reading or changing another App's matching slot.
- Computations and handlers restore their registration scope/window. Cleanup retires the scope
  before invoking callbacks and drops values outside internal table borrows; it cannot start new
  work in the retired scope.
- Hot reload: destroy and rebuild all scopes; named state stays in the store and is found again by name, and is reset and reported when its type changes (migration functions to be added later).

## 5. Incremental collections

Based on: DBSP (incremental versions of operators compose), ILC (changes are first-class values), Traceable Data Types.

- Each collection is a **stage**: ordered keys, an entry per key, a structural change log (`Insert` / `Remove` / `Move`, with the position at the time), and observers;
- Observers receive value-level events: insert, remove, update (old and new value), move;
- A view observes upstream and is itself a stage, so they can be chained: `list.filter(..).sort_by(..)`, `list.sort_by(..).group_by(..).flatten()`;
- Order is kept in an **order-statistic tree** (an implicit treap with parent pointers, subtree sizes and weight sums): position by key, insert and delete are O(log n),
  and insert position for filtering and grouping is O(log² n);
- `each` applies the change log: an insert creates a row, a remove destroys a row, a move only moves the node, and state inside the row is kept; when the data source is switched, it compares by key once;
- `virtual_each` mounts only the rows in the viewport and a buffer, row heights serve as tree weights, and converting between scroll offset and row index is O(log n); a grouped list is `flatten`ed first;
- Shared [change history](change-log.md) is capped independently of the slowest reader. A reader that misses history reconciles the authoritative snapshot; it never replays a partial tail. Surviving keys keep their rows, and absent keys retire. Entry Signals are owned by the list and reclaimed when removed or when the list is destroyed.

## 6. Text

Based on ILC's change structure. `TextBuffer` holds a string, a version number and a bounded edit log (`Append` / `Splice`). `text(live)` writes the full text the first time,
then passes edits as they are to the kernel (`append_text` / `splice_text`), so the cost of an append is O(amount appended). `TextBuffer` can be placed in list entries,
and editing it triggers no collection events. A history gap reads the full authoritative text; an unchanged prefix is appended to preserve selection, otherwise the text is replaced. Later edits resume incremental replay.

## 7. Diagnostics

Diagnostics carry a fixed code and source location, are deduplicated and counted by "code + location". All codes are registered in `octara::diagnostics` (severity, cause, doc anchor); a registered code is passed when reporting, an unregistered code does not compile, and tests ensure that the codes mentioned in source, tests and docs are all registered. Regression tests are in `examples/tests/integration/diagnostics.rs`, numbered as follows:

| ID | Error | Result |
| --- | --- | --- |
| I1 | Read in a component body and shown as a fixed value | `STRICT_READ_UNTRACKED` |
| I2 | Write during the build phase | `WRITE_IN_BUILD`, the write is rejected |
| I3 | Duplicate list key | `DUPLICATE_KEY` |
| I4 | Clamp a value into a range (using `watch`) | Behaves correctly |
| I4b | Two `watch`es write to each other | `FLUSH_RUNAWAY` |
| I5 | Use the entry of a deleted row | `READ_DISPOSED` |
| I6 | A handler captures a stale value sampled with `untrack` | **Silent** (explicitly opting out of tracking, undetectable in principle) |
| I7 | A hot patch inserts a new component before a same-named component | `AMBIGUOUS_COMPONENT`; behaves correctly under `component_keyed` |
| I8 | A helper function that reads other state is called in a `watch` callback | Behaves correctly (callbacks do not track) |
| I9 | A list row reads once and uses it as a fixed value | `STRICT_READ_UNTRACKED` |
| I10 | Unnamed public Signal construction | Rejected at compile time; no public constructor |
| M1 | Write state inside a Memo | `WRITE_IN_DERIVE`, the write is rejected |

There is also a **check of agreement with from-scratch computation** (`App::set_check_consistency`): after every operation, each window's tree is rebuilt from scratch from the current state and compared with the incremental result byte for byte.
In mutation tests it pinpoints the problem at the first erroneous event; this is the regression safeguard for all later optimizations.

## 8. Key evidence

Data is from Apple M4, `--release`, Headless; counts are exact, times are the minimum of 3 runs, and under machine load can fluctuate by up to 2x.

| Question | Evidence |
| --- | --- |
| Update precision | Toggle one item among 1,000 Todo items: 5 closures, 4 property changes; typing in the input box triggers no layout |
| Whether collections must be incremental | With the whole `Vec` as one Signal, appending a token in Chat takes 2,039 allocations; with incremental collections, 13 |
| Asymptotic cost of collections | Adding one item: 3.5 → 8.5 µs from 1k → 100k items (`Vec` approach: 8.3 → 647 µs) |
| Virtualization | 100k-message Chat with layout: 82 nodes; streaming a token in the viewport takes 13 µs (non-virtual 249 ms) |
| Grouped virtualization | 100k contacts with layout: rename 45 µs, change team 19 µs (non-virtual 62–66 ms) |
| Streaming text | With a 100 KB body, each token writes 4 bytes to the kernel (`String` approach: 100 KB) |
| Agent evaluation | Builder and DSL, 2 runs each, 4/4 passed on the first try with no diagnostics triggered; the documentation gap the Agent pointed out has been written into Section 4 of this document |
| DSL | Code size difference −22% to +8%; it can eliminate the inline form of I1/I9, but compile errors have misleading fix suggestions; not adopted |

## 9. Known limitations

- Kernel children are a `Vec`: with siblings on the order of 100,000, insert and remove are O(n); avoided by virtualization;
- Flattening builds a separate entry Signal per item (about twice the memory; see [map and join](map-join.md));
- After hot reload, virtual list rows are measured again;
- A very long paragraph without line breaks is laid out whole ([layout](layout.md));
- Tasks, subscriptions and cancellable resources are in [async](async.md).

### Interrupted incremental propagation

Collection writes and synchronous delta delivery share an internal failure boundary with
App execution. An unwind after a potentially mutating step makes the runtime unusable for
normal reads, writes and publication, even if application code catches it. Clearing the
pending delivery queue is resource cleanup, not recovery of already updated stages.
Normal pre-mutation refusals remain nonterminal. See
[runtime failure contract](runtime-contract.md#interrupted-collection-operations) for the
cleanup exception for existing source values and the no-rollback policy.

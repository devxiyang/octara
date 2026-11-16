# Collection data, row identity and content lifetime

Status: base consolidation committed as `af6fa20`; bounded-log recovery follows in the working tree, 2026-11-16.
The shared keyed-row mechanism, direct keyed Vec input, fresh row reads, structural-first
propagation, scope-owned collection producers, Chat migration and explicit prelude exports
are implemented. The follow-up [bounded change-log protocol](change-log.md) also implements
snapshot recovery for collection and text history gaps. Virtual interaction pins and any
replacement of the incremental operator evaluator remain outside these slices. No compatibility layer or second execution engine is introduced.

[Source research](../research/collection-regions.md) pins the latest inspected Rune and Exact
revisions and distinguishes their mechanisms from the choices below. The existing
[Signals contract](signals-redesign.md) continues to govern value access and permissions;
[runtime contract](runtime-contract.md) governs dispatch and retirement. This proposal changes
collection propagation and region ownership, not those foundations.

The [architecture review](collection-regions-review.md) separates required contracts from
unproven implementation choices. Same-handler row reads, invalid-input interaction handling
and producer retirement have explicit rules below. The implemented slice and test evidence are
recorded in [implementation validation](collection-primitives-validation.md). Broader prototype
gates still apply before rewriting any collection operator.

A subsequent [React 19.3 / Solid 2 RC comparison](../research/react-solid-boundaries.md) reduces
the public model further: ordinary Vec inputs go directly to keyed list bindings. ReadList is
an optional incremental capability, not the mandatory representation of every derived Vec.

## Why this needs a replacement boundary

Three independent problems currently meet in Chat:

1. `answer` copies `Message.parts: Vec<Part>` into a writable `StateList`, then maintains it
   with `watch`. This is a second update protocol for the same business data. Its current loop
   handles insertion/update, not deletion/reordering; position keys fit its current append-only
   use but cannot express general item identity.
2. `each` and `virtual_each` retain a row when a replacement list contains the same key, but
   retain the original source's item handle too. A focused headless reproduction switched key
   `1` from text `old` to `new`: the Agent target stayed stable, but the text stayed `old`.
   This was an experimental failing test, not a new failure in the committed suite.
3. `part` samples its enum once when building. Text and reasoning content capture that version
   of `TextBuffer`; reassigning a different buffer or enum variant under the same row key does
   not replace the captured resource. Adding stable IDs alone cannot repair that binding.

Relevant baseline: `crates/octara/src/reactive/list.rs`, `crates/octara/src/view/list.rs`,
`apps/chat/src/model.rs` and `apps/chat/src/ui.rs`, at Octara `b6a3eae`.

Moving the watch into a constructor would hide the first problem and leave the other two.
The replacement must define ownership, propagation and replacement together.

## One data authority, three view responsibilities

```text
Signal<ordinary Rust data> --> closure / memo --> keyed list binding
StateList --> ReadList --> private delta input --> same keyed list binding
                                                      |
                                   child scopes + current item reads
                                                      |
                                property bindings / keyed child content
                                                      |
                                            retained Kernel nodes

TextBuffer edits ----------------> current text binding
```

One dependency graph determines what needs updating; one scope tree determines what must end.
These responsibilities do not introduce new public coordinator, input, producer or Region types.
Keep `Signal`/`ReadSignal` as the value API. A normal reactive Vec with explicit `(key, value)` pairs is enough to use `each`. Shared/expensive projections can be ordinary memoized values;
a reusable collection object is not required just to render them.

Keep StateList/ReadList as an explicit storage/read-capability choice for large, independently
edited keyed collections. Their delta input and a Vec's snapshot input share the private keyed
row implementation, not a mandatory public collection engine. A snapshot/index cache inside a
list binding is allowed; an application-owned writable StateList mirror is not.

Property bindings update existing nodes. Structural bindings (`show`, `keyed`, `each`) own child
scopes and use the same graph/scope infrastructure. `virtual_each` adds residency and measurement
policy to keyed rows. This is a common lifetime/update discipline, not one generic diff engine
forced onto branches, text and large lists.

| Owner | Owns | Does not own |
| --- | --- | --- |
| Application state | Business values, durable item keys, request lifetime | Mounted nodes |
| Snapshot computation / incremental input | Read-only result/cache, order, dependencies and optional change cursor | An independent writable copy of business state |
| Row coordinator | Key-to-mounted-row mapping and current row inputs | Business mutation policy |
| Row scope | Local UI state, bindings and handlers while the row exists | Permanent offscreen application state |
| Content scope | One current enum branch or resource-specific subtree | The enclosing row's identity |

A derived cache may store values. It is not a second authority: applications cannot write it,
and its freshness follows the reactive graph rather than a watch callback.

## Collection freshness has one contract

There are two input strategies with one output contract:

- **Snapshot input:** a pure tracked closure produces `Vec<(K, T)>`, pairing each item with its
  business key. A sealed `IntoRows` conversion accepts that ordinary value or `ReadList`;
  applications do not construct an adapter or another reactive collection. It can read ordinary structs, enums and `Vec`s inside Signals. Recompute
  after a dependency changes; validate keys before publishing the result. An arbitrary new
  snapshot requires at least an O(n) key scan. No sublinear replacement claim is made.
- **Incremental input:** `StateList` records accepted edits. Collection operators consume those
  edits and preserve useful filter/sort/group/join indexes. Ordinary per-item changes must not
  degenerate into whole-list snapshots merely to share an interface with the first strategy.

Both publish ordered unique keys and current item reads. An internal revision/cursor can avoid
rescanning unchanged input. It is generated by the runtime, never a manual application version
that must be incremented correctly for the UI to stay fresh.

Collection reads bring required upstream computations current; views do not wait for a later
watch to copy their input. Writes invalidate dependents after the mutable borrow is released.
Snapshot-derived computation participates in the reactive graph. Incremental operators must
obey the same pure-read, ownership, failure and publication contracts.

Moving the synchronous `Observer::inserted/removed/updated/moved` network into graph-scheduled
collection computations is a candidate implementation, not a prerequisite established by the
source-switch bug. First prototype a mixed snapshot/incremental chain and a two-input join.
Verify read-after-write, diamond consistency, disposal, callback counts and changed-item work.
Retain a specialized internal delta evaluator if it satisfies the contract more simply; it
must not independently run UI work, effects or user callbacks after owner retirement. Only
replace the scheduling implementation when that comparison establishes a concrete benefit.

Each operator keeps its current incremental algorithms where they are useful. Its cache and
pending edits are private implementation details; user predicates/projections run in the pure
execution context. Item-only operators remain item-only: changing their ambient dependency
semantics is outside this redesign. Stateful filtering is expressed by a tracked snapshot
computation, rather than silently accepting an untracked external Signal in an item predicate.

Do not introduce a second global scheduler, a transaction language, arbitrary mutation rollback,
or a public change-event bus. At UI propagation, enclosing structural work must settle before
dependent mounted-row/content/property work runs. Newly queued structural work must also run
before those descendants; sorting one batch by creation order is insufficient. If an ancestor
retires a scope, discard that scope's queued work. Reuse runtime scope ancestry and pending-work
handling; verify this dependency before adding a new scheduling abstraction.

Public surface target: direct reactive Vec + explicit key + row builder, and an incremental
ReadList input where needed. Final function signatures/private input adapters are deferred to
compiling the ordinary-Vec and Chat examples. Do not require applications to call a new derived
collection constructor or implement a collection trait. Ordinary filter/map/sort on small Vecs
can remain Rust operations in a read closure or memo. Incremental operators retain their useful
specialization; returning a read capability can remove the separate public Derived holder
without removing its scope-owned computation. Groups stays only for actual grouped data/member
access, not as another generic ownership wrapper.

## Row identity and current input are separate

Use a single internal row coordinator for `each` and `virtual_each`. It owns the mounted row
scope and a stable read-only input for that row. A row builder receives the key and this input;
it must not receive a permanently captured read handle belonging to one upstream list.

On each published collection revision:

1. Validate the source runtime/liveness and unique keys before applying membership changes.
2. Match keys within this region. Retained rows keep their scope and mounted identity.
3. Rebind every retained row whose upstream item source changed, even if the key and value
   compare equal. Its next read follows the current source; the old source loses its dependency.
4. Apply insertions, removals and order changes. Retire removed scopes before stale handlers or
   tasks can run. Publish dependent property work from the resulting inputs.

The row input is a pure read of the **current selected collection and key**, not a value copied
only when the coordinator runs. If a handler selects source B instead of A and reads its row
again while that key exists in B, the read resolves B immediately. It establishes dependencies
on the current selection, relevant membership and current item, detaching the old dependencies.
An implementation may cache this computation with the existing reactive machinery; it may not
use value equality to skip changing the dependency route.

If the key is absent, the input is invalid, or the row scope has already retired, the read
reports an invalid access and fails the operation under existing read-failure semantics. It
never returns the last value or manufactures a default. Code needing optional membership asks
the model/collection for that key rather than reading a known-removed row. In particular, after
a handler deletes its own item it should not keep reading that row input.

Reading brings data current only: it does not mount/dispose rows, run cleanup or recursively
flush UI. Mounted identity still follows the coordinator's accepted membership. Thus a read
can observe a new value before it is painted, exactly as an ordinary Signal read can. Verify
this rule, including absence and recovery, in the API prototype before fixing signatures and
generic bounds. No public proxy/lens or extra generation argument is needed.

The private row-input indirection can use the existing Signal/memo machinery. It is not a
public mutable row model. A prototype must show that a source switch detaches old subscriptions,
that repeated reads are current, and that the indirection is released with its row. Do not add
a universal proxy/lens system to avoid a measured-small value copy.

Snapshot production may require cloning owned items; expensive values can use ordinary `Rc`
handles. `Signal<T>::with/update` still supports non-Clone complex data. This does not promise
zero-copy arbitrary borrowed iterators that outlive a Signal borrow.

Dependency granularity is the Signal, not an automatically inferred Rust struct field. Reading
`message.parts` through a Message Signal subscribes to that Signal, so a status-only write can
recompute the parts projection. Equality cutoff or deliberately split state can reduce that
cost when needed. TextBuffer edits are independent and must not write the Message merely to
announce a token; this is what keeps streaming off the collection-reconciliation path.

| Change | Required result |
| --- | --- |
| Same key, updated scalar fields | Same row scope; affected properties see current fields |
| Same key, different upstream list | Same row scope; row-input reads resolve the new input; handlers must resolve their current model/action owner |
| Reorder/prepend | Same retained row scopes; handlers act by key, never a captured index |
| Key absent in a published result | Scope retired; old Agent target and row handle cannot act |
| Key returns after published absence | Fresh row scope and target |
| Unchanged value after source switch | Presentation may be reused; input still rebinds |

For row identity, published membership means a result actually accepted by that row coordinator.
Removing and reinserting the same key before the coordinator observes the absence is equivalent
to replacement in a final `Vec`; it does not promise a remount. Incremental input follows the
same rule when coalescing edits or recovering from a truncated log. A derived read inside an
update makes data current but does not itself publish membership to every consuming view.
To demand a new logical item, use a new business key. Once the coordinator accepts a removal,
it retires the old row before accepting a later result in which that key returns. This makes
snapshot fallback sufficient without a new public incarnation or historical tombstone protocol.

Keys are unique within a region, not global object IDs. Chat needs a durable `PartId` within a
message; tool protocol IDs remain protocol data. Display strings are diagnostics, not an excuse
to replace typed key equality with formatted-string equality. Framework scope/generation and
Agent target checks remain internal. Applications do not manage new epochs for ordinary rows.

Rebinding a read cannot rewrite arbitrary captured mutation handles. A retained handler that
captures list A and calls `A.remove(key)` will still address A after a source switch. Handlers
must resolve the current action owner/model through live application state when they run. If
keys are only unique within a conversation, changing conversations must change the enclosing
keyed boundary (or use a composite business key). Reuse across sources is valid only when the
key denotes the same logical entity within that region; equal integers alone do not establish
that contract. Do not automatically introduce two-way writable row proxies to conceal this.

## Content replacement is a child lifetime boundary

A stable row is not a promise that every child resource stays unchanged. Use the existing keyed
content concept to own one replaceable child scope. Refactor its implementation together with
the row coordinator where lifetime operations overlap; do not create a second reconciliation
engine under a different public name.

For Chat, derive the content discriminator from the current `Part` variant and the identity of
any resource captured by the subtree, such as its `TextBuffer`. When that discriminator changes,
retire the old content scope and build the current one. Preserve row-level state outside this
boundary; content-specific state is intentionally reset. Ordinary status changes use property
bindings and do not rebuild the entire row.

Choose the smallest replacement boundary. A Reasoning or Tool disclosure's open state belongs
outside its buffer-specific text subtree, so replacing the buffer under the same part/type
does not collapse the disclosure. Changing the part variant can reset variant-local state.
Persistence across actual virtual-row eviction is a separate model-owned choice.

Bindings that can switch resources directly should do so, detaching their old subscription.
Explicit content replacement is for constructors that capture a resource or produce a different
subtree shape. Do not include every text revision in the discriminator: appending to the same
TextBuffer must update its existing binding. Nor can the runtime detect arbitrary values frozen
by `untrack`; use an explicit binding or boundary for values that must remain live.

Row lifetime and content lifetime also determine task ownership. Presentation work dies with
its content; an ongoing model request belongs to the conversation/request owner, so scrolling
its row offscreen cannot cancel generation.

## Virtualization changes residency, not data semantics

Virtualization adds viewport selection, overscan, height estimates, anchor maintenance and
temporary interaction pins to the shared coordinator. It does not implement another source
switch or key reconciliation algorithm.

Only resident rows have mounted scopes. Rows needed by focus, composition or an active
interaction can be temporarily pinned with a clear release condition; scrolling alone cannot
drop an active editor. Ordinary offscreen rows are disposed. A durable expanded/selected state
that must survive eviction belongs to the model, indexed by business key. Do not retain all row
scopes indefinitely to simulate persistence.

Keep the weighted `Seq` index. Heights are valid for a key **and** its current content and layout
constraints; a new buffer, cleared text, width/font change or relevant content update invalidates
the measurement. Scroll anchors use a surviving key plus offset, with a defined neighbor fallback
when that key disappears. They are not raw indices that drift after prepend/reorder.

If measurement work is budgeted, remaining work stays pending. Resumption belongs to the current
region incarnation and ends on completion, disposal or replacement. Progress measures missing
rows in the target window; it cannot count unrelated rows between the old and new windows.
Do not copy Rune's numeric budgets without measurement of Octara's workload.

Collection edit history uses the shared [bounded change-log protocol](change-log.md). The cap
is independent of stalled readers. A history gap explicitly requests a current snapshot;
collection readers reconcile current keys while text readers restore authoritative content.
Overflow cannot silently skip an unknown prefix and replay only the remaining tail. Normal
readers retain incremental behavior. The protocol is private and introduces no public
incarnation, tombstone or snapshot-adapter objects.

## Lifetime and update boundary

Keep runtime-qualified handles, generation validation, pure/effect separation, App entry and
scope retirement. Removing the public Derived wrapper does not remove its producer owner:
construction registers the producer in the current scope, and scope retirement disconnects
its dependencies and disables future evaluation before captured state is disposed. ReadList
clones do not extend that execution lifetime. Create intentionally shared producers in the
App root; reading or composing a retired producer is rejected before running user closures.

A retained handle can keep cache storage until final release, which frees value-owned graph
nodes in their creating runtime even outside App entry or while another App is current. It is
not a promise of a readable frozen result: items can contain live TextBuffer/Rc handles and
cannot be deep-frozen generically. Weak owner references must not keep the App alive. Preserve
the existing release/runtime-ownership tests and add producer-retirement tests with an upstream
source that remains live in another scope. This deliberately specifies the new lifetime API;
existing release tests alone do not establish it.

An input update has this logical order, with pull evaluation inside the graph rather than a
new global pipeline object:

```text
accepted model edits → current derived data → membership/input rebinding
                    → content/property work → layout/presentation
                    → external observation of the settled result
```

Watch remains for effects. An effect may cause another bounded settle round before output is
declared settled. No user callbacks run under graph-table/collection mutable borrows. Duplicate
keys invalidate a computed candidate; this does not roll back the application's invalid state.
Internal evaluation records an invalid result with its dependencies, so the coordinator can
observe that failure and recover on new input; duplicate validation must not throw away the
producer as though its Rust callback had panicked. Public collection reads fail instead of
exposing cached valid data. At reconciliation, retire
the affected region's rows and their targets, cancel their active interactions through normal
scope/window cleanup, and expose the diagnostic with no actionable rows. Keep the coordinator
subscribed so a later valid result can mount fresh rows with fresh targets. Parent/unrelated
regions can continue; arbitrary callback panics still follow the existing runtime failure policy.

An incremental insertion rejected *before changing valid source data* is different: it leaves
the source and its existing rows valid, so it needs no region retirement. Do not reuse targets
from a failed snapshot when recovery mounts its keys again. Source validity and pending ancestor
structure must be resolved before another row event dispatch; target identity alone does not
prove that its business mapping is valid. Pending/failed work remains visible to inspection
instead of being reported as settled. No model rollback, per-handler version lock or general
transactional recovery layer is introduced.

## Replace, retain and remove

| Area | Decision |
| --- | --- |
| `reactive/graph.rs` | Retain Signals, memo cutoff, guards and ownership; integrate snapshot derivation and enforce ancestor structural dependencies |
| `reactive/list.rs` | Remove public Derived wrapper while retaining explicit producer ownership; validate the private delta evaluator before deciding its scheduling replacement |
| `view/list.rs` | Replace separate each/virtual_each row maps and source-switch logic with one coordinator and stable row inputs |
| `view/mod.rs` keyed content | Reuse the public concept; align child retirement/replacement with the new coordinator |
| `reactive/seq.rs` | Retain ordered/weighted indexing, test through both input strategies |
| `TextBuffer` | Retain separate incremental edits and resource identity; audit log resync independently |
| Chat | Remove writable parts mirror/watch and positional identity; add logical part keys and explicit replaceable content |
| Inspector | Extend existing selected-node evidence with current key, binding/rebuild reason and pending region work where needed |
| Compatibility | Remove superseded APIs/callers together; no aliases, old engine flag, VM, general Store DSL or replay database |

## Delivery gates

The implementation can land in reviewable slices, but every slice follows this ownership model.
Do not ship the withdrawn adapter as an intermediate public API.

1. **Executable semantics and API prototype:** reproduce source switching under equal keys in
   both list modes; settle same-handler row reads, invalid-input actions/recovery and producer
   retirement. Specify same-key value/resource/variant changes, published removal and fresh
   reinsertion. Verify current reads and stale-source detachment, not just final screenshots.
2. **Collection computation:** implement tracked snapshot input and prototype integration with
   incremental operators. Test read-after-write, dependency replacement, filter/sort/group/join
   equivalence, disposal, wrong runtime, duplicates and bounded-log fallback. Compare with full
   recomputation and current incremental work before choosing a scheduling rewrite.
3. **Shared row/content ownership:** migrate both list modes and content replacement. Verify
   state/target retention, fresh targets after retirement, handler keys, retired async work and
   no updates from old buffers. Remove old paths in the same slice that replaces them.
4. **Chat migration:** ordinary `Vec<Part>` remains model data; no UI write mirror. Cover text,
   reasoning, tool and file transitions, reload/persistence and request replacement. Token append
   must not rescan parts or rebuild rows; structural edits are allowed to scan the small parts Vec.
5. **Real-window and cost acceptance:** stream while reading history, prepend/reorder, replace
   and clear content, resize, focus/select, return to latest, reload and reopen. Inspect targets,
   text and scope counts as well as GPU captures. Compare pre/post mounted rows, recomputations,
   allocations and layout work for representative Chat and large-list workloads. Native OS IME
   qualification is separate from semantic input in a real window.

Prototype gates can change a private implementation if its complexity or measured cost is too
high. They cannot weaken current-input freshness or retirement semantics. Research alone is not
proof of performance, platform behavior or completed implementation.

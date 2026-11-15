# Signals: a small primitive with explicit runtime boundaries

Status: selected breaking API contract, 2026-11-15. The first correctness slice implements
non-Clone values, in-place mutation, phase/tracking separation, runtime identity and captured
observer scopes. The second slice implements the two handle types, watch stop handles,
direct property inputs, public module boundaries and unified untrack sampling. The third slice
implements App root ownership and shutdown, removing Signal::new and Task::detach;
see the [reactive baseline](reactive-core.md).
This replaces the preceding field/Model-heavy proposal and the blanket public-untrack removal
proposal. There is no compatibility requirement: migrate replaced APIs and their callers together.
Keeping a useful name or mechanism is a design choice, not a compatibility layer.

[Consolidation](core-consolidation.md) is the refactor/removal inventory; [runtime contract](runtime-contract.md)
defines dispatch, scope teardown, hot reload, async retirement and failure. No VM is introduced.

The [scope decision](core-consolidation.md#scope-and-necessity) distinguishes required semantics
from preferred API choices. The first round establishes value access, dependencies, lifetime and
propagation correctness. The full API shape below is a target, not a requirement to replace every
wrapper, expose Watch::stop or add direct property conversions before that round can finish.

## What the references contribute

| Reference | Adopt | Boundary |
| --- | --- | --- |
| Solid | A value with read/write access, dynamic subscriptions, separate derived computations | Store/proxy behavior is above Signal; JS equality is not Rust structural equality |
| Rune | Ordinary structs, enums, borrowing and mutation; optimize specific workloads | Do not adopt root-view reruns or manual subtree versions beside Signals |
| Exact | Instance-owned state, controlled update boundaries, retirement of obsolete work, direct inspection | Do not import its Plan/VM, checkpoints or rollback guarantees for arbitrary Rust code |

Evidence: [Solid source review](../research/solid-signal-boundary.md),
[Rune research](../research/rune.md), [Exact runtime review](../research/exact-runtime-simplicity.md).
Rune's local baseline is 3686e94; Tracked/region observations come from the separately pinned
upstream baseline recorded in the research. Exact's local commit and component contracts were
read as source, not executed as platform qualification.

## Four responsibilities

| Part | Responsibility |
| --- | --- |
| Signal | Store one value, provide read/write access, record reads and invalidate dependents |
| Reactive graph | Compute memo/bindings, update dependency edges, queue observers, equality cutoff and loop diagnostics |
| Scope | Own state/computations/cleanup and attached UI work; retire them on disposal |
| App | Safe UI-thread entry, ready tasks, clock, bounded propagation/layout and compatible output |

Signals do not contain widget models, field paths, native protocols, Agent transport or request
state machines. Named state construction, source locations and hot-reload identity belong to the
scope/runtime layer. Generation and runtime checks remain internal correctness mechanisms.

## User API

Ordinary use:

```rust,ignore
let count = state("count", || 0);
let doubled = memo(move || count.get() * 2);

col()
    .child(text(move || format!("Count: {}", count.get())))
    .child(text(move || format!("Double: {}", doubled.get())))
    .child(button("Add").on_press(move || count.update(|n| *n += 1)));
```

Two graph handles suffice: Signal<T> is writable; ReadSignal<T> is read-only. memo returns
ReadSignal<T>, as does Signal::read_only. Handles remain Copy and UI-thread-only; copying a
handle does not copy its value or keep its scope alive. Remove the separate public Memo wrapper.
Both handles validate runtime identity plus slot generation before access, including private
disposal. Equality of handles means the same allocation identity, not equality of T or proof of liveness.
They are !Send and !Sync; background workers send ordinary owned values to the UI thread.

| Operation | Meaning |
| --- | --- |
| get() | Owned read, requiring T: Clone |
| with(f) | Synchronous borrowed read; no Clone bound |
| set(value) | Replace the value |
| update(f) | Mutably borrow the value for a synchronous callback, then invalidate |
| read_only() | Read capability for the same source |

The target signatures require T: 'static for state, Signal and borrowed operations; only get
requires T: Clone. memo requires T: PartialEq + 'static and returns ReadSignal<T>, without Clone
on construction or borrowed reads. update takes FnOnce(&mut T) and returns (); with takes
FnOnce(&T) -> R. Callback lifetimes prevent a reference into the value from escaping. Neither
Signal nor ReadSignal exposes Deref, DerefMut or a mutable guard that could defer notification.

Keep the supported exports in octara::reactive and the prelude. graph and seq become private
implementation modules; consumers do not depend on arena layout or scheduling types.

Creation, with, set and update require owned 'static data, not Clone/Serialize/Debug. update
mutates in place; it must not clone the whole struct. References cannot escape with/update or
cross await. An active conflicting borrow is diagnosed. Validate write permission before invoking
an update callback. Acquire value-local storage through a short graph lookup, releasing the graph
table borrow before user code. Release the value borrow before running observers. On unwind,
restore context/borrows and invalidate possibly modified state; abort the operation according to
the runtime's failure policy. No rollback of arbitrary Rust mutation is promised.

Source storage can use a private, type-erased value-local cell obtained through a short graph
lookup. A borrowed read permits other immutable reads; a conflicting write is rejected before
invoking its mutation closure. update does not itself subscribe the surrounding observer to the
old value. No observer runs while its mutable borrow is active. A read or write to another signal
does not borrow the entire graph's value storage. Replacing/dropping old values also runs Drop
outside graph-table borrows. These rules apply to non-Clone values and to destructors owning
incremental collection nodes, not only scalar tests.

Invalid reads (wrong runtime, disposed handle, borrow conflict or unavailable cyclic memo) report
a registered diagnostic and fail the operation; they never fabricate a default or silently return
a cached cyclic result. Invalid writes report and perform no mutation; update's callback is not
called. Existing panic-based reads may remain the failure mechanism under the runtime failure
policy; adding a parallel family of try_get/try_set APIs is outside this slice.

For the first implementation, an accepted set/update invalidates the source once even when its
value is unchanged. This conservative rule works for arbitrary T without equality or a cloned
before-image. It differs deliberately from Solid's default === comparison. memo requires PartialEq
and stops propagation of equal derived results. Do not add several equality/quiet/force write APIs
until a concrete use case justifies them.

An unchanged source write can therefore rerun a directly subscribed binding, whereas an unchanged
memo result cuts off its downstream work. A property sink may independently skip an identical
Kernel value. Mutations coalesce through graph dirtiness, so several writes need not run each
observer several times. This is a deliberate initial tradeoff, not a claim of Solid-equivalent
equality or field-level tracking.

state(key, init) is a runtime convenience around allocation: current scope plus an explicit name,
lazy initialization, and reuse of compatible named state on hot reload. App initialization also
has a scope for shared state. Keep raw allocation private; applications use state for owned state.
The key is not required by the dependency algorithm itself.

Initialization is lazy and runs only for a new/reset value. Component duplicate keys are diagnosed;
the current baseline retains one value for same-type duplicates. App root names are persistent
lookups: repeated same-key/type calls reuse state, while type conflicts reject without replacing it.
Component key/type matches preserve eligible values across an explicit hot reload; normal removal
and re-addition creates fresh state. Stronger target value transfer does
not preserve old mounted-scope handles. Ordinary unscoped construction is rejected; App::run
supplies an App root scope for application-wide state. Signal::new is removed from the public API;
private scope-owned and value-owned allocation remain for framework internals.

## Tracking, effects and untrack

Execution phase, active dependency collector and current owner are three separate dimensions.
All have unwind-safe guards; changing one cannot silently relax the other two. Phase restrictions
also cover StateList, TextBuffer and effectful registration, not just Signal::set.

| Execution | Dependency behavior | User mutations/effects |
| --- | --- | --- |
| Component build | Ordinary live-state reads diagnose a frozen value; binding closures track later | Allocate owned state and register bindings/watches/handlers; no ordinary state writes |
| State initializer | Sample initial values without subscribing the outer observer | Construct the new value; no writes to existing reactive state or effectful escape |
| Memo / binding expression / watch source | Track actual reads and replace conditional dependencies on each run | No state writes, task starts or watch registration |
| Incremental list selector/comparator | Item-based pure function; ambient state reads diagnose missing reactive dependencies | No writes or effectful registration |
| Event / watch callback / task poll | Untracked current reads | Write live state, start scoped work |
| Cleanup | Untracked reads under retiring owner | Release resources, update live ancestor state; no new work in retiring scope |

Structural bindings may enter a framework-owned mount operation that creates a child scope and
runs its build body. This is a narrow internal capability, not a public way for arbitrary derived
closures to become builders or handlers. Internal list/text propagation similarly gets narrow
storage operations, not a general permission-bypass boolean available to application code.

Read tracking and lifetime ownership are independent. Read within a memo, binding or watch source
registers a dependency. Ordinary handlers, task polls and watch callbacks read current values
without subscribing. State initializers deliberately sample initial values; retained state does
not reinitialize merely because an input changes. Ordinary construction-body reads that freeze
changing UI still receive a diagnostic.

Keep one public untrack(f) utility. It temporarily disables collection into the surrounding observer
and restores it afterwards, including on unwind. It does not change the execution phase, owner,
write permission, notification behavior or time. A memo read inside it still maintains that memo's
own dependencies. Remove duplicate public get_untracked/with_untracked methods; callers needing
an explicit snapshot compose get/with with untrack.

This is a positive selection of a small composable primitive, not preservation of an old API for
compatibility. Do not replace it with a general public initialization wrapper or numerous special
sampling hooks. Routine events and initializers need no explicit untrack. Pure derived values
normally track every changing input; opting out changes semantics and can deliberately leave
results unchanged when that input changes.

watch(source, callback) -> Watch remains the normal side-effect API: source determines triggers, callback
samples any other values and may write. First invocation is immediate with no old value; equal
source results do not repeat the callback. The source result needs PartialEq + 'static, not Clone;
keep the previous owned result and borrow old/new values for the callback. Scope disposal stops
the watch, while ignoring/dropping the stop handle preserves scope-owned watching. Registering effectful work from
pure computations is rejected before the first callback. Nested initialization or untrack cannot
escape a pure-write restriction.

The callback shape remains Fn(&S, Option<&S>) + 'static; the returned Watch has no value type
parameter and carries only stop capability. New diagnostics for ownership/borrow/phase violations
must be registered with cause and documentation, rather than emitted as ad hoc error strings.

Watch::stop is idempotent, detaches dependencies, cancels queued invocations and releases retained
callback/results when safe. Stopping during a callback cannot preempt its current Rust execution
but prevents subsequent invocations. A callback writing its source schedules another bounded round;
it never recursively calls itself. Registration during an allowed build may execute the initial
callback in its explicit effect phase; registration from a pure expression is rejected first.

Memo remains lazy and caches its result. Recompute stale upstream memos before downstream reads;
remove obsolete conditional edges; preserve notifications raised during an observer's own run.
Subscriptions use registration ownership, independent of which window happened to trigger a flush.
Dynamic dependency correctness and a diamond dependency are acceptance tests, not a requirement
to rewrite the existing push/pull algorithm.

## Complex data stays ordinary

```rust,ignore
#[derive(Default)]
struct User {
    name: String,
    age: u32,
    settings: std::collections::HashMap<String, bool>,
}

let user = state("user", User::default);
let greeting = memo(move || user.with(|u| format!("Hello, {}", u.name)));

// In an event:
user.update(|u| {
    u.age += 1;
    u.settings.insert("notifications".into(), true);
});
```

User need not implement Clone. get of the whole User would require Clone; with does not. Structs,
enums, tuples, Option/Result and owned nested collections are ordinary values. Signal<User> tracks
User as one source, not individual fields. A memo selecting a field can cut off equal results,
but its selector may still run after another field changes.

Current implementation already stores Clone + 'static complex values, but requires Clone too
broadly and copies the whole value in update. This proposal changes those costs and bounds; it
is not introducing complex-value support for the first time.

Mutating a separately held Rc<RefCell<_>> does not notify the containing Signal. Mutations must
use the observable owner or a specialized observable object. Supporting a struct is not a promise
to intercept all reachable memory changes.

Keep StateList for large keyed changing collections and TextBuffer for incremental text. Plain
Vec/HashMap state remains whole-value reactive. StateList's current old/new-row protocol still
requires Clone for rows; removing that is separate work. Never replace its incremental operations
with whole-list diffing merely to unify types. TextBuffer handle identity and buffer edits remain
separate dependencies when the text source itself can change.

Field adapters, control getter/setter composition and a deep Store are upper-layer concerns. Keep
the existing small Kit Model<T> trait for Signal or a getter/setter pair; do not introduce a concrete
Model replacement. Do not put field(...) on Signal, require Model for ordinary state or implement
proxy/macros/lenses in this slice. Rust does
not automatically intercept normal struct-field access as JavaScript Proxy does.

Property conversion accepts matching Signal<T>/ReadSignal<T> alongside static values and closures
for existing supported property types. This is binding construction, not an eager get during build.
Formatting/conversion remains an ordinary closure; it need not be a cached memo unless computation
or equality cutoff warrants it. ReadSignal never implements a writable control capability.

## Update and lifetime contracts

An input/task result enters App, executes with its captured owner, marks affected Signals, then
flushes derived computations and observations, handles required layout feedback and prepares output.
A read of a dirty memo can update it on demand. Consecutive writes inside an event are visible to
later reads in that event; normal automatic binding/output processing waits for the App boundary.
Watch feedback is processed under finite budgets; completion of input delivery need not await
network or native animation. Cross-window dirty work cannot be lost.

Every mounted stateful instance has a scope. Reordered instances use stable keys. Bindings,
watches and handlers retain the scope/window where registered. UI tasks started from handlers
inherit that owner. Disposal retires callbacks and result interest before releasing state;
app-lifetime tasks require an explicit choice. Late external replies and reused node names cannot
restore a retired instance. These are runtime contracts, not extra Signal methods.

Exact's transactional implementation is useful evidence for coherent update boundaries, but
Octara cannot roll back arbitrary Rust callbacks or external effects. Distinguish rejected input,
delivered input with pending work and broken invariants. Inspection reads actual owners and reports
source, lifetime, dependencies/pending reasons where useful; arbitrary values use opt-in formatting,
not mandatory serialization or a permanent causal-history system.

## Implementation slices and acceptance

Use first-round obligations A-D in the [delivery plan](core-consolidation.md#delivery-gates).
Prove non-Clone value access, pre-mutation permission checks, context restoration, conditional and
diamond dependencies, equality cutoff, owner disposal and reliable propagation. Start with
reproductions of the existing failure paths; reuse the current push/pull algorithm where it works.

Two-handle API unification, early watch stopping and direct property conversions form the second
implemented slice. They were not prerequisites for the earlier correctness fixes.
When an API changes, migrate its consumers together
without aliases. Preserve incremental list/text behavior throughout.

Use Counter, a non-Clone struct, keyed rows, async search and Chat as relevant acceptance cases.
Verify affected native behavior in real windows and request ordering with local HTTP. Measure
representative workloads when their implementation changes, before adding new adapters.

No field-Model subsystem, general Store or second state engine is a prerequisite for these slices.

# Runtime architecture

Status: selected target contract, revised 2026-11-15; correctness, Signals API and App root lifetime
slices implemented, broader primitive consolidation pending. Current details are in [reactive core](reactive-core.md)
and [async](async.md); the complete target below is not a claim that every mechanism has shipped.
Rust builders, Signals, scoped tasks, the retained Kernel and GPU rendering are the implemented
foundation. Live attachment is implemented;
the remaining correctness work below is tracked separately. [Architecture](../ARCHITECTURE.md)
records what currently works.

[Consolidation](core-consolidation.md) owns the source-based refactor/removal inventory and delivery
gates; [Signals](signals-redesign.md) owns the replacement API. These are breaking changes with no
compatibility mode. No VM or separate public execution engine is part of this architecture.

These are target semantics and preferred mechanisms, not an indivisible first implementation.
The [scope decision](core-consolidation.md#scope-and-necessity) limits the first round to value
access, dependency correctness, lifetime and reliable propagation. More elaborate reload transfer,
API removal and primitive restructuring must not become prerequisites for those fixes.

Keep the application in Rust and update UI through fine-grained bindings. Give Agents direct
access to the running app through a small tool interface. The [Exact comparison](../research/exact-runtime-simplicity.md)
motivates this scope; the [theory baseline](../research/runtime-foundations.md) supplies vocabulary
for time and observation without prescribing a public identity system.

## Structure

| Owner | Responsibility |
| --- | --- |
| Reactive runtime | Signals, memos, bindings, dynamic dependencies and scopes |
| App | UI-thread entry, ready tasks, app time, affected windows and platform requests |
| Window and Kernel | Window interaction and lifetime; retained nodes, styles, text, layout, motion and semantics |
| Renderer | Display data to pixels, with derived caches |
| Platform host | Actual windows, native controls, input, pacing and available capture/display feedback |
| Inspector and CLI | Query those owners and route explicit actions; connect, launch and format results |

Keep these boundaries within the existing crates. Inspector reads authoritative data; it does
not keep a second application model. Background computation returns through tasks or channels.
Only the UI thread changes reactive and UI state.

The execution engine is the cooperation of App, the existing task scheduler, the reactive graph
and retained windows. Business callbacks remain compiled Rust. Keep these internal modules rather
than introducing a public Engine/Transaction/Plan object. Framework budgets cannot preempt arbitrary
Rust callbacks; arbitrary Rust state and external side effects cannot be rolled back.
Pure-phase guards constrain framework-visible writes and registrations. They cannot intercept
arbitrary file I/O or mutation hidden inside a Rust closure; keeping derived code free of those
effects remains an application contract, not a sandbox guarantee.

```text
input / task completion
         |
         v
 App -> Signals -> affected bindings -> Kernel
         |                              |
         +---- window work / cleanup ---+
                                        |
                                  layout and output
                                        |
                                 renderer / host

 CLI / Agent -> existing Inspector -> App or host
```

## An update

An outer App entry defines the automatic propagation boundary. App::run deliberately enters the
App root scope with no implicit window; window dispatch enters the captured callback owner. Nested
ordinary entry coalesces into the outer boundary and cannot force recursive flushing. Calling an
entry helper from a build body, initializer or pure computation must not grant write permission.
OS/transport callbacks that
arrive while dispatch is active enqueue work, except for the narrow synchronous native protocol
path described below. An explicit wait/settle is not allowed to re-enter a running callback.

1. The App accepts input or ready work at a safe UI-thread boundary.
2. Handlers and ready task polls change Signals. Writes are immediately readable; dirty memo reads
   recompute on demand. Automatic propagation happens after the current callback/poll boundary.
   The reactive runtime updates affected bindings; structures mount or dispose scopes. Kernel,
   graph-table, scheduler and platform borrows are released before user code runs.
3. Process affected windows, required layout and controlled measurement feedback under a budget.
   A later window invalidating an earlier one must preserve that dirty work and schedule it again.
   Unchanged windows and subtrees reuse existing data.
4. Complete applicable lifecycle, focus and hover repair before preparing output that depends on
   them. Prepare geometry, native placements, drawing and accessibility from compatible window
   data at the declared app time.
5. Submit output according to host pacing. Queue native notifications for later safe processing;
   transport work cannot recursively enter application handlers while state is borrowed.

This is a bounded update boundary, not an all-window fixed point over arbitrary callbacks and
future native work. Keep current incremental behavior and correct demonstrated invalidation or
ordering defects. No universal application Revision is required before live inspection works.
Budgets constrain framework-controlled polling, propagation and layout rounds; the UI thread
cannot preempt an arbitrary synchronous Rust callback.

Reactive work retains push invalidation, pull memo evaluation and equality cutoff. Each batch
processes bindings before watches, with stable registration order within a kind. Watch writes
schedule further work rather than recursively flushing; an observer invalidated during its run
must remain scheduled. A diamond dependency must not publish a memo from a mixture of old/new
dependencies. This is not atomic execution of all watches or all external side effects.

App maintains a dirty-window work set. Remove work only when its required stage has completed;
if processing B invalidates previously processed A, A is queued again. Scope open/close, focus
repair and layout feedback may create more work. Track them under explicit finite budgets rather
than repeating all windows indefinitely. Coalesce redundant work, not ordered user actions or
stream data whose consumer requires every item.

Drawing may coalesce updates and reuse display data. If native placement and deferred drawing
would otherwise use different geometry or animation times, reconcile them at the window's output
boundary. Submission and the presentation evidence supplied by the host remain separate stages.

App time advance is explicit. A read may prepare or drain bounded ready work at its documented
boundary, but does not secretly move app time or wait indefinitely. Preserve existing inspect
semantics; introducing an independent sampling clock is not a prerequisite for live attachment.

There are four distinct observations: input was delivered; framework propagation/layout is ready;
output was submitted; the host supplied presentation evidence. Internal per-window generations
may validate that drawing, accessibility and native placement use compatible geometry, but no
universal application revision is exposed. Host presentation evidence must not be inferred from
a Signal flush, frame counter or successful GPU submission alone.

The implemented [window completion path](window-completion.md) propagates unfinished layout
through preparation and reports it via the existing pending-work APIs, without a retry timer.

## Waiting and failure

An action can finish delivering while a network request or animation remains pending. Preserve
that distinction in its reply. A bounded wait or settle returns remaining work and its reason;
a timeout alone does not damage the runtime or suspend all ordinary input. Existing advance and
settle operations retain their documented clock behavior; a host declares any added native wait.

Bound reactive/layout feedback and diagnose loops. Yield or retry only where the remaining work
can safely continue. Do not submit inconsistent partial output as a completed frame. If an
internal invariant is broken and safe continuation cannot be established, stop the affected
window/runtime, invalidate its targets, and require explicit rebuild/restart before resuming it.
Rebuilding UI is not rollback of application data. Avoid a general incomplete-tree recovery protocol.

Ordinary pending work, recoverable operation errors and broken invariants are different outcomes.
Rust callbacks may already have produced effects; errors and lost replies do not authorize replay.
The framework does not promise rollback of arbitrary Rust state or external operations.

| Outcome | Required behavior |
| --- | --- |
| Rejected before dispatch | Report stale target, invalid action or permission error; do not invoke the handler |
| Delivered, work pending | Preserve queued work, schedule a wake/deadline, report the pending reason; do not replay input |
| Finite scheduling budget exhausted | Preserve resumable work and yield; do not label a partially prepared window complete |
| Reactive cycle or repeated runaway feedback | Diagnose the responsible computation and stop affected propagation until explicit recovery; do not busy-loop retries |
| User callback panics | Restore guards/borrows during unwind; retire failing work and mark the affected runtime unusable for ordinary continuation; propagate or report at an existing host panic boundary |
| Transport disconnected after delivery may have occurred | Outcome unknown; never automatically repeat a potentially effectful action |

No catch_unwind around every callback is required. Context/compute/flush/poll bookkeeping must
remain safe during unwinding, but restoring guards does not certify that user data is consistent.
For panic=abort there is no recovery promise. In-place mutation can have partially changed a value;
invalidate its dependents and block ordinary publication from the failed runtime, without claiming
that the old value was restored. App-wide shared graph corruption affects all of that App's windows.

## Lifetimes and external work

The ownership tree is App root -> window root -> mounted component/branch/row scopes. Ordinary
Rust values retain ordinary ownership; value-owned collection entries use private allocation and
disposal rather than a public second scope system. A callback captures runtime, scope and optional
window at registration/mount; the trigger's current window is not its owner. Restore the complete
owner on binding/watch execution, handler dispatch, task polling and cleanup. An explicit absent
window clears the current window; it must not mean "inherit whichever window happened to flush".

App::run establishes the root owner for shared state and intentional App-lifetime tasks. Remove
Task::detach: choose the lifetime before spawning. App work must access windows explicitly and
check their liveness. Moving a future to the root does not extend the lifetime of UI handles it
captures. Normal spawn/async_memo/interval/channel subscriptions remain scoped; ignoring their
handles does not cancel them. Task cancellation remains explicit or owner-driven.

App lifetime ends through explicit, idempotent `App::shutdown`, not Rc handle counts. The desktop
host calls it when its event loop returns, before propagating an error or exiting. Shutdown closes
the entry gate first, closes windows without flushing new work, disposes the App root and stops
the scheduler's ready queue, timers, named channels and host wakes. New App::run closures are not
called after shutdown; opening a window is rejected. Existing window handles are closed.
An active Rust invocation finishes its current stack; already-running blocking work can finish
but has no retained task interest. Pending work may still report those workers. No arbitrary user
recovery of user side effects or automatic last-handle shutdown is promised. A cleanup panic
interrupts that teardown pass; another explicit shutdown resumes the remaining work without
replaying any cleanup callback that already started.

Disposal has a defined order:

1. Mark the subtree retiring so no new input, observer callback, task poll or reply can enter it.
2. Revoke subscriptions/request interest, cancel tasks, and withdraw owned host interactions where
   supported. Already-running blocking work may finish without permission to publish results.
3. Dispose children before parents; execute each scope's cleanup in reverse registration order,
   once, with its original context available for teardown. Cleanup may release external resources
   and update still-live ancestor state, but may not start new work in the retiring scope.
4. Release nodes, values and context after cleanup and active value borrows end. Run user Drop code
   outside graph/scheduler/platform table borrows. Generation invalidation prevents later access;
   a temporarily held storage cell does not make the logical handle live again.

Retiring scope values may be sampled by their own cleanup before release; ordinary callbacks and
writes into retiring state are rejected. A cleanup panic follows the failure policy above: no
claim that arbitrary remaining user cleanup completed successfully.

Keep scopes and generation-checked handles. A node reference must still identify its owning
window/tree after destruction, reload or restart; connection/incarnation checks can supply the
outer lifetime. Names and test IDs help discovery. They never make a retired occurrence valid.

Explicit hot reload is a separate operation from ordinary removal. Retire old computations,
handlers, requests and tree targets; transfer eligible named values by scope path/key and type;
build fresh scopes, handles and subscriptions; release unclaimed values. Move non-Clone payloads
where possible. Changed types reset with a diagnostic; ambiguous keys never silently merge state.
Old UI Signal handles do not become valid for the replacement instance just because the name
matches. App-root state outside the reloaded subtree stays owned by the App.

Value retention is not a promise to rewrite handles hidden inside arbitrary Rust structs. Values
capturing retired UI handles need reinitialization; framework-owned StateList/TextBuffer restoration
must explicitly preserve their data/identity rules and reconnect valid internal ownership. No
general automatic object-graph migration or migration DSL is introduced.

A platform ask or registered external request belongs to a scope/task or an explicit app lifetime,
and depends on its host window where applicable. Retire it with its owner, withdraw the host
interaction when supported, and reject or dispose late replies. Explicit cancellation resolves a
still-live waiter; it cannot leave the waiter pending forever. Cancelling interest and aborting
transport have separate meanings.

Use additional generations where asynchronous feedback needs them. A row can keep its key while
its content or width changes, so a measurement must validate the relevant content/metric epoch.
These are local request and measurement checks, not one shared version for every subsystem.

## Agent interface

Extend [existing inspection](agent-context.md): read the tree or selected node, source and
property information, geometry, diagnostics and pending work; perform input, capture and explicit
clock/wait operations. Begin with one opt-in local connection and serialized request/reply.

Connection metadata identifies the running code and supported host capabilities. Use existing
handles with the lifetime checks above; add a narrow change stamp only for a query or cache that
needs it. No six public ID types, general resource router or operation registry is required.
Application debug values are optional and impose no universal serialization bound.

Replies identify the actual input/capture route and distinguish delivery, remaining work, failure
and unknown transport outcome. Semantic activation, framework input and OS input establish
different behavior. Capture reports what was captured and the correlation the host can support.

## Native platform behavior

GPU drawing remains the default. Native controls and platform behavior have explicit owners:
composition, selection, keyboard geometry, scrolling and navigation cannot have competing writers.
Start with the actual adapter and its tested reconciliation rules, such as deferred controlled
writeback during composition or reconciling intent after a cancelled native transition.

Queue ordinary notifications. Required synchronous text queries and protocol edits use a safe
adapter entry, without invoking arbitrary handlers under a Kernel/platform borrow. A synchronous
edit followed by a selection/marked-range query must expose that edit immediately in the adapter;
reconcile with application state at the safe boundary. Derive broader
abstractions from working adapters. Keep the winit shell while improving native text and exploring
a mobile host; mobile input and capture support must be established on the actual simulator/device.

During composition the native editor owns the draft/marked range and selection. An identical
controlled value must not reset composition. Conflicting application writeback follows the
adapter's explicit defer/reconcile policy; after composition, report the actual committed input
before considering any still-current deferred write. Do not let an obsolete pending value overwrite
a newer application decision. Target incarnation and local edit/measurement validity are checked
where feedback returns, not encoded as extra Signal methods. Candidate windows remain OS-owned;
their contents/placement require host evidence rather than framework text assertions.

Focus repair, cancellation, blur and dismissal are lifecycle work, not ordinary activation. They
must execute when a node becomes disabled or leaves the active modal region. Custom handlers may
prevent documented default actions, but cannot suppress mandatory teardown or corrupt primitive
state. Define callback/default ordering per event family in primitives; do not impose one universal
"last callback wins" rule on input, focus, editing and cleanup.

## Rust usage and optimization

Business data and services use ordinary structs, enums, traits and methods. UI-observed changes
must pass through Signals, incremental collections or an explicit observable adapter; plain Rust
field mutation does not acquire automatic tracking. The chat app's Model/Request/Event boundary
is an existing example of this separation.

Builders accept static values or tracked closures. Components compose through ordinary functions:
an instance builds on mount, then bindings update its properties.
Keep state and task ownership explicit through existing scopes, with no new application DSL or
mandatory framework base model.

| Workload | Existing mechanism to use and verify |
| --- | --- |
| Repeated derived computation | Pure memo values with tracked dependencies |
| Streaming text | TextBuffer edits instead of replacing the whole property; long-paragraph layout remains a separate cost |
| Large changing lists | Incremental collections, stable row keys and virtual_each |
| Idle or moving output | Framework-owned paint/layout caches and animation sampling |
| Native editing/navigation | Local adapter ownership and completion/cancellation reconciliation |

The useful Rune lesson is familiar Rust composition and optimizing a particular stage for a
particular workload. Most of that is already present here. Keep Signals as the default update
mechanism; do not add root-view recomputation, manual subtree versions or a public live_only
escape that can silently leave ordinary UI stale. Add optimizations after measurement and retain
from-scratch/reference checks where applicable.

## Implementation slices

The live desktop loop is an existing baseline, not a new prerequisite. Follow first-round
obligations A-D in the [delivery plan](core-consolidation.md#delivery-gates): value access,
dependencies, lifetime and propagation. Each change must reproduce a concrete case and preserve
the current real-window loop. Documentation and affected-path validation accompany each slice;
API simplification and primitive restructuring are independent later work.
Mobile remains later platform work, not an acceptance gate for this desktop runtime refactor.

Deferred extensions are general snapshot/watch, a universal resource namespace, complete causal
history and a global UI revision. Introduce a specific extension only when a reproducible tool
task establishes its need. Agent improvement uses the ordinary development loop; a separate
framework-managed learning service is outside this architecture.


## Failure boundary

Failures have three distinct contracts:

- Expected failures use the operation's existing Result, AsyncState::Failed or diagnostic
  refusal. Invalid input, absent targets and failed requests do not fail the App.
- Programming errors (foreign-runtime reads, state type conflicts, dependency cycles) remain
  panics. Signal reads do not gain a parallel family of fallible APIs.
- A panic escaping an execution boundary loses the App's permission to execute or publish.
  Restoring Rust guards does not prove that shared application state was restored.

Fallible external operations share an admission check before allocation or input delivery.
A failed App returns APP_FAILED through their existing error channel; a closed window refuses
input. Infallible execution APIs reject reuse by panic. The original callback panic still
propagates: there is no blanket catch-and-convert layer. run_until_idle returns the failure's
pending work instead of trying to poll it. Inspection and explicit teardown remain available.
This is admission, not a transaction: an admitted callback can mutate state before panicking.


A panic escaping an App or window operation marks that App failed (`APP_FAILED`).
`App::is_failed()` and diagnostics remain readable; ordinary entries, task polling and input
are rejected. Pending reports include the failure, and all window completion marks are cleared.
Shut down the App explicitly and create another one. Teardown remains available, but arbitrary
user cleanup can itself panic; neither rollback nor completion of all cleanup is promised.
A panic caught entirely inside user code before crossing an entry boundary cannot be detected
by this policy. Such recovery is the caller's responsibility. Low-level graph unwind tests
verify bookkeeping only, not permission to resume a failed App.

Internal task cleanup registrations are now released on completion or cancellation. Scope
retirement still runs surviving callbacks in reverse registration order. Registration identity
includes the scope incarnation, so late destruction cannot remove work in a reused scope.
User `on_cleanup` callbacks retain their scope-lifetime semantics.

The [reference comparison](../research/panic-boundaries.md) distinguishes this conservative
fallback from expected Result/AsyncState failures and from adapter-specific local recovery.
No panic-to-business-error conversion or automatic replay is introduced.

### Native host and Inspector

The native shell contains an unwind only when that entry newly failed the App. A panic in
the host/GPU or a later diagnostic/cleanup operation is not treated as application recovery.
After failure it stops frame demand, timers and input delivery, retains the windows for
diagnosis, and accepts an OS close request as shutdown of the failed App. It exits with failure
status and removes its Inspector endpoint. Initialization before a mounted App and aborting
panics are outside this containment contract.

Inspector snapshots expose `failed`. A failed snapshot describes the retained tree, not a
successfully settled or presented frame. Selection extraction can construct unmounted virtual
rows, so that field is omitted after failure. Only `inspect` and `issues` remain available;
input, reload, settling, resize and capture return APP_FAILED without delivery.

A callback panic returns APP_FAILED without claiming delivery. Live feedback reports
`delivery: "unknown"`; clients must not retry. Pending native operations retain facts already
established by their phases: refused before admission, delivered after confirmed input, unknown
when unwinding prevents confirmation. Each waiting response is completed once on failure;
dropping capture jobs also stops their wake sources. No callback is replayed.

Validate with `examples/live_failure.py` and the explicit `window failure` fixture. The script
requires a visible window with presented frames before injecting handler, task and binding
panics. It checks unchanged presentation counts, readable diagnostics and refused later work.
On macOS, compile `examples/live_window.swift` and pass it as `--native-helper` to also check
OS activation and native close through the specified process's accessibility window.

### Blocking worker failures

A background closure's panic must not kill a shared worker or leave its in-flight counter
permanently positive. The worker catches that closure's unwind and delivers either its value
or original panic payload through the existing one-shot result. Completion retires in-flight
work and wakes the waiting task. Polling the failed result releases the result lock before
resuming the panic on the awaiting thread; an ordinary App task then follows APP_FAILED as
usual. A returned Result::Err remains a value, not a panic or App failure.

Cancellation only withdraws interest. Already-running blocking work is not interrupted; when
it finishes, its bookkeeping retires even if nobody awaits it. A cancelled task is not revived
to observe a late value or panic. No rollback of external side effects or automatic retry is
provided. This borrows Rune's worker containment and owner-thread propagation separately,
without adding a public task-error wrapper or changing Background<T>'s Output = T.

### Result interest and cancellation

The awaiting future is the only long-lived strong owner of its one-shot result slot.
Background workers and native reply callbacks retain weak result references. Dropping the
future therefore releases its stored waker without waiting for blocking work or a platform
reply. A late producer discards its unclaimed result rather than reviving a task. Native
reply callbacks also hold the scheduler weakly, so a platform retaining an obsolete callback
cannot prolong a retired App's scheduling state.

Both producers use one delivery path. It releases the result/waker locks before waking and
returns unclaimed values to the producer. Background completion retires in-flight accounting
before dropping such user-owned values. Already-running blocking work still finishes normally;
its counter is separate from whether anyone wants its result.

A completion racing cancellation may briefly upgrade the weak slot and enqueue a stale wake.
Existing task identities reject that wake after retirement. No additional cancellation flag,
public handle, generation protocol or rollback mechanism is introduced.

Tests hold a worker behind a channel, drop its waiting future, and verify that the waker is
released before allowing the worker to finish. A second test retains a native reply across
future cancellation and App shutdown, verifies that scheduler state is released, then delivers
and drops the late result exactly once.

### Timers and channels

Sleep owns one registration on the App clock that created it. Repeated polls reuse that
registration; dropping a wait removes it immediately without disturbing another wait in the
same task. Polling it in a different App is a programming error. interval and clock require
a positive period; INVALID_INTERVAL rejects zero before allocating a task, clock or cleanup.

Channel enqueue, sender count, receiver closure and waker registration share one mutex.
Closing the receiver removes its queued values and waker; destructors and wake callbacks run
outside that mutex. A send racing closure either queues before closure (and is then released)
or returns its value. Keeping a sender does not retain the abandoned queue.

A receiver has one outstanding wait, enforced by recv(&mut self). The Recv future owns its
waker registration: dropping just the wait releases the waker without closing the receiver or
consuming queued values. This also makes cancellation after a sender wakes the task safe: the
next wait still receives the message. No waiter registry, per-wait identity or runtime
multiple-consumer error is needed. Receiver::on retains its existing consuming API.

A named receiver owns a thread-safe interest token; the UI registry owns its scope cleanup
registration. Replacing a name releases the old registration immediately. Dropping or renaming
the receiver retires its interest and wakes the host; dead names are removed at the next App
boundary or named send. Channels remain Send when their values are Send. No UI-owned cleanup
token crosses threads, and an obsolete receiver cannot unregister a replacement.

### Task identity and scheduling order

A task identity is its allocation slot plus generation. It validates cancellation and late
wakes, but it has no ordering relation: a reused slot's larger generation does not make its
task newer than a subsequently allocated slot.

Each task therefore receives an internal monotonic spawn order. Ready processing first removes
retired identities, then sorts live tasks by that order and deduplicates repeated wakes.
The task table borrow is released before polling; an earlier task can still cancel a later
one or spawn work for the next ready batch. A panic preserves the not-yet-polled portion of
the batch through the existing unwind guard.

This guarantees ordering within a ready batch, not deterministic arrival of external work.
No new public handle, priority API or scheduler is introduced. Regressions cover slot reuse,
reverse wake arrival, repeated wakes, cancellation ordering and an obsolete waker targeting
a slot now occupied by another task.

### Task draining is not reentrant

Only the outer `Scheduler::run_ready` call consumes the ready queue. A task or a future's
destructor may reenter the App, but a nested drain leaves wakes queued for the outer call.
This matters because an actively polled future temporarily leaves its slot: consuming its
wake then would silently lose the next poll. Newly spawned work also waits until the current
poll and its retirement finish. A scoped guard releases the drain on both return and unwind;
App failure and shutdown retain their existing behavior.

### Cooperative task allowance per App turn

Each outer App entry starts one allowance of 1,024 task poll attempts. Nested entries,
window preparation rounds and other windows share it. This is an internal bound, not a
public priority API or a wall-clock deadline: one arbitrary Rust poll, its destructor,
ready-queue sorting and layout cannot be preempted. The value is a conservative starting
policy, not a measured optimal frame budget.

When an allowance ends in the middle of a batch, its remaining identities stay in their
original order and run before newly queued wakes on the next turn. New batches still sort
by creation order and deduplicate wakes. Retired identities are rejected at polling, and
shutdown clears both the suspended batch and incoming wakes. The same batch owner retains
remaining work on unwind; App failure still prevents ordinary continuation.

An exhausted turn with ready work requests another host wake at the outer boundary.
Pending work includes an Update explaining that ready tasks yielded. Windows may present
currently prepared state but remain incomplete while scheduling has yielded, so capture
cannot mistake a partial update for a settled result. This also applies to ready subscriptions,
which would otherwise be excluded from the ordinary task-completion check.

`settle()` remains an explicit bounded multi-turn helper; it can spend more than one turn's
allowance. This change does not impose a wall-time budget on that helper, deduplicate the
cross-thread wake queue at insertion, or discard/coalesce channel messages.

Regressions cover a finite batch larger than the allowance and its host continuation,
self-waking tasks ahead of finite work, repeated wakes with nested polling, shutdown with a
suspended batch, and two windows sharing the allowance and recovering after cancellation.

### Execution revocation and resumable resource teardown

A watch invocation is not a new ownership scope. Tasks it starts belong to its registration
scope. Rerunning or stopping the watch does not replace/cancel those tasks; retiring that scope
does. AsyncMemo explicitly replaces its previous Task and validates results. No implicit
invocation scope or new public ownership primitive is added.

Execution permission and release progress are separate facts. App keeps its one-way shutdown
gate and an internal Pending/Running/Complete teardown state. A panic resets Running to Pending,
never reopens execution, and propagates unchanged. A subsequent explicit shutdown resumes;
nested shutdown during an active pass does nothing. `is_shutdown()` means execution has stopped,
not that every resource has been released. `pending()` reports an Update while shutdown teardown
is incomplete. There is no automatic retry loop or blanket panic-to-error conversion.

Scopes have Live, Disposing, Retiring and Disposed states. Only Live admits new work. Disposing
prevents recursive disposal; unwind changes it to Retiring. Children remain attached until their
release succeeds. Cleanup registrations are removed one at a time before invocation, in reverse
registration order: an interrupted callback is consumed, unstarted callbacks remain owned.
Nodes and contexts similarly leave their ownership records before disposal, outside table
borrows. The scope returns to the free list only after all stages complete. Teardown continues
children before parents and never reuses an incompletely released scope slot.

App shutdown closes the scheduler before user scope cleanup. Scheduler execution closure is
independent from cancellation progress: if one future destructor panics, remaining slots survive
for a later shutdown. No destructor or cleanup is invoked again merely because a pass failed.
Already-running blocking work and an active poll still follow their existing lifetime rules.
A panic while another Rust destructor is already unwinding may abort; this design cannot promise
recovery from double panic, process abort, or rollback of external side effects.

Windows revoke input first and retain their registry entry/root scope until disposal succeeds.
The platform window is replaced before its old value is dropped outside the platform-table
borrow. Interrupted hot reload restores reload mode on unwind and keeps the retiring root
reachable. Final Runtime shutdown disposes remaining scope-owned allocations and the runtime's
remaining nodes, including named state preserved by an interrupted reload, before clearing
named-state and observer-queue indices. App and Window handles may remain alive after release.
Unlike ordinary component disposal, final App shutdown also invalidates value-owned collection
item nodes retained by external ReadList handles. Such handles cannot prolong runtime graph
values past completed shutdown and remain safe to drop after the runtime goes away.

Tests cover watch task ownership; root and branch cleanup panic; successive cleanup/context
panics with reverse ordering and no replay; task-destructor panic; window cleanup and hot reload
failure with retained handles; complete-state idempotence and stale scope identities. These
establish resumable framework teardown, not successful completion of a callback that panicked.

### Future notification and finite identities

Sleep timers retain both their owning TaskId and the most recently supplied Waker. Task identity
controls cancellation/liveness; the Waker controls notification, including child futures inside
wake-driven combinators. Re-polling replaces the Waker without adding a timer. Expiry extracts
timers before invoking their wakers; replacement, cancellation, stale-timer pruning and shutdown
release wakers outside timer-table borrows. Background/native one-shot waits likewise clone the
new Waker before locking and drop the replaced Waker after unlocking, preserving the second
result check that closes the result/registration race.

Task, reactive node and scope generations now use checked increment. Slot allocation checks
u32 index capacity before appending; App namespace sequence allocation uses checked atomic
update. Exhaustion is a programming/runtime limit that panics rather than wrapping in release
builds and accepting a stale identity. No UUID or public identity type is added. Private tests
seed generation counters at their limit; they do not allocate billions of objects.

### Lifecycle transitions and internal borrowing

Execution permission, result interest, resource release and presentation are separate
boundaries. They are not represented by one universal lifecycle enum:

- A task slot is `Suspended(future)`, `Polling` or `Cancelled`. Polling transfers the
  Future to a guard outside the table. Cancellation of an active poll revokes permission
  immediately; the guard releases the Future when the poll returns or unwinds. An absent
  slot is retired. Existing checked identity generations still guard slot reuse.
- A one-shot delivery is `Waiting(waker)`, `Ready(value)` or `Closed`, under one mutex.
  Registration and delivery cannot miss each other. Closing explicitly revokes interest,
  even if a producer temporarily holds a strong reference. An already extracted wake may
  race with cancellation: it is a notification hint, never permission to publish.
- Blocking work can outlive its cancelled receiver. Physical completion accounting stays
  separate from result interest; cancellation does not promise reversal of external effects.
- Scope disposal and App teardown retain their resumable progress. A started cleanup is
  consumed before invocation and is not replayed after panic.
- A frame's publication record advances only after the host presentation call returns.
  This records successful handoff, not an OS display acknowledgement. A host panic cannot
  roll back external effects, but fails the App and does not report a completed handoff.

For bookkeeping containers, follow **validate, transition/extract, release borrow, then
invoke/drop**. Waker cloning can run user code; so can final Arc/Rc release, generic values,
Future destruction and closure capture destruction. Their disposal belongs outside locks
and table borrows. Host notification replacement and capture painter replacement obey the
same rule as task cancellation and scope context replacement.

This rule does not remove the intentional value borrow in `Signal::with` or `Signal::update`:
those APIs explicitly lend `&T` or `&mut T` to user code and diagnose conflicting same-value
access. Likewise a platform method has exclusive access to its platform object. Neither
is permission to hold unrelated registry/table borrows across arbitrary callbacks.

Validation covers cancellation while a producer retains the slot, latest-waker replacement,
concurrent completion/registration, reentrant result and callback destruction, and failed
host publication. Existing tests cover active-poll shutdown, stale AsyncMemo publication,
unwinding cleanup resumption and unfinished layout refusal. Native OS acceptance is separate
from these deterministic core/host-adapter tests.

### Completion commitment, accounting and effects

One-shot `commit` only transfers a result into its delivery slot and extracts the
notification; it does not call the Waker. Background work owns an unwind-safe
completion guard. It retires the physical in-flight count after commitment and before
running the notification or destroying a rejected result. Native replies use the same
commit/effect boundary without pretending to own a background worker count.

An ordinary work panic remains a result payload resumed by its awaiting task. A panic
in external completion (Waker, host wake, or unclaimed result destruction) is different:
there may be no receiver left. Background and native reply completion catch that panic,
retain its original payload in the owning scheduler, and expose pending Update work.
The next normal App entry, before its user closure, or task-drain boundary resumes it
through the existing App failure guard. Shared pool workers survive these completion
failures. Shutdown bypasses normal execution admission and can retire an unobserved
failure; inspecting pending work does not consume it.

A failed notification is not retried, since the notification itself may be the failing
host adapter. The host needs another explicit App entry to observe the panic. There is
no automatic restart, rollback of an already committed result, or claim that a successful
result makes the surrounding App healthy. In-flight zero means physical work has retired,
not that all external notification side effects have finished.

AsyncMemo refresh, failure and success share one internal transition. It moves previous
values and errors into either the new state or a local retirement tuple. Signal mutation
and invalidation complete before that tuple drops, so destructors can read the committed
state. Public Signal::with/update keep their explicit reference-borrowing contracts;
no new public transition or result primitive is introduced.

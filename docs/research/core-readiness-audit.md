# Core readiness after task lifetime consolidation

Reviewed 2026-11-16 at Octara `7cbcaad`. This is a source-based audit and proposed
acceptance plan, not a claim that the remaining cases have been reproduced or fixed.
The preceding implementation passed 795 tests, with 3 ignored, plus workspace Clippy.
No runtime code is changed by this review.

## Reference boundary

Local reference sources are under `~/code/archive/reference`, not this repository:
Rune `3686e94ae09545fd5f2a61e22aabfbeba1cd896f` and Exact
`08c889a9de21a30b7c3b308422fc588cf1a480d3`. These are inspected snapshots, not asserted
latest upstream revisions. Solid's linked `next` RFC is a mutable proposal document.

- [Rune memo](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune/src/memo.rs)
  explicitly separates subtree identity from validity: versioned captures, environment,
  recorded geometry reads and hot reload participate in reuse. Adopt explicit validity
  contracts for caches; do not add manual version arguments to automatically tracked Signals.
- [Exact settlement](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/runner/src/runner/settlement.rs)
  checks dependency readiness before reuse. Its
  [overrun handling](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/runner/src/runner/overrun.rs)
  represents refused work and later attempts explicitly. Borrow observable incomplete work;
  do not transplant VM interruption or automatic request retries into arbitrary Rust callbacks.
- [Solid 2 reactivity RFC](https://github.com/solidjs/solid/blob/next/documentation/solid-2.0/01-reactivity-batching-effects.md)
  separates tracking from effects and defines scheduling/cleanup semantics. Use it to ask
  precise timing questions, not to replace immediate Rust reads with microtask commits or
  introduce transparent async reads.

## Already present; do not add another primitive

Signals support non-Clone values through borrowing, explicit read handles, memos with equality
cutoff, dynamic dependencies, scope disposal and untracking without granting write permission.
Tests already cover diamonds, dependency switching, mutation unwind, watch stopping and scope
retirement (`reactive/graph.rs`). These are not missing capabilities.

AsyncMemo checks result admission against current inputs and task lifetime. Tasks, timer waits,
channel waits and one-shot results now have explicit retirement. App failure prevents normal
continuation; Inspector can still explain it. The last two commits establish task creation order
within a ready batch and prevent nested drains from consuming an active task's wake.

## Remaining work, in order

### 1. Bound one host turn and report unfinished work

Evidence: `task.rs::run_ready` limits 10,000 **batches**, not total polls; each batch has
unbounded size. `app.rs::prepare_window` may call it across 16 rounds, while
`app/time.rs::settle` has its own 1,000-iteration loop and background wait. `TaskWaker`
appends every wake to a Vec and deduplicates only when draining. Channel storage is also
unbounded, though message retention and wake coalescing are different semantic concerns.

These facts establish that the existing limits are not one per-host-turn work budget.
They do not establish measured input latency, a lost wake, or a need for a new channel API.

First acceptance cases: continuously self-waking work alongside a finite task; many ready
tasks; repeated wakes before polling; cancellation/shutdown while work remains. Verify finite
work progresses, an outer host turn returns within an agreed poll bound, queued work survives
and continuation is requested. Capture/Inspector must not imply settlement when work remains.
A single Rust Future::poll cannot be preempted; the guarantee applies between polls.

Prefer an internal drain outcome and a shared per-turn allowance if tests demonstrate the need.
Do not independently tune every loop, add public priorities, or build an executor abstraction.
Measure wake coalescing separately; do not silently coalesce channel messages.

### 2. Test composed reactive semantics rather than redesign Signals

Individual diamond, branch switching and equality-cutoff tests exist. Add a bounded combination
matrix: switch dependencies while an upstream memo stays equal, write from the first watch into
the second watch's source, remove the owning branch with callbacks queued, and read before/after
flush. Check values, callback counts and retirement against the existing structural-before-
binding-before-watch contract. A two-phase effect scheduler is not justified merely because
Solid uses one. Retain the current public API unless a concrete semantic contradiction appears.

### 3. Decide whether invocation-owned cleanup has a real use case

`watch` callbacks run under their registration scope. `Watch::stop` disposes the observer;
work spawned by a previous callback is not thereby a child of that invocation. This is a
lifetime distinction, not a demonstrated bug. Inventory real subscription/replacement sites
before changing behavior. AsyncMemo already replaces its task and should remain the standard
async derivation path. Prefer an explicit Rust-owned replacement guard if needed; do not silently
make ordinary scope-owned work disappear on every callback rerun.

Separately audit cleanup panic: `dispose_scope` retires the scope before calling user cleanups;
a panic interrupts the remaining cleanup/body. Determine what subsequent App shutdown releases
and what remains until App drop. Preserve fail-stop semantics; do not promise all user cleanup
callbacks execute or recover from arbitrary destructor panics without explicit design/tests.

### 4. Close identity exhaustion and native acceptance gaps

Task slot generation uses `u32 += 1`; release overflow can eventually reuse an identity.
This is a concrete arithmetic boundary, not a normal-workload failure. Define checked exhaustion
or permanent slot retirement and test near the boundary without billions of allocations.
Do not replace local handles with UUIDv7 merely to address overflow. Namespace allocation and
other arenas deserve the same small audit, without inventing a public identity system.

Visible presentation and native OS close after App failure remain unverified on the locked
computer. The existing offscreen/protocol evidence is not a substitute; complete that acceptance
when a visible desktop is available, without blocking deterministic core work.

## Scope decision

Prioritize host-turn progress, then the composed graph/lifetime checks. Keep Rust + Signals,
AsyncMemo and explicit ownership. No VM, deep Store, global UUID migration, public scheduler,
transparent async Signals or new UI-kit components are required by this audit. Backpressure,
per-invocation cleanup and cache optimization remain demand-driven, not automatic additions.

## First follow-up: cooperative task turns

The next implementation replaces the batch-count cap with a shared per-outer-App-entry poll
allowance, preserves an unfinished batch ahead of new wakes, requests host continuation and
reports yielded work as incomplete. See [the runtime contract](../design/runtime-contract.md#cooperative-task-allowance-per-app-turn).
This addresses the poll-count boundary only. It does not establish a wall-clock latency bound,
introduce insertion-time wake coalescing/backpressure, or complete the remaining graph/lifetime
and native acceptance work above.

## Second follow-up: composed scalar graph checks

Three regression tests now exercise four branch/read combinations, two watch creation orders,
and structural disposal triggered by a preceding watch after an early descendant memo read.
All cases passed without changing runtime implementation. Dependency replacement survives
value equality cutoff, memo reads are fresh before effect delivery, and queued descendant
callbacks retire with their branch. Watch feedback is sequential and intentionally order
sensitive; the [reactive contract](../design/reactive-core.md#composed-reads-and-effects) now
spells out that distinction. No two-phase scheduler or additional Signals primitive is justified
by these cases. Invocation-owned cleanup and cleanup-panic shutdown remain separate audits.

## Third follow-up: ownership and cleanup-panic audit

Inspected AsyncMemo, paced text, Chat reasoning, forms and Combobox watchers. These sites do not
establish a need for an invocation-owned cleanup primitive: async replacement is explicit and
other work is component-owned or synchronous state synchronization. A regression confirms that
Watch::stop leaves previously spawned scope-owned tasks alive until their owner retires.

Two panic probes identify the remaining teardown limit: branch cleanup can retire a scope before
releasing its nodes, and later shutdown skips that scope; shutdown cleanup can similarly leave
state allocated while the scheduler guard cancels tasks. Values in the probes release when the
runtime drops. This is permitted by the current failure contract, but is a concrete target for
a future lifecycle design rather than more watch APIs. See the
[audited limits](../design/runtime-contract.md#execution-revocation-and-resumable-resource-teardown).
That audit added no panic-recovery implementation or public lifetime mechanism. The implementation
follow-up below supersedes its delayed-release limitation.

## Fourth follow-up: resumable framework teardown

Execution gates now remain permanently closed independently of teardown progress. Scope records
retain unstarted children/cleanups/nodes/contexts; a started callback is removed before invocation.
App shutdown can resume after panic, including interrupted window close and hot reload, without
reopening normal work or replaying callbacks. Scheduler cancellation similarly resumes remaining
slots after a future destructor panic. Incomplete teardown is visible through pending work.

The earlier retention probes now require state release on the next shutdown while App/Window
handles remain alive. Additional tests cover context destructor panic, parent/child ordering,
slot reuse, task destructor panic and hot-reload named state. No watch invocation scope, automatic
retry engine or user-code rollback has been introduced.

## Native acceptance follow-up

The desktop was unlocked and the three handler/task/binding failure cases passed again on
binaries rebuilt with resumable teardown: non-occluded windows had two presented frames each,
failed Apps stopped presenting, and PID-scoped native close exited each process with status 1
and removed its socket. See [native failure validation](native-failure-validation.md).
This closes the earlier visible-presentation/native-close gap for those cases; it does not
claim native keyboard fault injection or native cleanup-panic scenarios.

Final follow-up validation: workspace tests passed (808 passed, 3 ignored), workspace all-target
Clippy passed with warnings denied, and formatting/diff checks passed. One obsolete retained-list
assertion was updated to require release at completed App shutdown. A separate parallel CLI test
fixture collision was fixed by adding a process-local sequence to its timestamped socket path;
this does not change runtime identities. The native three-case acceptance above passed on the
rebuilt runtime. Changes remain uncommitted at this checkpoint.

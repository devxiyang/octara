# Runtime architecture

Status: selected architecture direction, 2026-11-14. Rust builders, Signals, scoped tasks, the
retained Kernel and GPU rendering are the implemented foundation. Live attachment and the
correctness work below remain implementation targets; [Architecture](../ARCHITECTURE.md) records
what currently works.

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

1. The App accepts input or ready work at a safe UI-thread boundary.
2. Handlers and tasks change Signals. The reactive runtime updates affected bindings; structures
   mount or dispose their scopes. Kernel borrows are released before user code runs.
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

Drawing may coalesce updates and reuse display data. If native placement and deferred drawing
would otherwise use different geometry or animation times, reconcile them at the window's output
boundary. Submission and the presentation evidence supplied by the host remain separate stages.

App time advance is explicit. A read may prepare or drain bounded ready work at its documented
boundary, but does not secretly move app time or wait indefinitely. Preserve existing inspect
semantics; introducing an independent sampling clock is not a prerequisite for live attachment.

## Waiting and failure

An action can finish delivering while a network request or animation remains pending. Preserve
that distinction in its reply. A bounded wait or settle returns remaining work and its reason;
a timeout alone does not damage the runtime or suspend all ordinary input. Existing advance and
settle operations retain their documented clock behavior; a host declares any added native wait.

Bound reactive/layout feedback and diagnose loops. Yield or retry only where the remaining work
can safely continue. Do not submit inconsistent partial output as a completed frame. If an
internal invariant is broken and safe continuation cannot be established, reset the affected
window/runtime and invalidate its handles. Avoid a general incomplete-tree recovery protocol.

Ordinary pending work, recoverable operation errors and broken invariants are different outcomes.
Rust callbacks may already have produced effects; errors and lost replies do not authorize replay.
The framework does not promise rollback of arbitrary Rust state or external operations.

## Lifetimes and external work

Keep scopes and generation-checked handles. A node reference must still identify its owning
window/tree after destruction, reload or restart; connection/incarnation checks can supply the
outer lifetime. Names and test IDs help discovery. They never make a retired occurrence valid.

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

1. **Live loop:** connect the existing Inspector to a real desktop window through a safe host
   queue. Read a control and its source, act, observe pending work and capture the result.
   Capability and delivery reporting accompany the first supported operations.
2. **Local correctness:** request ownership and late-result handling, stale-target and measurement
   checks, cross-window dirty propagation, cleanup ordering and coherent per-window output.
   Scope each fix to a concrete failing scenario.
3. **Native pilot:** direct text-input integration, then a mobile example covering composition,
   keyboard geometry, cancelled navigation and late work after leaving a page.

Deferred extensions are general snapshot/watch, a universal resource namespace, complete causal
history and a global UI revision. Introduce a specific extension only when a reproducible tool
task establishes its need. Agent improvement uses the ordinary development loop; a separate
framework-managed learning service is outside this architecture.

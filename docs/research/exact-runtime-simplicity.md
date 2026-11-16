# What Exact suggests simplifying in Octara

Research baseline: 2026-11-14. Detailed source and test inspection used Exact
`08c889a9de21a30b7c3b308422fc588cf1a480d3`; targeted checks of the runtime and Agent paths at
upstream `4e10172a13a6756687fec70ed61f16dbd648fc8a` confirmed the main mechanisms below.
Mobile examples and tests retain their original pinned links. This is source-based analysis,
not a report of executing Exact's tests or qualifying its platforms.

The [2026-11-16 collection/region review](collection-regions.md) pins the newer `main` head
`5491ab29`, covering row input replacement, branch ownership, bounded virtual residency and
resource-retirement failures. The broader runtime comparison below keeps its stated baselines.

**Octara's initial observation and convergence proposal was too large for the first working loop.**
Exact demonstrates a smaller external model: query the current app, operate it through a declared
path, wait explicitly when necessary, and return useful values and errors. Necessary correctness
mechanisms remain local to commits, requests, measurements, editing and navigation.

The Rust builders, Signals, retained Kernel and GPU direction do not need replacing. The main
reduction is to remove a general resource/provenance/history system and global convergence contract
as prerequisites for useful live inspection. The selected scope is now recorded in
[Runtime architecture](../design/runtime-contract.md). Its planned runtime changes are not yet
implemented; the comparison does not imply that Exact's internals are simple.

## Start with the questions an Agent actually asks

Exact's public command families cover trees, state, geometry, screenshots, input, clocks, logs,
environment preferences and performance. Runner-owned facts are read directly from the runner
and Kernel; rendered geometry, captures and input belong to their real host. New narrow questions
can fit within these operations. There is no second reconstructed application model.
[Runner interface][agent], [scope policy][scope].

The scope policy records a concrete predecessor failure: a small primitive set expanded into
roughly eighty wire names. It deliberately excludes a general dataflow/bindings/provenance explorer,
while admitting development source maps and explanation of one selected node. This is a scope
decision, not proof that richer tools are never useful.

For Octara, retain the existing inspector vocabulary and make the real-window loop work. A
generic router for windows, nodes, bindings, tasks, requests and events can wait until direct
queries demonstrably need it. A common implementation can still serve headless and live hosts
without making every internal object a public resource API.

## A few tags and a connection can supply enough identity

Exact's ordinary observation tags are `epoch`, `incarnation` and `clock`. They have narrow meanings:

| Field or mechanism | Scope |
| --- | --- |
| Kernel epoch | Advances for a commit with created, destroyed or touched nodes; not every runner state change |
| Kernel incarnation | Changes on Kernel reset; old keys cease to resolve |
| Clock | Runner application time, not proof of platform animation or display completion |
| Layout stamp | Incarnation, epoch and layout publication, used where measurement caches need it |
| Plan digest | Requested for development inspection/source compatibility, not computed for every ordinary read |
| Carrier/session | Supplies part of runtime identity and request ordering; local incarnation values can repeat across host replacements |

[Tags and on-demand identity][tags], [epoch publication][epoch], [layout stamp][layout-stamp].

The native transport matches ordered requests and replies through a FIFO queue. The Apple host
finishes one request before reading the next. It therefore does not need a mandatory correlation
ID or asynchronous operation registry for every command. The transport also handles disconnect
and foreground changes so late replies are not mistaken for new answers. This simplicity depends
on preserving those ordering constraints. [Wire][wire], [host request loop][host-requests].

For Octara's first local connection, explicit request IDs are optional if requests remain serialized.
Connection/incarnation and stale-target validation are still required semantics. Add explicit
correlation when multiplexing or asynchronous outcomes actually require it. Build provenance can
be connection metadata; turn numbers and cause links can be optional debug detail.

## Current context and source explanation precede a general causal graph

Exact's selected-node inspection exposes current properties, authored/dynamic/inherited style
sources, tokens, plan site and instance path. Its driver joins the same reply's plan digest to a
compatible development source map for declaration and component call sites. An incompatible map
produces unavailable source rather than a plausible but incorrect location. Runtime hosts do not
read source maps; the driver and development design tooling perform the join. Plan compatibility
does not establish the original authored source revision. [Node inspection][node], [source join][source-map].

This supplies useful first-hand context without a permanent input-to-every-dependency graph.
Exact also retains local origin/ticket metadata and can enable commit tracing for a particular
call; selective explanation does not require eliminating existing instrumentation. [Targeted trace][trace].
For Octara, prioritize node source, current/target values, property origin, task/resource status
and pending reason. Add a deeper dependency explanation when a real debugging task needs it.

Exact's inspect metadata has also caused quadratic work: one root-to-node walk for every exported
node. The fix reuses a stored node-to-site map. Another issue records repeated navigation subtree
walks making tool queries stall while the app itself remained responsive. Observation needs its
own cost budget. [Plan identity issue][identity-cost], [navigation query issue][query-cost].

Exact's typed `state` output comes from its compiled Plan/Value model. Octara cannot automatically
reflect arbitrary Rust closure captures or business objects; opt-in debug values remain appropriate.

## Keep commit settlement separate from waiting for the screen

Exact has two different mechanisms that should not be merged into one global convergence rule.

An action, timer callback or reply performs a controlled runner transaction: checkpoint state,
evaluate the Plan/VM, settle derives/resources, update the instance tree, apply Kernel operations,
then conclude. Settlement uses tentative values and rejects dependency cycles or exhausted bounds.
A mutation's `then` handler runs as a subsequent commit. [Commit boundary][commit], [settlement][settlement].

The Agent's `clock settle` is a separate host-level operation. It combines requests, framework
motion, list measurement and native activity under finite budgets. A timeout reports `settled:false`
with a reason. It does not itself place the entire application into a persistent input-frozen
state. A native-motion test deliberately exceeds the wait budget and expects an unfinished result.
[Host settling][host-settle], [native-motion test][motion-test].

Input delivery is also distinguished from later settling: when an input was delivered but landing
its consequences failed, the driver returns `landError` rather than presenting the input as refused
and encouraging a duplicate action. [Delivery and landing][landing].

Queries are not uniformly passive. The Apple Agent drains queued work before requests, and some
reads establish launch/canvas readiness. This does not imply that every query advances virtual
time. Exact does not adopt the proposal's blanket non-draining query contract; it provides a
different tool boundary to evaluate, rather than proving passive queries have no value.
[Read preparation][host-reads].

For Octara, preserve a clear outer update boundary, bounded internal propagation and coherent
per-window output. Make wait/advance/settle explicit operations with truthful budgets and reasons.
Do not require all native activity or arbitrary layout feedback to finish before every normal
update. An observation wait timeout should not create a framework-wide recovery state machine.
Existing inspection behavior can remain while live ingress is added; a sampling rewrite need not
block that first loop.

## Exact still pays for transactions and treats internal corruption separately

Exact can restore many refused actions because its mutable state and staged effects are controlled
by the Plan/VM. Checkpoints preserve slots, resources, request bookkeeping, timers and other state.
Kernel batches validate before writes. The ordinary refusal tests keep the old Kernel and state.
These guarantees require real machinery. [Checkpoint implementation][checkpoint], [refusal tests][refusal-test].

There is an explicit exception: a later phase of a multi-phase document commit can fail after
earlier phases mutated the tree. Exact then requires poison/reset rather than presenting or
continuing that partial tree. This is an internal consistency failure, distinct from a network
request pending or an observation wait expiring. [Partial-commit failure contract][poison].

For Octara, do not promise rollback of arbitrary Rust callbacks or external side effects. Separate
normal pending work, recoverable operation errors and broken internal invariants. Use a small,
explicit reset/restart path for a broken runtime rather than designing indefinite operation over
an incomplete tree. Before requiring transactional Kernel batches, identify the actual failure
boundary that needs them; do not copy Exact's VM or checkpoint engine as an inspection prerequisite.

## Put freshness and native interaction checks where they belong

Exact's requests use tickets: enqueue, retain interest, fulfill, then reject replies whose tickets
are no longer held. Forgotten late stream replies are tested not to overwrite current values or
produce a commit. The executor distinguishes forgetting a result from aborting transport; an
already-started write may continue without delivering its result. [Ticket path][tickets],
[late-stream test][late-test], [executor policy][forget].

Collection feedback has its own revision, scroll sequence and row measurement epoch. Tests keep
the same row key and view while changing content, then reject an old measurement. This check
belongs to that asynchronous feedback boundary, not to a universal app revision. [Collection test][row-test].

On iOS, controlled text writes use a local `pendingValue` while marked text is active. Navigation
uses UIKit's gesture/transition ownership plus pending synchronization; cancelling a native pop
does not dispatch a logical Back. These are concrete local protocols. [Editing][editing],
[navigation reconciliation][navigation].

For Octara, keep scope/request ownership, late-result disposal, content freshness and one owner
per interaction region. Begin native text and navigation work at real adapter boundaries. A
universal editing-session or navigation-transaction abstraction can follow demonstrated shared needs.

Real simulator input remains necessary evidence. Exact distinguishes normal addressed actions from
platform input; its Android documentation explicitly says ordinary `type` does not exercise keyboard
composition, and several native animations remain outside the Agent clock. A small interface must
still report which path ran. [Android input limits][android], [native Back distinction][input-testing].

## Recommended first scope for Octara

| Keep in the first working loop | Defer or remove as a prerequisite |
| --- | --- |
| Real app connection and UI-thread ingress | General resource namespace and public schemas for every internal owner |
| Tree/node, source, geometry, task/pending reads | Permanent dependency/causal graph and generic provenance explorer |
| Input, capture, explicit advance and bounded settle | App-wide publication requiring all-window layout and cleanup convergence |
| Owner identity, request tickets and local freshness checks | An early public identity scheme built around all six concepts |
| Serialized replies with delivery/error distinction | General queued/committed/terminal outcome registry before concurrent use exists |
| Direct reads and small bounded diagnostic logs | A generic snapshot/watch protocol before continuous observation needs it |
| Explicit reset for broken invariants | Persistent incomplete-tree mode with ordinary-input freezing and turn recovery |

The concrete first acceptance case remains small: attach to the actual window, find a control,
inspect its value and source, operate it, read the result and pending work, and capture what the
host shows. Implement it using existing owners and inspector facilities. Additional machinery
must earn its place through a reproducible failure or a concrete tool task.

## References

[agent]: https://github.com/expo/exact/blob/4e10172a13a6756687fec70ed61f16dbd648fc8a/runner/src/agent.rs#L1
[scope]: https://github.com/expo/exact/blob/4e10172a13a6756687fec70ed61f16dbd648fc8a/rules/DEFERRED.md#L1876
[tags]: https://github.com/expo/exact/blob/4e10172a13a6756687fec70ed61f16dbd648fc8a/runner/src/agent.rs#L94
[epoch]: https://github.com/expo/exact/blob/4e10172a13a6756687fec70ed61f16dbd648fc8a/kernel/src/kernel/document.rs#L187
[layout-stamp]: https://github.com/expo/exact/blob/4e10172a13a6756687fec70ed61f16dbd648fc8a/kernel/src/kernel.rs#L454
[wire]: https://github.com/expo/exact/blob/4e10172a13a6756687fec70ed61f16dbd648fc8a/scripts/agent-wire.mjs#L36
[host-requests]: https://github.com/expo/exact/blob/4e10172a13a6756687fec70ed61f16dbd648fc8a/host/apple/Sources/ExactKit/Agent.swift#L113
[node]: https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/runner/src/agent.rs#L527
[source-map]: https://github.com/expo/exact/blob/4e10172a13a6756687fec70ed61f16dbd648fc8a/scripts/agent-inspect.mjs#L33
[trace]: https://github.com/expo/exact/blob/4e10172a13a6756687fec70ed61f16dbd648fc8a/runner/src/runner/trace.rs#L12
[identity-cost]: https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/issues/closed/20261102-agent-tree-plan-identity-walks-every-node.md
[query-cost]: https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/issues/closed/20261030-agent-tree-inactive-is-quadratic.md
[commit]: https://github.com/expo/exact/blob/4e10172a13a6756687fec70ed61f16dbd648fc8a/runner/src/runner/commit.rs#L269
[settlement]: https://github.com/expo/exact/blob/4e10172a13a6756687fec70ed61f16dbd648fc8a/runner/src/runner/settlement.rs#L246
[host-settle]: https://github.com/expo/exact/blob/4e10172a13a6756687fec70ed61f16dbd648fc8a/host/apple/Sources/ExactKit/Agent.swift#L882
[motion-test]: https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/tests/ExactKitTests/AgentClockIOSTests.swift#L367
[landing]: https://github.com/expo/exact/blob/4e10172a13a6756687fec70ed61f16dbd648fc8a/scripts/agent-session.mjs#L805
[host-reads]: https://github.com/expo/exact/blob/4e10172a13a6756687fec70ed61f16dbd648fc8a/host/apple/Sources/ExactKit/Agent.swift#L168
[checkpoint]: https://github.com/expo/exact/blob/4e10172a13a6756687fec70ed61f16dbd648fc8a/runner/src/runner/commit.rs#L16
[refusal-test]: https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/runner/tests/it/now_screen.rs#L832
[poison]: https://github.com/expo/exact/blob/4e10172a13a6756687fec70ed61f16dbd648fc8a/kernel/src/kernel/document.rs#L108
[tickets]: https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/runner/src/runner/commit.rs#L1086
[late-test]: https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/runner/src/runner/stream_tests.rs#L241
[forget]: https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/src/executor_forget.rs#L70
[row-test]: https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/runner/src/instance/collection/tests.rs#L1023
[editing]: https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/IOS/FieldEditingIOS.swift#L36
[navigation]: https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/IOS/NavigationIOS.swift#L1302
[android]: https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/android/README.md#L196
[input-testing]: https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/docs/testing.md#L203

# Runtime foundations: time, observation, and feedback

Status: research baseline, 2026-11-14. The selected implementation direction lives in
[runtime architecture](../design/runtime-contract.md) and [Agent context](../design/agent-context.md).
This note separates published models, engineering evidence, and Octara's design inferences.
It does not describe guarantees already implemented by Octara.

[Identity and progress in mature UI runtimes](mature-runtime-identities.md) compares fixed Chromium,
Flutter/Dart, Compose and GPUI source snapshots. The
[Exact comparison](exact-runtime-simplicity.md) motivates the smaller selected scope. The names
below are analytical vocabulary for distinct questions, not a required set of public IDs or a
promise of app-wide layout convergence.

## What a timeline must distinguish

An input can occur on a platform, enter a queue, be adopted by the application, change derived state,
produce layout, and eventually contribute to a submitted frame. These are different observations.
Background completions and native callbacks introduce additional orderings even with one UI thread.
Our inference is to specify those boundaries before choosing a scheduler or a rendering architecture.

| Dimension | Question distinguished by the analytical vocabulary |
| --- | --- |
| Processing order | `TurnId` orders accepted inputs and completions inside one runtime. |
| Causality | `CauseId` and explicit request/binding links explain why work was produced. |
| Logical state | A revision must identify a specified publication boundary; its scope cannot be inferred from its name. |
| Sample time | `SampleTime` selects time-dependent values using the application clock. |
| Host progress | Submission and available presentation feedback name their own stages and provenance. |
| Incarnation | `BuildId`, `RuntimeId`, and resource generations distinguish code and object lifetimes. |

One increasing integer cannot answer all these questions. In particular, processing order does not
establish that one independent completion caused another, and a logical commit does not identify
the pixels currently visible on a physical display.

## Published models and their limits

**Causal order.** Lamport defines happens-before through process order, message transmission, and
transitivity; it is a partial order. Logical clocks can extend this to a total order, but a smaller
clock value does not imply a causal relationship. [Time, Clocks, and the Ordering of Events][lamport]
provides a vocabulary for distinguishing arrival, adoption, and cause. It does not prescribe a UI's
policy for competing requests, such as whether an older search response should be discarded.

**Operation boundaries.** Herlihy and Wing's [linearizability][linearizability] makes an operation
appear to take effect between invocation and response while preserving the real-time order of
non-overlapping operations. This motivates stating precisely what an action acknowledgment means.
It does not make independently read fields a coherent aggregate query, nor does it establish GPU
completion or physical display. We must specify the observed aggregate and its commit boundary.

**Transitions, safety, and progress.** [Specifying Systems][tla], Chapters 2, 7, and 8, models behavior
as state sequences and distinguishes safety from liveness. Internal steps can be abstracted through
stuttering; a model step need not be a wall-clock tick. Octara can initially specify small transition
systems: rejecting a retired handle is a safety property; eventually processing an accepted input
requires progress assumptions. A loop that never changes state may preserve safety while failing
to make progress. A full formal model of the framework is not a prerequisite for these contracts.

**Discrete change and continuous sampling.** Elliott's [Push-Pull FRP][frp], Sections 2–5,
distinguishes discrete reactive values from time functions and combines push propagation with pull
sampling. Its behavior semantics samples combined values at the same time. This motivates keeping
application changes separate from frame cadence and giving a frame one declared sample time.
It does not make every Signals implementation FRP, or prove deterministic Rust callbacks, numerical
integration, external I/O, or native animation. Each needs its own explicit assumptions.

**Same-time feedback.** [Discrete-Event Systems][superdense], Sections 2–4, uses superdense tags
`(time, microstep)` to order events at the same model time. Its fixed-point and non-Zeno results
require stated mathematical conditions. Numbering UI passes does not establish those conditions.
Octara can expose microsteps for diagnostics, but needs bounded work and an explicit unfinished
result when layout or native feedback does not converge. An arbitrary retry count is not a proof.

**Identity versus observed value.** Clojure's [Values and Change][clojure] distinguishes a stable
identity from its successive states and from immutable values. This motivates resource handles
for live entities and stable observation values for their state at a declared revision. Octara can
keep mutable internals; adopting this distinction does not require persistent data structures
everywhere or serialization of arbitrary Rust application state.

These models guide our questions. Their proofs apply to their defined languages and assumptions;
they do not transfer to Octara merely because its terminology or dependency graph looks similar.

## Unix and Plan 9: a small interface that exposes the real system

[The Use of Name Spaces in Plan 9][plan9] describes services exposed through named resources and
a common protocol. Its window system serves the same console and graphics interfaces it consumes,
which permits recursive hosting; imported process resources work with existing debugging tools.
The useful inference is that inspection and automation should address the same runtime resources
used by the application: windows, nodes, bindings, tasks, requests, diagnostics, and events.

Octara need not implement a filesystem or encode every operation as bytes. A discoverable resource
protocol with explicit lifetimes lets the CLI, inspector, and Agent reuse the same facts and actions.
Host adapters should report their capabilities so that a real window, a simulator, and a headless
host can share semantics while honestly exposing different evidence.

Linux also supplies counterexamples to simplistic interface analogies. Its [/proc documentation][proc]
states that an open descriptor for a dead process does not start acting on a later process that
reuses the PID. The [inotify manual][inotify] documents coalescing and queue overflow: a stream of
notifications is not an infallible history. Names and composable tools need lifetime and loss rules.

## Seven failure modes to carry into the design

The following are failure modes illustrated or defended against by primary contracts, source
comments, and tests. They are not a claim that every cited project currently has these bugs.

1. **A delayed result targets the wrong lifetime or content.** Linux's descriptor rule prevents
   recycled PIDs from retargeting existing handles. Exact's [collection regression test][exact-row]
   preserves row identity while changing its measurement epoch, then rejects the old measurement.
   **Octara inference:** validate runtime, resource allocation generation, and the relevant content
   or request generation before applying delayed work. A stable row key is not a measurement epoch.

2. **A query combines individually valid facts from different moments.** GPUI's
   [platform contract][gpui] samples geometry in `prepare_frame` and requires geometry getters to
   remain stable through drawing without callback reentry. **Octara inference:** observe a declared
   logical revision and sample time in one query boundary. Host facts from other boundaries carry
   their own provenance; do not silently label them as part of the same snapshot. Observation must
   not implicitly advance time or settle pending work.

3. **A controlled text write destroys composition.** Exact's [iOS composition tests][exact-ime]
   hold transformed application values while text remains marked, then apply the appropriate value
   after commit or cancellation. **Octara inference:** model native editing, marked text, selection,
   and application writeback as a protocol rather than assigning a string after every callback.
   These tests call `UITextInput` APIs; they do not establish real keyboard end-to-end coverage.

4. **A feedback loop is mistaken for useful progress.** Masonry's [render root][masonry] documents
   an IME-area echo loop on KDE/Wayland and suppresses unchanged output. Its rewrite loop also caps
   passes, warns, and schedules another frame if invalidations remain. **Octara inference:** make
   feedback idempotent where possible, budget work, expose remaining dirty causes, and distinguish
   convergence from yielding. Neither equality checks nor pass limits alone guarantee convergence.

5. **A virtual clock is assumed to control native motion.** Exact's [Agent clock test][exact-clock]
   starts a four-second UIKit animation; settling waits within a wall-clock budget and reports
   `settled: false` with a motion reason. **Octara inference:** state which clocks are controlled,
   bound settling, and report uncontrolled native work. Advancing application time cannot by itself
   freeze a keyboard, compositor, network, or native transition.

6. **Submission is reported as display.** Qt's [`frameSwapped` contract][qt] means queued for
   presentation. **Octara inference:** distinguish logical commit, scene production, submission,
   and the presentation feedback a backend actually supplies. An acknowledgment or screenshot
   needs the strongest observed stage and its source; neither should promise physical scanout
   without evidence from a platform facility that establishes it.

7. **Snapshot followed by watch silently misses a change.** Inotify documents lost events and
   cache rebuilding after overflow. **Octara inference:** establish snapshot plus cursor and
   subscription under one ordered boundary, or replay retained events from that cursor. If the
   cursor expired or a subscriber fell behind, report a gap and require a fresh snapshot. Bounded
   observation history is sufficient; this does not require durable event sourcing.

## What these constraints imply for Agent-first architecture

The minimal improvement loop is direct: inspect current context, act through a declared path,
observe feedback, edit, and try again. The framework supplies evidence; the Agent decides how
to improve the application. It does not need a separate autonomous optimization subsystem.

The selected runtime design gives existing owners clear update, lifetime and failure boundaries.
The Agent interface starts with direct reads, actions and bounded waiting, exposing source,
current values, diagnostics, pending work and real host feedback. Detailed causal links remain
an extension driven by actual debugging tasks. Typed inspection adapters can expose application
values when useful; mandatory serialization of every Rust value is unnecessary.
Semantic actions and platform input should identify which path ran, because invoking a handler
does not establish hit testing, keyboard behavior, navigation transitions, or accessibility behavior.

Start with one application owner and explicit external ingress. Add parallel computation only
where measurements and ownership rules justify it. Keep stale-result rejection separate from
transport cancellation and from rollback of external side effects. These are design choices to
validate in Octara, not requirements imposed by the cited theories.

## Source boundary

This note uses targeted sections of original papers/books and official documentation or source,
not a claim to have exhaustively reviewed the literature. Exact was inspected read-only at commit
`08c889a9de21a30b7c3b308422fc588cf1a480d3`; its tests were read, not executed. GPUI and Masonry links
track upstream `main` and may move. Mobile examples motivate future host contracts; they do not
mean Octara currently supports those hosts. No cited formal result proves the proposed runtime.

[lamport]: https://lamport.org/pubs/time-clocks.pdf
[linearizability]: https://www.cs.columbia.edu/~wing/publications/HerlihyWing90.pdf
[tla]: https://lamport.org/tla/book-21-07-04.pdf
[frp]: https://conal.net/papers/push-pull-frp/push-pull-frp.pdf
[superdense]: https://ptolemy.berkeley.edu/publications/papers/05/DE_Systems/DE_SystemsDraft.pdf
[clojure]: https://clojure.org/about/state
[plan9]: https://9p.io/sys/doc/names.html
[proc]: https://docs.kernel.org/filesystems/proc.html
[inotify]: https://man7.org/linux/man-pages/man7/inotify.7.html
[gpui]: https://github.com/zed-industries/zed/blob/main/crates/gpui/src/platform.rs#L1182
[masonry]: https://github.com/linebender/xilem/blob/main/masonry_core/src/app/render_root.rs#L758
[exact-row]: https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/runner/src/instance/collection/tests.rs#L1023
[exact-ime]: https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/tests/ExactKitTests/FieldCompositionIOSTests.swift#L135
[exact-clock]: https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/tests/ExactKitTests/AgentClockIOSTests.swift#L367
[qt]: https://doc.qt.io/qt-6/qquickwindow.html#frameSwapped

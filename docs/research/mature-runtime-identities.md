# Identity and progress in mature UI runtimes

Research baseline: 2026-11-14. This compares Chromium/Viz and DevTools, Flutter/Dart,
Jetpack Compose, and GPUI, with debugging and tracing protocols as supporting references.
The question is which identities and completion boundaries Octara needs for reliable live tools.

The strongest common pattern is **a small identifier with a precisely defined owner, lifetime,
and purpose**. The inspected implementations do not expose one uniform set equivalent to
the initial Octara draft's `BuildId`, `RuntimeId`, `TurnId`, `CauseId`, `Revision`, and `SampleTime`.
Their mechanisms support separating those meanings, but not making every meaning a mandatory
top-level protocol field or treating a counter as evidence that an entire UI has finished.

For Octara, prioritize runtime incarnation, lifetime-safe handles, operation correlation, declared
clock domains, and truthful completion stages. Keep turn/cause metadata incremental. The selected
architecture leaves universal app-wide layout convergence out of its first implementation.

## Source snapshots

These are fixed development source snapshots, not claims about the exact behavior of a shipped
stable release. The linked tests provide executable specifications; they were read, not run here.

| Project | Source snapshot |
| --- | --- |
| Chromium | `1c533856ad9f464b5e4889a82b4000a684e2d702` |
| DevTools Protocol | `c092aa9630c73ff7765a8fe0f1d3ff4847a3676a` |
| V8 | `dc7873b18ab6f3003e0bed1dd856bf215fd6bbae` |
| Flutter | `8c321e4a93a036d767d98e0f7780bcb20ac170f0` |
| Dart SDK | `f3f72dd63911938effc75feacc86f52022216c67` |
| AndroidX | `513b42ad3bf8a14efc0593200ecef7d1afcf5a54` |
| GPUI/Zed | `f0fa51bb510fc91f6da7ec55f3e0f317f1f975ec` |

Supporting documentation was accessed on 2026-11-14. OpenTelemetry's site identified its core
specification as 1.62.0 and semantic conventions as 1.45.0. Public names below remain the projects'
own terms; similar names do not establish equivalent guarantees.

## Chromium separates producers, surfaces, submissions, and presentation

| Mechanism | Actual purpose and scope |
| --- | --- |
| `FrameSinkId` | Identifies a submitting client/sink using two namespace components; [definition][frame-sink] |
| `LocalSurfaceId` | Identifies a surface/configuration within an embedding lifetime, using parent/child sequences and an embedding token |
| `BeginFrameId` | Identifies a frame-production opportunity using source identity and sequence number |
| `frame_token` | Correlates a submitted frame with later processing/presentation feedback within its producer lifetime |
| Frame and feedback timestamps | Describe scheduling and observed pipeline stages, separately from identity |

A surface may receive many frames without changing its local surface identity. Parent and child
configuration changes can be independently newer: the regression test compares `(6,3)` and `(8,2)`
as incomparable, with `(8,3)` newer than both. The embedding token additionally has a security role;
it is not merely an allocation generation. [Surface definition][surface], [ordering test][surface-test].

Source identity matters even when sequence numbers exist. `BeginFrameAck` preserves the original
source because acknowledgments from an old source may arrive after an observer switches sources.
The receiver can reject those acknowledgments. [BeginFrame definitions and ACK][begin-frame].

The renderer crash/recreation test accepts frame token 1 after tokens 2 and 3 from the previous
RenderWidget lifetime. Numeric ordering is meaningful within the proper incarnation, not globally.
[RenderWidget regression test][frame-token-test].

Acknowledging a compositor frame releases submission backpressure. Presentation feedback is a
separate result and can report failure. Its flags distinguish hardware clock and hardware completion
evidence. Frame rejection can still produce an ACK and resource return; successful-presentation
callbacks have different conditions from failure-feedback callbacks. [Compositor protocol][viz-protocol],
[rejection path][frame-rejection], [feedback structure][presentation], [callback tests][presentation-test].

**Octara inference:** preserve owner/incarnation checks and distinguish operation completion from
frame submission and supported presentation feedback. Neither `BeginFrameId` nor `frame_token`
establishes an app-wide semantic revision. Do not import Chromium's parent/child sequencing unless
Octara actually has independent producers that need it. Source provenance can also be conditional:
Chromium's BeginFrame source-location fields are debug-only because of storage/binary cost.

## DevTools gives remote references explicit invalidation and release

CDP distinguishes frontend `NodeId`, backend node identity, JavaScript `RemoteObjectId`, and
execution-context identity. `DOM.documentUpdated` explicitly invalidates frontend node IDs.
Resolving a DOM node into a JavaScript object is an explicit protocol operation, not an assumption
that these identifiers share one namespace. [DOM protocol][cdp-dom].

`Runtime.evaluate` and `callFunctionOn` offer `uniqueContextId` because numeric context IDs can be
reused across processes. Their stated purpose includes preventing evaluation in the wrong context
after cross-process navigation. Remote objects can be released individually or by object group.
Cross-debugger stack references carry an optional debugger identity alongside their local ID.
[Runtime protocol][cdp-runtime].

V8's test creates two contexts and verifies that unique IDs select the intended values. Malformed
or unresolved unique IDs, or conflicting context selectors, return errors. A supplied but unresolved
unique identity fails instead of falling back; omitting selectors can select the default context.
[Context tests][v8-context-test],
[resolution implementation][v8-context].

**Octara inference:** names discover resources, operational handles select a particular occurrence,
and clients need explicit invalidation/release behavior. A stable source path or test name cannot
replace runtime identity. Detailed creation traces can be opt-in, as CDP's node stack tracing is.

## Flutter distinguishes inspected objects, tree identity, runtime, and clocks

`WidgetInspectorService.toId` associates an object with a tool ID and observation groups.
The same object can belong to several groups; releasing its final group removes the reference
record. In this source snapshot, object references use `WeakReference`/`WeakMap`: membership does
not keep ordinary inspected objects alive. Tests verify GC and failure of IDs after final release.
[Weak references][flutter-weak], [group implementation][flutter-inspector], [GC test][flutter-gc],
[group lifetime test][flutter-group-test].

Widget keys have a different purpose: element/state matching during tree updates. `GlobalKey`
documentation warns that recreating a key on each build discards state and can break an ongoing
gesture; its debug label does not determine identity. [Framework identity contract][flutter-key].

Hot restart can create new UI isolates without restarting the native process. Hot reload preserves
state, which can combine old state with new code differently from a cold start. Runtime incarnation,
code identity, and process identity therefore need separate treatment. [Restart implementation][flutter-restart],
[reload behavior][flutter-reload].

Flutter's Scheduler separates an epoch/time-dilation-adjusted frame timestamp from raw engine time.
Its private debug frame counter is assert-only. Engine `FrameTiming.frameNumber` correlates build/
raster metrics, with timing reports potentially batched; raster completion does not assert display.
[Scheduler][flutter-clock], [engine timings][flutter-timing], [batch reporting][flutter-timing-batches].

A source comment records a concrete clock failure: after a warm-up frame, an old timestamp could
make a new implicit animation jump straight to its end on the next real frame. Resetting the epoch
keeps adjusted animation time continuous. Clock adjustment tests also distinguish raw and adjusted
time. [Warm-up correction][flutter-epoch], [clock tests][flutter-clock-test].

**Octara inference:** an Inspector handle must not accidentally keep an unmounted subtree alive.
`RuntimeId` should identify the UI runtime incarnation, including in-process recreation. Sampling
time needs a clock domain and reload/epoch rule. Frame metrics and debug counters are not state commits.

## Dart and DAP make reference expiry part of the protocol

Dart VM Service treats object IDs as opaque and names as nonunique. Temporary IDs belong to an
isolate ID zone; the default ring has finite capacity. A reference may be expired by the zone or
its target may have been collected. Client-created zones are removed on disconnect. Unlike Flutter's
weak Inspector references, unexpired VM temporary IDs can retain their targets, so zone invalidation
and reuse affect memory. [ID contract][dart-ids], [zone lifecycle][dart-zones].

The Debug Adapter Protocol limits ordinary suspended-state scopes/variable references to that
suspension. Continuing execution invalidates them; other reference classes such as thread identity
have different lifetimes. Its message `seq` and response `request_seq` correlate protocol traffic,
not program state versions. [DAP lifetime contract][dap-overview], [protocol schema][dap-schema].

**Octara inference:** define resource lifetime, observation-reference lifetime, and operation
correlation separately. Distinguish retired resources, expired observations and unavailable values.
Use bounded reference metadata and clean it up when the observing client disconnects. Reference
retention policy must be deliberate because tools can otherwise change the system they observe.

## Compose snapshot IDs represent a real multiversion mechanism

Compose's `SnapshotId` participates in selecting a visible `StateRecord` from a state object's
record chain. Snapshots carry invalid sets; pinned snapshots affect record reuse, and mutable
snapshot application needs conflict handling. IDs can advance inside snapshot processing. This
is a state visibility algorithm, not an increment after a frame finishes. [Snapshot implementation][compose-snapshot].

Tests cover conflicting isolated writes, observers notified only after global apply notifications,
closed snapshots rejecting apply, and record reuse preserving a live old snapshot. Those properties
require the supporting storage and lifetime machinery, not just a version field. [Snapshot tests][compose-tests].

`Recomposer.changeCount` is closer to a lightweight change-batch counter, but its scope is one
Recomposer, and the inspected implementation increments before calling `applyChanges`.
While the recomposition loop is running, `awaitIdle` waits for earlier recompositions and their
successful application; an inactive Recomposer is considered idle. Neither case establishes layout,
drawing or presentation completion. Frame timestamps arrive separately through the frame clock.
[Recomposer count/apply][compose-recomposer], [idle contract][compose-idle].

Official phase guidance shows layout callbacks writing state and scheduling recomposition for a
subsequent frame, potentially causing visible jumps. It recommends coordinating layout through
appropriate layout primitives rather than that feedback pattern. [Phase feedback example][compose-phases].

**Octara inference:** keep the proposed publication revision lightweight unless snapshot isolation
is actually needed. Define the exact completed phase before naming an operation committed or idle.
Compose is not evidence that arbitrary layout feedback should converge within one global update.

## GPUI uses owner-aware entities and an outer update boundary

GPUI's `EntityId` is a generational slotmap key within its app's EntityMap. Handles carry a weak
association with their originating map; different apps can have the same numeric key. Debug paths
check context association. Weak upgrade checks whether the allocation is still present and whether its strong
reference count is already zero, before deferred cleanup executes. Cached drawing dependencies
also avoid re-subscribing to retired entities. [Entity ownership and checks][gpui-entities].

Tests prevent slot reuse before cleanup and cover weak upgrade after the last strong reference is
dropped, plus dependency reuse after an entity is released. An ID surviving in a cache does not keep its
resource valid. [EntityMap tests][gpui-entity-tests].

Nested updates use `pending_updates`; the outermost completion flushes effects. The flush handles
events, notifications, deferred work and lifecycle cleanup, then schedules window frames in the
production path. Drawing dirty windows within that loop is a test-mode behavior. The inspected
path does not define Octara's proposed universal turn/cause/revision scheme. [Update boundary][gpui-update],
[effect processing][gpui-flush].

**Octara inference:** adopt owner-aware generational handles and explicit outer update cleanup.
Do not infer app-wide layout/draw completion from an effects flush. A single UI thread can provide
useful boundaries without requiring every internal batch to become a public transaction identity.

## Tracing and artifact identity solve additional questions

OpenTelemetry separates `TraceId`, operation `SpanId`, parents and links. Links represent batching
and fork/join, where several independent inputs contribute to later work. Its context API restores
the previous context after a scoped attachment. Tracing also permits non-recording spans and limits
on retained events/links. [Trace API][otel-trace], [batch relationships][otel-overview],
[context scopes][otel-context], [retention limits][otel-limits].

**Octara inference:** use explicit task/request provenance and allow several contributing causes.
Tracing can export those relationships, but sampled or bounded traces cannot be the authoritative
registry for current resource validity, pending requests or cancellation. No OpenTelemetry dependency
is required to establish those facts.

Code identity also differs from instance identity. OpenTelemetry describes service version and
service instance separately. Sentry Debug IDs associate a transformed JavaScript artifact with its
source map after filename-based matching proved unreliable. Supported tooling can derive IDs from
file content; this is artifact identity, not a universal application state or process identifier.
[Service identity][otel-service], [artifact mapping][sentry-debug].

**Octara inference:** expose build/artifact provenance in connection metadata. Do not use a source
path, PID or package version alone as evidence that the attached code matches local edited files.

## Consequences for the Octara proposal

The following are design recommendations, not mechanisms already implemented by Octara.

| Proposed concept | Recommended treatment after this comparison |
| --- | --- |
| `BuildId` | Connection/build metadata tied to actual code artifacts; keep code installation distinct from state recovery |
| `RuntimeId` | Required incarnation of the UI runtime; changes on replacement even when the PID remains |
| Resource handle | Required lifetime/owner validation, with a generation or equivalent opaque identity |
| Operation correlation | Required to connect accepted actions to outcomes; distinct from an update or frame counter |
| `TurnId` | Internal ordering/debug metadata first; expose when needed to explain scheduling |
| `CauseId` | Incremental provenance, compatible with parent/link tracing; missing or truncated ancestry is explicit |
| `Revision` | Provisional publication/version concept whose completed phase must be specified and tested |
| `SampleTime` | A time value with clock-domain/epoch semantics, attached to sampled geometry and output |
| Frame feedback | A separate correlation and stage when supported; submission cannot imply presentation |

Retain the six semantic distinctions in the glossary, but avoid requiring clients or business
code to manage six top-level IDs for every interaction. A minimal attach/action/observe loop needs
runtime identity, an operational target, request correlation and honest results. Queries that return
sampled output additionally need its time and stage. More detailed provenance can be requested later.

The initial draft made `Revision` include all-window layout and feedback convergence.
The inspected projects do not establish that as a general requirement. The selected
[runtime architecture](../design/runtime-contract.md) instead starts with a narrow update boundary
and per-window output, leaving a universal revision out of the first implementation.
Restricted internal measurement feedback may justify a stronger guarantee; arbitrary application
callbacks and asynchronous native progress need explicit later-update policies.

Before freezing that contract, compare these alternatives with the same examples: two windows
sharing state, a virtual list measuring changing rows, and composition/native geometry callbacks.
Measure update latency and continued input handling as well as snapshot consistency. Keep bounded
settle as an explicit observation/test operation where appropriate. A useful revision requires a
clear guarantee and affordable progress behavior, not resemblance to another project's ID name.

## References

[frame-sink]: https://github.com/chromium/chromium/blob/1c533856ad9f464b5e4889a82b4000a684e2d702/components/viz/common/surfaces/frame_sink_id.h#L28
[surface]: https://github.com/chromium/chromium/blob/1c533856ad9f464b5e4889a82b4000a684e2d702/components/viz/common/surfaces/local_surface_id.h#L44
[surface-test]: https://github.com/chromium/chromium/blob/1c533856ad9f464b5e4889a82b4000a684e2d702/components/viz/common/surfaces/local_surface_id_unittest.cc#L65
[begin-frame]: https://github.com/chromium/chromium/blob/1c533856ad9f464b5e4889a82b4000a684e2d702/components/viz/common/frame_sinks/begin_frame_args.h#L54
[frame-token-test]: https://github.com/chromium/chromium/blob/1c533856ad9f464b5e4889a82b4000a684e2d702/content/browser/renderer_host/render_widget_host_unittest.cc#L1563
[viz-protocol]: https://github.com/chromium/chromium/blob/1c533856ad9f464b5e4889a82b4000a684e2d702/services/viz/public/mojom/compositing/compositor_frame_sink.mojom#L118
[frame-rejection]: https://github.com/chromium/chromium/blob/1c533856ad9f464b5e4889a82b4000a684e2d702/components/viz/service/frame_sinks/compositor_frame_sink_support.cc#L1154
[presentation]: https://github.com/chromium/chromium/blob/1c533856ad9f464b5e4889a82b4000a684e2d702/ui/gfx/presentation_feedback.h#L21
[presentation-test]: https://github.com/chromium/chromium/blob/1c533856ad9f464b5e4889a82b4000a684e2d702/cc/trees/presentation_time_callback_buffer_unittest.cc#L150
[cdp-dom]: https://github.com/ChromeDevTools/devtools-protocol/blob/c092aa9630c73ff7765a8fe0f1d3ff4847a3676a/pdl/domains/DOM.pdl#L945
[cdp-runtime]: https://github.com/ChromeDevTools/devtools-protocol/blob/c092aa9630c73ff7765a8fe0f1d3ff4847a3676a/pdl/js_protocol.pdl#L1355
[v8-context-test]: https://github.com/v8/v8/blob/dc7873b18ab6f3003e0bed1dd856bf215fd6bbae/test/inspector/runtime/evaluate-unique-context-id.js
[v8-context]: https://github.com/v8/v8/blob/dc7873b18ab6f3003e0bed1dd856bf215fd6bbae/src/inspector/v8-runtime-agent-impl.cc#L266
[flutter-inspector]: https://github.com/flutter/flutter/blob/8c321e4a93a036d767d98e0f7780bcb20ac170f0/packages/flutter/lib/src/widgets/widget_inspector.dart#L1385
[flutter-weak]: https://github.com/flutter/flutter/blob/8c321e4a93a036d767d98e0f7780bcb20ac170f0/packages/flutter/lib/src/widgets/widget_inspector.dart#L717
[flutter-gc]: https://github.com/flutter/flutter/blob/8c321e4a93a036d767d98e0f7780bcb20ac170f0/packages/flutter/test/widgets/widget_inspector_test.dart#L382
[flutter-group-test]: https://github.com/flutter/flutter/blob/8c321e4a93a036d767d98e0f7780bcb20ac170f0/packages/flutter/test/widgets/widget_inspector_test.dart#L1700
[flutter-key]: https://github.com/flutter/flutter/blob/8c321e4a93a036d767d98e0f7780bcb20ac170f0/packages/flutter/lib/src/widgets/framework.dart#L117
[flutter-restart]: https://github.com/flutter/flutter/blob/8c321e4a93a036d767d98e0f7780bcb20ac170f0/packages/flutter_tools/lib/src/run_hot.dart#L645
[flutter-reload]: https://docs.flutter.dev/tools/hot-reload
[flutter-clock]: https://github.com/flutter/flutter/blob/8c321e4a93a036d767d98e0f7780bcb20ac170f0/packages/flutter/lib/src/scheduler/binding.dart#L1087
[flutter-timing]: https://github.com/flutter/flutter/blob/8c321e4a93a036d767d98e0f7780bcb20ac170f0/engine/src/flutter/lib/ui/platform_dispatcher.dart#L2140
[flutter-timing-batches]: https://github.com/flutter/flutter/blob/8c321e4a93a036d767d98e0f7780bcb20ac170f0/packages/flutter/lib/src/scheduler/binding.dart#L275
[flutter-epoch]: https://github.com/flutter/flutter/blob/8c321e4a93a036d767d98e0f7780bcb20ac170f0/packages/flutter/lib/src/scheduler/binding.dart#L1059
[flutter-clock-test]: https://github.com/flutter/flutter/blob/8c321e4a93a036d767d98e0f7780bcb20ac170f0/packages/flutter/test/scheduler/scheduler_test.dart#L241
[dart-ids]: https://github.com/dart-lang/sdk/blob/f3f72dd63911938effc75feacc86f52022216c67/runtime/vm/service/service.md#L365
[dart-zones]: https://github.com/dart-lang/sdk/blob/f3f72dd63911938effc75feacc86f52022216c67/runtime/vm/service/service.md#L633
[dap-overview]: https://microsoft.github.io/debug-adapter-protocol/overview#lifetime-of-objects-references
[dap-schema]: https://github.com/microsoft/debug-adapter-protocol/blob/main/debugAdapterProtocol.json
[compose-snapshot]: https://github.com/androidx/androidx/blob/513b42ad3bf8a14efc0593200ecef7d1afcf5a54/compose/runtime/runtime/src/commonMain/kotlin/androidx/compose/runtime/snapshots/Snapshot.kt#L878
[compose-tests]: https://github.com/androidx/androidx/blob/513b42ad3bf8a14efc0593200ecef7d1afcf5a54/compose/runtime/runtime/src/nonEmulatorCommonTest/kotlin/androidx/compose/runtime/snapshots/SnapshotTests.kt#L157
[compose-recomposer]: https://github.com/androidx/androidx/blob/513b42ad3bf8a14efc0593200ecef7d1afcf5a54/compose/runtime/runtime/src/commonMain/kotlin/androidx/compose/runtime/Recomposer.kt#L727
[compose-idle]: https://github.com/androidx/androidx/blob/513b42ad3bf8a14efc0593200ecef7d1afcf5a54/compose/runtime/runtime/src/commonMain/kotlin/androidx/compose/runtime/Recomposer.kt#L1573
[compose-phases]: https://developer.android.com/develop/ui/compose/phases#recomposition-loops
[gpui-entities]: https://github.com/zed-industries/zed/blob/f0fa51bb510fc91f6da7ec55f3e0f317f1f975ec/crates/gpui/src/app/entity_map.rs#L201
[gpui-entity-tests]: https://github.com/zed-industries/zed/blob/f0fa51bb510fc91f6da7ec55f3e0f317f1f975ec/crates/gpui/src/app/entity_map.rs#L1305
[gpui-update]: https://github.com/zed-industries/zed/blob/f0fa51bb510fc91f6da7ec55f3e0f317f1f975ec/crates/gpui/src/app.rs#L1431
[gpui-flush]: https://github.com/zed-industries/zed/blob/f0fa51bb510fc91f6da7ec55f3e0f317f1f975ec/crates/gpui/src/app.rs#L2243
[otel-trace]: https://opentelemetry.io/docs/specs/otel/trace/api/
[otel-overview]: https://opentelemetry.io/docs/specs/otel/overview/#links-between-spans
[otel-context]: https://opentelemetry.io/docs/specs/otel/context/#attach-context
[otel-limits]: https://opentelemetry.io/docs/specs/otel/trace/sdk/#span-limits
[otel-service]: https://opentelemetry.io/docs/specs/semconv/registry/attributes/service/
[sentry-debug]: https://docs.sentry.io/platforms/javascript/sourcemaps/troubleshooting_js/debug-ids/

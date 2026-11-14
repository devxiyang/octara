# Agent context and live inspection

Status: the minimal Unix live connection is implemented; remaining boundaries are called out below.
This is the small live-tooling design paired with the
[runtime contract](runtime-contract.md). The [Exact comparison](../research/exact-runtime-simplicity.md)
explains the scope reduction; [Agent knowledge](agent-knowledge.md) describes the built facilities.

An Agent needs the running app's current facts, the source behind them, and a way to perform and
observe an interaction. Octara supplies this through its existing owners and inspector. The Agent
uses its normal edit, build, run and inspect loop to improve the app.

## Current implementation and boundaries

| Implemented | Remaining boundary |
| --- | --- |
| Opt-in Unix socket, one client, ordered JSON-line requests executed on the main thread | Windows transport and stronger attached-build identity |
| Live semantic values, bounds, actions, source and component call sites from the actual owners | Property origins, sampled opacity/color and stronger validation across tree replacement |
| Pending work, asks, diagnostics and explicit realtime settling | Reset/restart policy for broken internal invariants |
| Framework pointer/key/text dispatch and semantic actions with route/delivery feedback | OS input injection and native focus/IME/gesture qualification |
| Realtime framework double taps and long presses; native client-area size requests with actual-size feedback | Further native input and window-manager qualification |
| Asynchronous window GPU capture returning the existing capture report | Further platform display-capture support and coherent native/drawn geometry guarantees |

The shell enables attachment only when started with `OCTARA_INSPECT=/absolute/path.sock` on
macOS or Linux. The endpoint is a Unix socket with user-only permissions; an existing path is not
removed or replaced. The CLI's `inspect --connect PATH` and `run --connect PATH` connect directly,
without building a headless harness or requiring an app manifest. With no `--connect`, the existing
headless workflow remains available.

The existing Inspector request/reply format is reused; snapshots retain `octara.inspect/1`.
Live replies add clock, platform,
PID and route information; snapshot replies also list supported operations and transport. Input
replies report delivery, and wait replies report settling and pending reasons. A PID and framework
version are useful context, but do not prove the running build matches edited local source.

## Direct questions over the real app

Reuse the inspector's current request vocabulary. Queries read `App`, `Window`, Kernel, scheduler
and diagnostic state directly. Host geometry and captures come from the host that owns them. The
CLI connects, formats replies and relates source information; it does not reconstruct another UI.

Current queries answer which windows and controls exist, their semantic values and presented
bounds, where a selected node was created, what work is pending, and which diagnostics apply.
Property origins can be added where the existing binding or style owner can supply them.
Unavailable detail is explicit; arbitrary futures do not become introspectable. These queries do
not yet expose every sampled visual property or establish a shared native/drawn geometry version.

Business values may opt into a debug adapter. Ordinary Rust application state needs no mandatory
`Serialize` or `Debug` bound. Collect response data at the safe boundary before formatting; never
invoke arbitrary application formatting under a Kernel borrow. Source locations identify their
available build context rather than silently treating edited local files as the running code.

## One local connection with ordered replies

The server processes one local development client with serialized request/reply processing. A
request enters the host queue, executes on the UI thread against the actual app and selected
window, and finishes its reply before the next command is processed. Gestures, resize, capture
and settle wait asynchronously while the native event loop continues. Application async work may remain pending
after a reply.

This ordering provides request correlation without a general operation registry. A disconnected
connection fails its outstanding command; replies from it cannot be reused by a new connection.
If the transport cannot establish whether an action ran, its outcome is unknown. The client must
inspect or recover rather than assume failure and repeat the action. Explicit request IDs become
useful when actual concurrent commands or separate result delivery require them.

Replies belong to their originating connection and are discarded after it disconnects. The client
does not automatically resend a command after a transport failure. An accepted capture can finish
within its budget after disconnect; a new capture is refused while that job is still active.
Requests are limited to 1 MiB and replies to 16 MiB; an oversized reply closes the connection
because the operation may already have run. More explicit runtime/build
identity, including replacement inside one OS process, remains future work. No fixed set of six
public identifier types is required for this first connection.

## Targets and observation identity

Snapshots list the app's windows; a request's optional `window` number selects one, and node
actions resolve the existing test ID in that window. Missing windows or controls return an error.
Asynchronous gestures, resize, capture and settle recheck that their owning window still exists.
Multi-stage gestures retain the original node identity; finding a replacement with the same test
ID does not authorize the remaining input stages.

Stronger tree-incarnation and allocation-generation validation remains a design requirement when
tools retain targets across tree replacement. A current test ID lookup can resolve a newly created
control with the same name; it does not establish that this is the object an earlier read observed.
Reloaded state and matching source paths do not establish object or build identity.

Snapshots state the selected window and app time. Other live replies carry host context, but there
is no general observation version shared by separate reads. Adding one needs a concrete consistency
use: tools combining node, source and geometry must be able to detect incompatible observations
instead of attaching newer information to an older target. Target state, sampled framework geometry
and host-owned geometry also need explicit distinctions as that richer feedback is added.

## Reads and explicit waiting

The live host uses wall time. Processing a request may tick the app to elapsed host time and poll
ready work; it does not jump forward to a future timer as the headless host can. A sampling rewrite
or application-wide fixed point is not a prerequisite for attaching to a window.

Live `settle` explicitly waits for the selected window's reported work to finish while allowing
native events and async replies to proceed. Its default budget is 10 seconds and its maximum
`limit_ms` is 30,000. On expiry it returns `STEP_FAILED`, `live.settled: false` and pending reasons.
Ordinary pending network or native motion does not mean the runtime is damaged; reset/restart
policy for broken internal invariants remains separate work.

Live `double_tap` and `long_press` dispatch framework pointer events in stages separated by real
time. Double taps use mouse clicks within the recognizer's double-click interval; long presses
hold a touch through the configured long-press duration before release. The UI event loop keeps
running throughout. A removed or replaced target, a closed window or a disconnected client ends
the pending gesture and cancels held input where applicable. These requests do not jump the app
clock forward to manufacture recognition.
An independent double click may first wait for the preceding click run to expire. A hold that
does not invoke a long-press or context-menu handler is cancelled and returns `STEP_FAILED`.
If the first click or touch-down was already sent, subsequent failure retains
`live.delivery: "delivered"` so the client can inspect the result before deciding what to do next.

Live `size` accepts logical client-area dimensions and calls the native window's resize request.
It waits at most five seconds for the actual native inner size, GPU surface configuration and
framework viewport to agree. An already-correct size can complete immediately; a synchronous
native result uses the same GPU/framework resize handling as a native resize event. Requests must
be finite and positive, fit the GPU's physical dimension limit and allocate at most 32 megapixels.
The `live.resize` result includes `requested`, `actual`, `physical`, `scale`, `viewport`, `matched`
and `synchronized`. Physical-pixel rounding is allowed. If the window system constrains or refuses
the requested size, or synchronization times out, the request returns `STEP_FAILED` with the
observed dimensions rather than reporting the requested size as though it had been applied.

Virtual `advance` remains headless-only. Any future live clock control must state what it controls;
it cannot advance arbitrary OS animation, external I/O or compositor progress.

## Actions and feedback

An action reply distinguishes input delivery from subsequent waiting. Report whether the action
was delivered, refused before delivery, or has an unknown transport outcome. If delivered input's
follow-up processing or waiting fails, preserve that delivery fact and report the later failure
separately. A client must not repeat a delivered input as though it had been refused.

Report pending work when waiting ends before completion. A successful application update does
not assert physical display. Live capture asynchronously settles on wall time, obtains native
snapshots when needed, and uses the actual window's GPU renderer for painted pixels. It returns
the unchanged capture report in `Reply.capture` and leaves the app running. The report identifies
painted/display pixels, native coverage, pending work and any display-capture fallback. The
settling budget defaults to 5 seconds and is capped at 30 seconds; native snapshots may then wait
up to 5 seconds. This is not a guarantee of physical compositor presentation.

Semantic activation, framework pointer/key dispatch and OS input are different routes. Live
tap/secondary-tap/double-tap/long-press/type/key use framework dispatch; `size` identifies its native
window route, and semantic control operations identify their semantic route. This does not inject
OS pointer/key events or qualify native focus, keyboard/IME and
navigation gestures. Real simulator/device input is needed for those checks. Future mobile
adapters must report their actual capabilities; desktop/headless support does not imply mobile support.

## First acceptance and scope

Live attachment is disabled by default and enabled explicitly for development. The implemented
endpoint is local and user-only, and follows the existing protocol compatibility rules.

The real-window smoke script is [`examples/live_inspect.py`](../../examples/live_inspect.py).
Its acceptance loop attaches to a spawned window, inspects a control's value and source, operates
it, expects the result, captures GPU pixels and continues operating. It also exercises invalid
requests, bounded realtime settling, window selection and reconnect behavior. Running the script
against the built host is separate from describing the implementation here. Stronger stale targets across reload, disconnect
during input and richer delivered-input/follow-up-failure checks remain validation work as those
boundaries evolve.

Keep bounded diagnostic logs and direct rereads. Detailed dependency tracing, a general resource
router, an outcome registry and snapshot/watch can be added when a reproducible tool task needs
them; they are not prerequisites for this loop or an Agent self-improvement subsystem.

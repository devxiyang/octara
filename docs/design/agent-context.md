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
| Opt-in Unix socket, one client, ordered JSON-line requests executed on the main thread | Windows transport |
| Managed Cargo launch pairs an executable with a receipt and compares recorded workspace Rust files | Build provenance for unmanaged launches and inputs outside the recorded workspace |
| Live semantic values, bounds, actions, source and component call sites; opaque allocation targets checked before input and between phases | Property origins and sampled opacity/color |
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
replies report delivery, and wait replies report settling and pending reasons. A managed launch adds
the staged executable digest to `live.build`.

## Verify the running build

Use two terminals on macOS or Linux. Choose unused paths for the Inspector socket and receipt.

```sh
octara launch -p octara-examples --bin window --socket /tmp/octara-counter.sock \
  --receipt /tmp/octara-counter.json -- counter
```

```sh
octara inspect --connect /tmp/octara-counter.sock --receipt /tmp/octara-counter.json --json
octara run --connect /tmp/octara-counter.sock --require-build /tmp/octara-counter.json --script steps.jsonl --json
```

The launcher builds the requested Cargo binary or example, copies that executable into a private
temporary directory under Cargo's target directory, and records its SHA-256 digest. The child reports this digest for its
lifetime. The launcher waits for the child to exit before removing the copy, so a new build can
replace Cargo's output even while the earlier window is still open. Use ordinary local executables
that can be run directly, with any resources they need available from the caller's environment.
After an interrupted launch has stopped, `cargo clean` also removes its staged executable.

`inspect` reports the running and expected build digests in `provenance`. For a matching receipt
with a source record, `source` is `matches` or `changed` and `source_scope` is `workspace_rust`.
The comparison covers tracked and nonignored untracked Rust files under the Cargo workspace in a Git checkout,
sampled before and after the build. It is `unknown` when the files cannot be compared or the receipt
belongs to a different build. Cargo manifests, lockfiles, toolchain configuration, non-Rust assets
and external path dependencies are outside this source comparison.

With `--require-build`, the CLI checks the digest via `inspect` on the same ordered connection
that would carry the script. A different or missing digest returns `BUILD_MISMATCH` with refusal
before any script request is sent. The requirement selects an executable, so a script can still
intentionally validate it after editing the workspace. An unmanaged window remains inspectable
and has no managed build digest. Receipt checks are for the local development workflow.

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
invoke arbitrary application formatting under a Kernel borrow. Source locations describe the
compiled code. Use a matching launch receipt before following them into edited local files.

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
because the operation may already have run. Managed build identity applies to the launched
executable. Tree replacements inside the process use the target lifetime checks below.

## Targets and observation identity

Internally, Kernel owns a private tree identity (App namespace, window and reload generation),
while NodeId retains its existing slot generation. App owns TargetGuard and checks these values
directly between operation phases. Inspector alone encodes the opaque wire tokens and projects
Kernel facts into Node/Picked replies through `inspect::tree` and `inspect::pick`. Kernel does
not import Inspector types or serialization. The adapter reads current facts without retaining
a second tree; Window's existing inspection/picking conveniences call it. This separation keeps
the existing wire format and lifetime semantics, without an identity registry or new public IDs.

Snapshots list the app's windows; a request's optional `window` number selects one, and node
actions resolve the existing test ID in that window. Missing windows or controls return an error.
Asynchronous gestures, resize, capture and settle recheck that their owning window still exists.
Multi-stage gestures retain the original node identity; finding a replacement with the same test
ID does not authorize the remaining input stages.

Snapshots and picks return opaque `target` strings. A snapshot and each `windows` entry carry a
window/tree target; a node carries its allocation target. A request can return that value in its
optional top-level `target` field. The check binds the operation to the original app instance,
generational window, tree incarnation and, for node operations, generational node. Values and
geometry may change while the reference remains valid. Destroying the node, reloading its tree
or closing and reopening its window retires the reference, even when a test ID or title is reused.

Node operations require their own target, while `key` and `type` without an ID require the
original focused node. Window operations use the window/tree target. `select` accepts a window
target; it does not claim allocation checks for every endpoint in its document selection. A
retired or mismatched reference returns `STALE_TARGET`; a malformed or wrongly scoped target is
`BAD_REQUEST`. Null or invalid targets never silently become unchecked requests. Omitting
`target` retains the original test-ID protocol, which deliberately resolves the current node.

The operation retains its guard between input phases. Focusing can run application callbacks,
so text is sent only if the original node still exists and owns focus afterward. Pointer phases
and timed gestures likewise stop rather than continue into a replacement. `delivery: "refused"`
reports a pre-input refusal; `"delivered"` on a failed result records earlier input and a stopped
continuation. A successful final handler may remove its own node or change focus. The guard is
not rechecked after that legitimate final effect. Tokens are lifetime checks, not authority or
locks on application state; matching local source paths still do not prove build identity.

The `reload` operation invokes the actual application hot-reload path, preserving named state
while replacing view trees and retiring their targets. It is available in headless and live
hosts; live replies advertise `target_validation: "identity"`. The
[target validation fixture](../../examples/TARGETS.md) exercises replacement, reload and window
reopening in a real host.

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
against the built host is separate from describing the implementation here. The separate target
fixture records guarded operation results across same-ID replacement, reload and reopened
windows, including refusal before delivery and recovery after a fresh observation.

Keep bounded diagnostic logs and direct rereads. Detailed dependency tracing, a general resource
router, an outcome registry and snapshot/watch can be added when a reproducible tool task needs
them; they are not prerequisites for this loop or an Agent self-improvement subsystem.

# Octara Roadmap

Each phase lists what it delivers and, under Status, what of it is implemented. Concrete tasks are in
[TODO](TODO.md).

The work follows the simplified [runtime design](design/runtime-contract.md): the existing
inspector now connects to a real desktop window and exposes semantic values, source and pending
work. Next, tighten request lifetimes and update/output correctness at their actual boundaries. Direct native
editing and a mobile host pilot follow. Rust builders, Signals and the retained Kernel remain
the foundation; [the Exact comparison](research/exact-runtime-simplicity.md) explains the scope.

## P0 — Reactive Core

> Status: implemented in `crates/octara` (see [Reactive core](design/reactive-core.md)): the reactive kernel,
> incremental collections, virtualization, named state and hot reload, structured diagnostics, headless testing,
> from-scratch consistency checking, and window rendering on the GPU. Kernel operations already carry event
> causes; deeper reactive tracing is an optional debugging extension, not a missing prerequisite of the core.

Signals, Memo, Effects, Scoped Lifecycle, Retained UI Tree, Taffy, and basic GPU rendering.
Also establish a minimal Semantic Inspector, Source Mapping, and a Headless test entry point.

We suggest using a counter application for the first development loop, with these acceptance criteria:

- An Agent can find the button and the counter value in the semantic tree and trigger interaction through typed actions.
- A Signal update drives only the relevant bindings; the affected nodes and properties are visible in diagnostics.
- After dependencies switch dynamically, old dependencies no longer trigger Effects that have been unbound.
- After a Scope is unmounted, its Effects, subscriptions, and async callbacks no longer run.
- The same application logic runs in a Headless environment and can also connect to basic window rendering.
- Node source and registered diagnostics are available when inspecting a failing interaction.

The counter loop is the suggested first implementation slice; it does not represent the full scope of P0.

## P1 — Desktop Runtime

Deliver desktop runtime capabilities for macOS, Windows, and Linux, completing text, scrolling, windowing, input, and basic components.

> Status: text layout, editing, selection, IME, clipboard, fonts, pointer and keyboard input, focus, scroll containers,
> AccessKit and basic components are implemented. The winit shell supports multiple windows and system fonts;
> file dialogs use rfd, and macOS has native menus, a menu bar and alert sheets. CI is configured for builds and
> tests on all three desktops, with real-window conformance checks on macOS and Windows. An owned macOS backend,
> direct platform text input protocols, Windows/Linux native menu integration and fuller platform verification remain open.

- Text layout, editing, selection, IME, clipboard, and font handling.
- Pointer and keyboard events, focus, window lifecycle, DPI, and size changes.
- Basic controls, scroll containers, and AccessKit accessibility integration.
- Verify real platform behavior and state the supported scope for each desktop platform.

## P2 — Native Extensibility

Native Modules, Native Islands, WebView, system APIs, and the extension SDK.

> Status: Native Island on macOS (overlay web views, underlay maps, snapshots in captures); Windows and Linux are designed.

- Provide Rust Traits and platform adaptation boundaries.
- Verify how GPU and native components cooperate on layout, stacking, clipping, input, and focus.
- Report unsupported platform capabilities and property combinations.
- Provide typed, authorizable interfaces for system operations.

## P3 — Agent-Native Toolchain

Extend the minimal P0 capabilities into the Inspector, Source Map, Headless, event replay, structured diagnostics, and CLI.

> Status: the inspector protocol, captures, `check`, registered diagnostics, recording and replay of input, the `octara`
> CLI and skill are implemented. The CLI can drive a headless harness or attach with `--connect` to an opted-in
> winit app (`OCTARA_INSPECT=/absolute/path.sock`, Unix only). Ordered requests execute against the real owners
> on the main thread; framework input (including realtime double taps and long presses), realtime settling and
> asynchronous GPU captures are implemented. Captures return their report without exiting the app. Live `size`
> requests the native client-area size, waits at most five seconds for host/framework agreement, and reports
> actual dimensions in `live.resize`. Virtual `advance` remains unsupported; OS input injection and stronger
> build/target identity remain open. Broader tracing and continuous
> observation are optional extensions to justify through concrete tool tasks.

- Form a complete Generate → Compile → Run → Inspect → Test → Fix workflow.
- Validate the local single-client loop in real windows: find a control, read its value/source,
  act, expect the result and capture it; exercise errors, window lifetime and reconnect behavior.
- Query actual nodes, geometry, diagnostics and pending work through their current owners;
  application-value adapters stay optional. See [Agent context](design/agent-context.md).
- Extend connection information with reliable build identity and target lifetime validation.
  Current live replies report protocol/host capabilities, semantic/framework routes and input
  delivery; native behavior still needs real OS input and platform verification.
- Keep explicit, bounded realtime waiting and its pending reasons. Design any live clock control
  separately; headless virtual-time operations do not imply control over the OS or external I/O.
- State clearly the clock, async, and external I/O boundaries covered by replay.

Verify these capabilities with concrete edit/run/observe tasks. Detailed causal tracing, generic
snapshot/watch and evaluation of guidance can remain optional development tooling. They are not
required stages of a runtime self-improvement service.

## P4 — Mobile + Ecosystem

iOS, Android, cross-platform component libraries, the module ecosystem, and application packaging and release.

- Integrate UIKit, Android Views, and mobile platform lifecycles.
- Validate native editing, keyboard geometry and interrupted navigation through a focused host
  pilot with real simulator input before generalizing mobile adapter APIs.
- Complete mobile input, permissions, native components, and system capabilities.
- Build the toolchain for components, modules, examples, and release.

## Constraints Between Phases

The Agent Inspector and Headless are not left for design until P3. The boundary of Native Islands is
defined in the initial architecture, with the full SDK delivered in P2. Mobile platforms are considered from the initial architecture,
with the full implementation placed in P4.

Each phase advances through runnable applications and verifiable behavior; concrete APIs, crate layout, and dependency versions are
decided at implementation time, and designs not yet verified are not written up as features that already work.

# Octara Roadmap

Each phase lists what it delivers and, under Status, what of it is implemented. Concrete tasks are in
[TODO](TODO.md).

## P0 — Reactive Core

> Status: implemented in `crates/octara` (see [Reactive core](design/reactive-core.md)): the reactive kernel,
> incremental collections, virtualization, named state and hot reload, structured diagnostics, headless testing,
> from-scratch consistency checking, and window rendering on the GPU. Of the acceptance criteria below, all are met
> but the causal chain, which reaches kernel operations (a cause id for each event) and not the reactive layer.

Signals, Memo, Effects, Scoped Lifecycle, Retained UI Tree, Taffy, and basic GPU rendering.
Also establish a minimal Semantic Inspector, Source Mapping, and a Headless test entry point.

We suggest using a counter application for the first development loop, with these acceptance criteria:

- An Agent can find the button and the counter value in the semantic tree and trigger interaction through typed actions.
- A Signal update drives only the relevant bindings; the affected nodes and properties are visible in diagnostics.
- After dependencies switch dynamically, old dependencies no longer trigger Effects that have been unbound.
- After a Scope is unmounted, its Effects, subscriptions, and async callbacks no longer run.
- The same application logic runs in a Headless environment and can also connect to basic window rendering.
- The minimal causal chain interaction → Signal → Effect → property update can be recorded.

The counter loop is the suggested first implementation slice; it does not represent the full scope of P0.

## P1 — Desktop Runtime

Deliver desktop runtime capabilities for macOS, Windows, and Linux, completing text, scrolling, windowing, input, and basic components.

> Status: text layout, editing, selection, IME, clipboard, fonts, pointer and keyboard input, focus, scroll containers,
> AccessKit and basic components are implemented; the winit shell runs on macOS (Windows and Linux build and test in CI).
> Several windows in the shell, menus, file dialogs and the owned macOS backend are not.

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
> CLI and skill are implemented; the evaluation task set is not.

- Form a complete Generate → Compile → Run → Inspect → Test → Fix workflow.
- Define versioned semantic and interaction protocols, diagnostic output, and test targeting.
- State clearly the clock, async, and external I/O boundaries covered by replay.
- Build a fixed task set and evaluate task success rate, compile-fix rounds, verification latency, and token cost.

## P4 — Mobile + Ecosystem

iOS, Android, cross-platform component libraries, the module ecosystem, and application packaging and release.

- Integrate UIKit, Android Views, and mobile platform lifecycles.
- Complete mobile input, permissions, native components, and system capabilities.
- Build the toolchain for components, modules, examples, and release.

## Constraints Between Phases

The Agent Inspector and Headless are not left for design until P3. The boundary of Native Islands is
defined in the initial architecture, with the full SDK delivered in P2. Mobile platforms are considered from the initial architecture,
with the full implementation placed in P4.

Each phase advances through runnable applications and verifiable behavior; concrete APIs, crate layout, and dependency versions are
decided at implementation time, and designs not yet verified are not written up as features that already work.

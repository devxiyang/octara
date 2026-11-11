# Octara Architecture

Status: architecture baseline. This document records the directions already decided; items marked "to be determined" need prototype verification.
For detailed designs of the reactive kernel and UI primitives see [`docs/design/`](design/); for research material see [`docs/research/`](research/).

## Product Definition

**Agent-First. Rust-Native. Cross-Platform.**

Octara is a modern cross-platform application platform designed for AI Agents: applications are written in Rust, using a
SolidJS-style fine-grained reactive model, rendered on the GPU by default, while also providing native components and operating system capabilities.

The product has four layers: UI Framework, Native SDK, Development Toolchain, and Agent Runtime.
The Agent development loop is Generate → Compile → Run → Inspect → Test → Fix.

## Six Core Principles

1. **Agent First**: an Agent can understand, generate, debug, test, and modify the UI.
2. **Rust Only**: the public APIs of applications and the framework are in Rust, with no dependency on a JavaScript Runtime.
3. **Fine-Grained Reactivity**: Signals, Memo, and incremental collections drive updates precisely, and a component runs only once;
   side effects use `watch`, separate from dependency tracking (see [Reactive core](design/reactive-core.md)).
4. **GPU First**: the UI is drawn with wgpu by default.
5. **Native Extensible**: Native Views and Native Modules are first-class citizens.
6. **Cross-Platform**: desktop first, with an architecture covering macOS, Windows, Linux, iOS, and Android.

## Technology Choices

| Module | Approach |
| --- | --- |
| Language | Rust 2024 |
| UI API | Rust builder (`el` / `row` / `col` / `text` with typed style methods), no view DSL |
| Reactivity | Fine-grained Signals, Memo, `watch`; named state; incremental collections (order-statistic tree); `LiveText` |
| Components | `octara-kit`: a component library driven by semantic tokens; the theme is a separate crate |
| UI structure | Retained UI Tree |
| Layout | Taffy |
| Text | Parley + editing subsystem |
| GPU rendering | wgpu |
| Windowing and events | winit + platform-native extensions |
| Native modules | Rust Traits + Crates |
| Native components | Native Islands |
| Accessibility | AccessKit |
| Async tasks | Custom single-threaded executor (scopes own tasks, application clock; see [Async](design/async.md)); I/O can be done on background threads with any runtime |
| Agent tooling | Semantic Inspector + Test Runtime |
| Development tools | Rust CLI + incremental builds |

The choices above describe the responsibilities of each component; they do not mean the dependencies have been introduced or the platform integrations completed.

## Reactive Runtime and Retained UI


Default update path:

```text
Signal
  → Reactive Effect / Property Binding
  → UI Property
  → Incremental Layout / Scene Update
```

A component builds views, scopes, and bindings. An ordinary state change updates the relevant properties without re-running the whole component function,
and without relying on a Diff of the whole UI Tree. Structural changes such as conditional branches and keyed lists need separate handling of child node
identity and of scope creation and destruction.

The Reactive Runtime is responsible for the dependency graph, Memo, Effects, scope lifecycle, and scheduling. The Retained UI Tree
is responsible for stable node identity, events, focus, layout association, and semantic information. Property changes need to be distinguished by impact scope
such as layout, paint, text, and semantics, so that an update enters only the necessary subsystems.

A Scope must own and clean up Effects, child Scopes, event subscriptions, and async tasks. Background tasks deliver results through the UI
Scheduler; a Scope that has already been destroyed should not keep receiving updates.

The details previously to be determined (handles and lifecycle, dynamic dependency switching, batching, execution order, reentrancy and cycle diagnostics, stale handle detection)
have been settled in the prototype; see [Reactive core](design/reactive-core.md).

## GPU and Native Capabilities

| Type | Implementation | Typical scenarios |
| --- | --- | --- |
| GPU View | wgpu | Regular UI, animation, charts, Canvas |
| Native View | OS UI Toolkit | WebView, special input fields, platform controls |
| Native Module | Rust + OS API | Files, windows, system menus, dialogs, notifications, permissions |

All three are managed through a unified Rust API. GPU Views and Native Views share the logical component tree, layout constraints, and
semantic interface. Compositing, stacking, clipping, transforms, input, and focus behavior of native components must respect platform restrictions;
it cannot be assumed that every platform supports arbitrary GPU scene effects.

The platform capability description of Native Islands, how restricted properties are reported, and how the window event loop cooperates with the native toolkit
all need to be determined through platform prototypes. Beyond Parley's text layout, editing, selection,
caret, IME, clipboard, and the platform text input protocol also need to be designed.

## Platform Targets

| Platform | GPU | Native extension | Platform priority |
| --- | --- | --- | --- |
| macOS | Metal | AppKit | P0 |
| Windows | D3D12 | Win32 / WinUI | P0 |
| Linux | Vulkan / GLES | GTK4 | P0 |
| iOS | Metal | UIKit | P1 |
| Android | Vulkan / GLES | JNI / Android Views | P1 |

Here P0/P1 are platform priorities; for development phases see the roadmap. The three desktop platforms are the first-phase delivery targets;
mobile platforms are considered from the architecture stage, but the first version is not required to be production-ready.

## Agent Runtime

| Capability | Goal |
| --- | --- |
| Semantic Tree | Structured description of components, roles, states, and executable actions |
| Source Mapping | Locate Rust source from a UI node |
| State Inspection | Inspect explicitly authorized reactive state |
| Causal Tracing | Correlate interactions, state changes, Effects, properties, and render updates |
| Deterministic Replay | Reproduce interactions and tests under controlled input |
| Headless Runtime | Run semantic and logic tests without a real window |
| Native Action Schema | Typed, authorizable system actions |
| Agent CLI | Create, compile, launch, inspect, and verify applications |

A minimal Inspector and Headless Runtime are established in P0, and P3 extends them to their full capabilities.

The Semantic Tree and AccessKit should share the base definitions of roles, labels, states, and actions. The Agent interface also
needs to express source, diagnostics, and authorization information. State inspection does not mean exposing arbitrary memory or all application state;
the observable fields and the boundary of sensitive data must be made explicit.

Source Mapping, runtime node identity, and test-oriented stable targeting need to be defined separately, to avoid using source
line numbers as node identity. The capacity and persistence policy of Traces must be bounded, so that a long-running application cannot
accumulate diagnostic data without limit.

Deterministic replay needs to control the clock, random sources, task delivery, and external I/O, or record their results. The first version should state clearly
the boundary of what can be replayed; beyond Headless tests, real GPU rendering, IME, accessibility, and native component integration still need to be verified.

The main Agent evaluation metrics are task success rate, compile-fix rounds, verification latency, and token cost; rendering
and interaction performance are also recorded, to avoid judging the framework's value by FPS alone.

## First-Version Scope

The first phase verifies three hypotheses:

1. Signals can reliably drive incremental UI updates and manage lifecycles correctly.
2. GPU UI and native components can coexist within clearly stated platform capability boundaries.
3. Built-in semantics, inspection, and testing measurably improve an Agent's application development efficiency.

Out of scope for now: a custom programming language or Contract DSL, a JavaScript / TypeScript Runtime, a Web rendering target,
a custom GPU backend or layout algorithm, dynamic plugins over an unstable Rust ABI, a full mobile SDK,
and complex multi-threaded rendering designed in advance for theoretical performance.

## Code Organization

| Package | Contents |
| --- | --- |
| `crates/octara` | `kernel` (node tree, invalidation classification, layout, semantics, counters), `reactive`, `view`, `app`, `testing` |
| `crates/octara-kit` | Component library, using only the public primitives of `octara` |
| `crates/octara-render` | CPU and GPU (wgpu) renderers for the display list |
| `crates/octara-shell` | Desktop window (winit), input, and accessibility bridge |
| `crates/octara-fonts` | Deterministic bundled CJK fonts (for tests and screenshots) |
| `crates/octara-cli` | The `octara` command; the checked-in `api.json` (public API, generated from nightly rustdoc JSON) |
| `examples` | Example applications, shared scenes, tests, and benchmarks |

## Core Architecture Decisions

- Fine-grained Signals and incremental collections drive the Retained UI Tree; ordinary updates do not use React-style full View Rebuilds.
- GPU First, with the extension boundary of Native Views / Native Modules defined from day one.
- Agent capabilities are built into the Runtime; semantics, source mapping, structured diagnostics, and Headless testing take part in the initial design.

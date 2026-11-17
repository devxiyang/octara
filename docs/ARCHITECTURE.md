# Octara Architecture

What is built, how its parts fit, and the decisions that shape them. Detailed designs are in [`docs/design/`](design/), research in [`docs/research/`](research/).

The 2026-11-17 [layered architecture redesign](design/layered-architecture-redesign.md) spans
runtime, document, primitives, kit and host/tooling boundaries. The first
[package split](design/package-boundaries.md) is implemented: window-independent values and
presentation protocols belong to `octara-primitives`; text shaping and fonts belong to
`octara-text`, with downward production dependencies.
The broader content, input and component-contract proposals are not yet implemented.
Its evidence is in the [foundation review](design/ui-foundation-review.md) and
[SwiftUI/Rune/Exact comparison](research/ui-composition-frameworks.md).

The selected direction is in [Runtime architecture](design/runtime-contract.md): keep Rust and
Signals, connect the existing Inspector to real windows, and improve concrete lifecycle/output
boundaries. [Agent context and live inspection](design/agent-context.md) defines that small tool
interface. These documents identify planned changes separately from the implementation below;
the [Exact comparison](research/exact-runtime-simplicity.md) explains the reduced scope.

The selected next architecture is [Runtime and Signals consolidation](design/core-consolidation.md):
native Rust execution with no VM, a small Signals API, explicit ownership and App update boundaries.
It includes a source-based refactor/removal inventory and ordered acceptance gates. Its
[Signals contract](design/signals-redesign.md) and [runtime contract](design/runtime-contract.md)
define the breaking replacement without compatibility aliases. Correctness, Signals API,
App root lifetime, style declaration ordering, exclusive element content and Inspector boundary slices are implemented;
broader primitive migration remains pending.
The sections below describe the current code.

The proposed next collection boundary is [Collection data, row identity and content lifetime](design/collection-regions.md).
It replaces application-maintained Vec/list mirrors and separates retained row identity from
current row input and replaceable content. It is not implemented; [latest Rune/Exact source
research](research/collection-regions.md) records the evidence and tradeoffs.

The [first-round scope](design/core-consolidation.md#scope-and-necessity) is limited to correct
value access, dependencies, lifetime and propagation. The remaining API/primitive inventory is
follow-up work, not a prerequisite for establishing those contracts.

## Product

**Agent-first. Rust-native. Cross-platform.** Apps are written in Rust with fine-grained
reactivity, drawn on the GPU, with native views and platform-owned interaction where appropriate.
The agent's loop is generate → compile → run → inspect → test → fix, and the framework answers
the agent's questions itself (semantic tree, diagnostics with causes, checks, scripted driving).

Principles:

1. **Agent first**: an agent can understand, generate, debug, test and change the UI.
2. **Rust only**: no JavaScript runtime, no view DSL; builder methods with GPUI's names.
3. **Fine-grained reactivity**: signals, memos and incremental collections update properties;
   a component runs once ([reactive core](design/reactive-core.md)).
4. **GPU first**, with native views as islands where they must be ([Native Island](design/native-island.md)).
5. **Cross-platform**: desktop first; the architecture covers iOS and Android.
6. **Reproducible tests**: headless runs, bundled fonts, an app clock and a CPU reference renderer
   control framework inputs. Replay also requires the same initial state and ordered external
   results; arbitrary I/O and host behavior are not controlled by input recording.

## Crates

| Crate | Contents |
| --- | --- |
| `octara-primitives` | Window-independent geometry, anchor placement, theme values, paths, images, icons, canvas recording, motion, node identity and presentation protocols |
| `octara-text` | TextEngine, Parley and deterministic monospace engines, fonts, shaping caches, line breaking and text geometry |
| `octara` | The core: reactive runtime, retained kernel, views, App/Window coordination, tasks, inspector, checks and diagnostics |
| `octara-macros` | `#[component]`: elements record the component function that built them and where it was called |
| `octara-kit` | Components (buttons, fields, select, switch, …) and streaming Markdown, built only on `octara`'s public API and theme tokens |
| `octara-render` | Primitive display list → pixels: CPU and wgpu renderers; no production dependency on the App runtime |
| `octara-shell` | Desktop windows (winit): multiple windows, input, IME, system fonts, scale, frames, AccessKit, file dialogs; macOS native web views, maps, menus and alerts |
| `octara-icons` | The Lucide set as path data (generated, checked in) |
| `octara-fonts` | A CJK subset for deterministic tests and screenshots |
| `octara-cli` | The `octara` command: `check`, `inspect`, `run`, `vocab`, `explain`, `skill`; public API extraction from source |
| `examples` | Example apps, their acceptance tests, benchmarks, screenshot and window tools |
| `apps/chat` | The AI chat reference app: threads, messages and model events; offline scripted or HTTP streaming `Model` implementations; JSON conversation storage and views composed from the kit |

Renderers and icons depend only on `octara-primitives` in production. Core depends on primitives
and `octara-text` plus the component macro; the text engine depends only on primitives within
the workspace. Kit and shell use the appropriate lower public APIs. The CLI reads their
canonical definitions and prelude aliases from the sources the application builds with.
`tools/check_layers.py` enforces permitted production edges, including optional and target-specific
dependencies. Dev dependencies may use the runtime to host integration tests.

## Inside `octara`

| Module | Responsibility |
| --- | --- |
| `reactive` / `task` | Dependency propagation, scope execution and teardown, scheduled work |
| `kernel` | Retained document, styles, text and editing geometry, layout, semantic and paint output |
| `view` | Construction, property bindings and structure coordination |
| `app` | App/Window ownership, entry boundaries, input routing, frame preparation and presentation |
| `platform` | Host protocols and headless adapters; consumes document editing/layout data |
| `inspect` / `capture` / `diagnostics` | Observation, operations, capture workflow and registered explanations |

Drawing and value modules are in `octara-primitives`; text layout and bundled fonts are in
`octara-text`. Kernel owns editing and consumes text layouts. Kernel no longer imports App editing types
or platform-owned cursor/layout types. App/View/task still share same-crate lifecycle coordination;
the package split does not claim those remaining internal boundaries are already separate crates.


### Source layout

The primitive modules live in `crates/octara-primitives/src/`; the runtime implementation follows responsibility:

```text
crates/octara/src/
├── reactive/
│   ├── mod.rs                 public reactive exports and on_cleanup
│   ├── graph.rs               graph storage, tracking and propagation
│   ├── graph/
│   │   ├── signal.rs          Signal/ReadSignal, memo, watch and public helpers
│   │   ├── scope.rs           scope ownership, named state, context and teardown
│   │   └── tests.rs           graph and lifetime regressions
│   ├── list.rs                incremental collections
│   ├── text.rs                text buffers
│   ├── seq.rs                 internal ordered/weighted index
│   └── log.rs                 bounded change history
├── task.rs                    scheduler, task identity/ownership and public exports
├── task/
│   ├── time.rs                app-clock waits and intervals
│   ├── background.rs          worker pool and one-shot/native replies
│   ├── channel.rs             channel ownership, waits and named receivers
│   ├── channel/tests.rs       channel-private invariant tests
│   ├── async_memo.rs          async derivations and result admission
│   ├── tests.rs               scheduler and cross-primitive lifetime regressions
│   └── test_support.rs        shared test-only drop/wake probes
├── app.rs                     App/Window state, entry boundaries and lifecycle
└── app/
    ├── frame.rs               mounting and bounded frame/layout preparation
    ├── presentation.rs        native view synchronization, capture and presentation
    └── …                      existing input, focus, IME, time and interaction modules
```

The new implementation submodules are private. Public primitives are re-exported through the
existing `reactive` and `task` paths; moved methods remain inherent App/Window methods. Helpers
shared across child files use at most their enclosing module's visibility where possible;
channel internals remain private, with their tests colocated beneath that module. The primitive package owns moved value types; the remaining runtime modules stay in this crate.
CLI `vocab` resolves public paths and cross-crate prelude exports to their defining files.

## App and windows

- `App` is a handle on the app's state: the reactive runtime, the scheduler and its clock, the
  resources every window's kernel reads (theme, text system), the platform, and the windows. A
  `Window` is a handle on one window's state: its kernel, interaction, size and scale, root
  scope and platform window. Closing a window disposes its scope and frees it.
- `Window::kernel()` reads the retained tree; windows expose no mutable Kernel borrow. Views
  update nodes through bindings and events so scope cleanup, focus, IME and frame delivery stay
  coordinated. Use `App::set_text_engine` for shared text resources and semantic scroll events
  for scrolling. Counter resets, the benchmark layout switch and the reference-rendering culling
  switch are explicit instrumentation controls; they do not grant tree mutation access.
- Platform inspection is read-only too: `App::platform()` and `Window::platform_window()` expose
  their protocols without mutable borrows. The desktop shell mutates its private `WindowHost`
  adapter's native state; closing the window drops that adapter. Apps use scoped clipboard and
  window operations, and headless tests configure host capabilities before opening.
- The reactive runtime carries one owner (a window): every binding, memo, watch and task runs as
  the window that made it, so free functions (`state`, `spawn`, `current_window`) need no
  context passed.
- Every input is an `InputEvent` through `Window::event`, recordable and replayable; after each
  operation every window settles and presents a `Frame` (native view changes, text input, redraw)
  to its platform window. See [the core](design/core.md).

## A frame

1. A signal changes; the bindings that read it rerun, each writing one kernel property. Component
   functions do not rerun and the tree is not diffed. Structure (`show`, `each`) inserts and
   removes nodes with their scopes.
2. A property change marks its categories (layout, paint, text, semantics); only layout changes
   make the next frame lay out again.
3. `settle_frame`: run ready tasks, flush the reactive graph, lay out with taffy (cached per
   node), run after-layout hooks (virtual lists measure rows, scroll state, canvas sizes) and
   repeat while they change something (four rounds at most, then `LAYOUT_LOOP`). An unfinished
   attempt preserves pending work and withholds native/platform frame publication; later update
   boundaries can recover. [Window completion](design/window-completion.md) defines the outcome.
4. The display list is built from the tree in paint order (shadows, quads, images, icons,
   canvases, glyphs, paths, clips, opacity layers), with moving properties sampled at the app's
   time, skipping subtrees that cannot show; unchanged and at rest, the last one is reused.
   Renderers draw it.
5. When assistive technology is active, the shell sends AccessKit what changed: nodes whose
   semantics, children or place changed, and subtrees new to it. Each node is placed in its
   parent's space (a scroll container's offset is its own), so moving or scrolling sends one
   node, not its subtree.

## Kernel

- **View builders** carry one content enum, with input, image, icon and native settings attached
  to the matching payload. They derive the Kernel kind and mount only that content's bindings.
  Incompatible content-specific calls fail at the builder caller; shared children and semantic
  properties remain available on `El`.
- **Nodes** live in a slotmap (`NodeId`); each has a role, semantics, handlers, styles and state
  overrides, and one **content**: none, text, a text input (value, composition, selection,
  scroll), an image, an icon, a canvas recording or a native view. Its `Kind` follows from the content.
- **Styles** resolve against the theme (semantic tokens, scales, breakpoints) and the node's
  states (hover, focus, pressed, disabled, touch, window size class); inherited text properties
  flow down. View bindings retain their declaration positions and replace their own patches;
  the Kernel receives the ordered base and applies its existing overrides. Static-only elements
  need no slot cache. Missing tokens are diagnostics.
- **Layout** is taffy (flexbox and grid); texts and inputs are measured by the text engine, which
  caches by `TextKey` (a node and a slot) per paragraph and width.
- **Motion**: changed visual and layout properties can move (springs, curves) on the app clock;
  presented values are sampled per frame; leaving subtrees stay as ghosts until they finish.
- **Semantics**: roles, names, values, states and actions feed AccessKit and the inspector from
  one place; `check` judges unnamed controls, unlabeled canvases, contrast and target sizes.

## Rendering

- The display list is plain data in logical pixels; every item names its source node.
- The CPU reference renderer is deterministic: screenshots for agents and tests, and the oracle
  the GPU renderer is compared with (at most two levels per channel).
- The GPU renderer (wgpu) draws instanced SDF quads, shadows, images, glyph masks and path masks
  with one WGSL shader; glyphs (swash) and paths (kurbo strokes, zeno coverage) are rasterized
  on the CPU by code both renderers share, into atlases that start over when full.
- See [rendering](design/rendering.md) and [vector graphics](design/vector-graphics.md).

## Input, focus, async

Pointer recognizers (tap, double, long press, secondary, drag, swipe with momentum), focus order
and groups, keymap and commands ([input](design/input.md)); tasks, timers, channels and resources
owned by scopes on the app clock ([async](design/async.md)).

## Agent runtime

- `octara::inspect`: the semantic tree with test ids, bounds, actions, source locations and
  component boundaries; requests (tap, type, key, advance, settle, size, theme, expect, …) over
  JSON lines; `check` across themes and sizes.
- Kernel owns tree identity and read-only node facts, with no Inspector dependency. `inspect::tree`
  and `inspect::pick` project those facts into the protocol. `app::TargetGuard` checks typed
  tree/node identity and focus between operation phases; only Inspector encodes opaque tokens.
  Window's `inspect`, `pick` and `target` are convenience calls into that adapter.
- What people point at: `pick` names the element at a point; a picking window (`mod+shift+c` in
  debug shells) outlines what the pointer is over and copies the clicked element's line, with
  the component that made it.
- Diagnostics are registered codes with cause and doc anchor; tests keep the registry
  complete.
- By default the CLI builds a headless harness around an app's view (`[package.metadata.octara]
  view`). `inspect --connect PATH` and `run --connect PATH` instead attach to a running desktop
  app without building a harness. The `octara` skill teaches agents the framework
  ([agent knowledge](design/agent-knowledge.md)).
- The winit shell's [live inspector](design/agent-context.md) is opt-in through
  `OCTARA_INSPECT=/absolute/path.sock` on Unix. One client sends ordered JSON-line requests;
  the shell executes them on the main thread against the actual `App`, windows and Kernels.
  Queries return the existing semantic values, bounds, source locations, diagnostics and pending
  work. Replies identify the realtime clock, input route and supported operations.
- Live tap/double-tap/long-press/type/key requests use framework input dispatch; they do not inject
  OS input. Multi-stage gestures wait on wall time while the event loop continues, validating the
  target again before completing. `size` requests the native client-area size and confirms the
  actual window, GPU surface and framework viewport agree within five seconds; `live.resize`
  reports the requested and actual dimensions. `settle` also waits on wall time. Captures run asynchronously through
  the window's GPU renderer and return the existing capture report without exiting the app;
  display-pixel requests retain the report's explicit fallback behavior.

Further dependency tracing or watch delivery must serve a concrete tool task. The Agent performs
improvement through its ordinary edit, run and observe loop.

The [Chat reference app](../apps/chat/README.md) tracks the current answer and title request per
conversation. Stop, replacement and deletion retire receivers before cancelling the transport;
queued or late events cannot update retired messages or release a newer answer's queue. Component
cleanup closes outstanding requests. Store clones serialize disk effects and discard stale saves
so an older worker cannot overwrite a newer save or recreate a deleted conversation.

## Platforms

| Platform | GPU target | Current integration and remaining work |
| --- | --- | --- |
| macOS | Metal | winit shell with AppKit web/map islands, menus and alert sheets; owned backend planned |
| Windows | D3D12 | winit shell; native menus and WebView2 in DirectComposition planned |
| Linux | Vulkan / GLES | winit shell; native menu bar and texture islands (WPE WebKit) planned |
| iOS | Metal | UIKit integration and mobile shell planned (P4) |
| Android | Vulkan / GLES | Android Views integration and mobile shell planned (P4) |

CI is configured to build and test all three desktop platforms and run the platform conformance
suite in real windows on macOS and Windows. Linux's real-window behavior and the full range of
desktop input, accessibility and native integration still need platform verification.

Desktop windows use system UI fonts and fallback by default. Headless tests and reference
screenshots can use bundled fonts for deterministic output. File dialogs are provided by rfd;
macOS uses AppKit for context menus, the menu bar and alert sheets. Other desktops draw menus
and prompts in the window, and tooltips are currently drawn on every desktop.

## Known limits

- **One thread.** Layout, display list, rasterization of glyphs and paths, and GPU submission run
  on the main thread (as in Rune and GPUI); the display list is data, so a render thread can
  come later.
- **Frames cost what shows.** Painting and hit testing skip subtrees whose drawing cannot reach
  the clip (each node knows how far its drawing reaches past its box), and a frame with nothing
  changed and nothing moving reuses the last display list (`bench -- --frames`: 0 µs idle,
  about 70 µs for a 5000-node list after a change, release build). Accessibility updates are
  incremental (4 nodes and about 40 µs after a press in that list, against 0.4 ms for the whole
  tree). Debug builds check both against a full rebuild every time they are reused or sent.
- **The shell is winit's**: it implements the [platform layer](design/platform.md)'s traits
  (clipboard, waker, multiple windows, native views, text input). On macOS, an owned
  `NSTextInputClient` child view answers document, selection, replacement and geometry queries
  synchronously; winit still owns the window and ordinary input delivery. Native tooltips and
  an owned macOS window backend remain open.
- **Native views**: macOS overlay web views and underlay maps are built
  ([Native Island](design/native-island.md)); Windows and Linux are designed.
- **Live inspection**: the opt-in transport currently requires Unix sockets (macOS/Linux).
  Live virtual `advance` remains rejected; the realtime host does not virtually advance the app
  clock. Native resize can be constrained or refused by the window system and reports actual
  dimensions on failure. A focused nonsecure editor exposes committed text, UTF-8/UTF-16
  selection, the displayed draft and marked range, preedit selection and caret geometry in the
  same snapshot; secure editors omit this context. Native queries reuse the text layout engine
  when the input method asks about a draft before its next frame. IME composition belongs to
  the focused field and is cancelled when that owner
  changes. Framework input does not qualify OS focus, IME or native gesture behavior; the
  [OS-input fixture](../examples/IME.md) records real keyboard/Chinese IME checks separately.
  Inspector operations can retain opaque window/tree or node targets across reads and reject
  replaced allocations before further input. The actual hot-reload path retires tree targets.
  Stronger build identity and richer sampled property information remain open.
- **Update and sample boundaries**: frames are submitted per window and display lists are pulled
  later. The first runtime refactor restores mounted callback owners, retires cancelled asks and
  revisits windows invalidated by later windows. Bounded unfinished reactive/window work remains
  inspectable. Stronger publication/native geometry contracts remain in the
  [runtime architecture](design/runtime-contract.md).

## Out of scope

A custom VM, bytecode or Plan interpreter, a custom language or contract DSL, a JavaScript runtime,
a web target, a custom GPU backend or
layout algorithm, dynamic plugins over an unstable ABI, and multi-threaded rendering designed in
advance.

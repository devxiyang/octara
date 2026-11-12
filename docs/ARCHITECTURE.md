# Octara Architecture

What is built, how its parts fit, and the decisions that shape them. Detailed designs are in [`docs/design/`](design/), research in [`docs/research/`](research/).

## Product

**Agent-first. Rust-native. Cross-platform.** Apps are written in Rust with fine-grained
reactivity, drawn on the GPU, with native views and OS capabilities where the GPU cannot serve.
The agent's loop is generate → compile → run → inspect → test → fix, and the framework answers
the agent's questions itself (semantic tree, diagnostics with causes, checks, scripted driving).

Principles:

1. **Agent first**: an agent can understand, generate, debug, test and change the UI.
2. **Rust only**: no JavaScript runtime, no view DSL; builder methods with GPUI's names.
3. **Fine-grained reactivity**: signals, memos and incremental collections update properties;
   a component runs once ([reactive core](design/reactive-core.md)).
4. **GPU first**, with native views as islands where they must be ([Native Island](design/native-island.md)).
5. **Cross-platform**: desktop first; the architecture covers iOS and Android.
6. **Deterministic by default**: headless runs, bundled fonts, an app clock and a CPU reference
   renderer give the same results on every machine.

## Crates

| Crate | Contents |
| --- | --- |
| `octara` | The core: reactive runtime, retained kernel, views, app runtime, tasks, motion, display list, paths, icons, canvas, inspector and checks, diagnostics |
| `octara-macros` | `#[component]`: elements record the component function that built them and where it was called |
| `octara-kit` | Components (buttons, fields, select, switch, …) and streaming Markdown, built only on `octara`'s public API and theme tokens |
| `octara-render` | Display list → pixels: the CPU reference renderer and the wgpu renderer |
| `octara-shell` | Desktop window (winit): input, IME, scale, redraw on demand, AccessKit, native views (macOS: WKWebView, placed with the frame) |
| `octara-icons` | The Lucide set as path data (generated, checked in) |
| `octara-fonts` | A CJK subset for deterministic tests and screenshots |
| `octara-cli` | The `octara` command: `check`, `inspect`, `run`, `vocab`, `explain`, `skill`, `api` |
| `examples` | Example apps, their acceptance tests, benchmarks, screenshot and window tools |

Everything but `octara` and `octara-macros` depends only on `octara`'s public API, which the CLI reads from the source an app builds with.

## Inside `octara`

```text
diagnostics ◄── reactive        signals, memos, watch, scopes; incremental lists; LiveText
                    ▲
kernel ─────────────┼──────────  retained node tree, styles and states, taffy layout, parley
  (paint, path,     │            text, motion, semantics, focus, display list, checks
   icon, canvas)    │            (does not depend on view or app)
    ▲               │
view ───────────────┘            elements and builders; each property is one binding
    ▲
app ◄── task                     App and its windows: input events, frames, recognizers, focus,
    ▲                            keymap, clock, recording; a single-threaded executor whose
    │                            tasks belong to scopes
inspect                          snapshots, scripts, `check` (the agent's protocol)
platform                         what the app needs from the system (traits); headless impl
```

## App and windows

- `App` is a handle on the app's state: the reactive runtime, the scheduler and its clock, the
  resources every window's kernel reads (theme, text system), the platform, and the windows. A
  `Window` is a handle on one window's state: its kernel, interaction, size and scale, root
  scope and platform window. Closing a window disposes its scope and frees it.
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
   repeat while they change something (four rounds at most, then `LAYOUT_LOOP`).
4. The display list is built from the tree in paint order (shadows, quads, images, icons,
   canvases, glyphs, paths, clips, opacity layers), with moving properties sampled at the app's
   time, skipping subtrees that cannot show; unchanged and at rest, the last one is reused.
   Renderers draw it.
5. When assistive technology is active, the shell sends AccessKit what changed: nodes whose
   semantics, children or place changed, and subtrees new to it. Each node is placed in its
   parent's space (a scroll container's offset is its own), so moving or scrolling sends one
   node, not its subtree.

## Kernel

- **Nodes** live in a slotmap (`NodeId`); each has a role, semantics, handlers, styles and state
  overrides, and one **content**: none, text, a text input (value, composition, selection,
  scroll), an image, an icon, a canvas recording or a native view. Its `Kind` follows from the content.
- **Styles** resolve against the theme (semantic tokens, scales, breakpoints) and the node's
  states (hover, focus, pressed, disabled, touch, window size class); inherited text properties
  flow down. Missing tokens are diagnostics.
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
- What people point at: `pick` names the element at a point; a picking window (`mod+shift+c` in
  debug shells) outlines what the pointer is over and copies the clicked element's line, with
  the component that made it.
- Diagnostics are registered codes with cause and doc anchor; tests keep the registry
  complete.
- The CLI builds a harness around an app's view (`[package.metadata.octara] view`) and drives it;
  the `octara` skill teaches agents the framework ([agent knowledge](design/agent-knowledge.md)).

## Platforms

| Platform | GPU | Native views | Priority |
| --- | --- | --- | --- |
| macOS | Metal | AppKit | P0 (the winit shell runs) |
| Windows | D3D12 | WebView2 in DirectComposition | P0 |
| Linux | Vulkan / GLES | textures (WPE WebKit) | P0 |
| iOS | Metal | UIKit | P1 |
| Android | Vulkan / GLES | Android Views | P1 |

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
- **One window in the shell**: an app holds any number of windows (headless, in the protocol
  and in captures), the winit shell shows one; OS popups and menus are
  [designed](design/windows.md).
- **The shell is winit's**: it implements the [platform layer](design/platform.md)'s traits
  (clipboard, waker, native views, text input); several windows, menus and an owned macOS
  backend are planned.
- **Native views**: macOS overlay web views and underlay maps are built
  ([Native Island](design/native-island.md)); Windows and Linux are designed.

## Out of scope

A custom language or contract DSL, a JavaScript runtime, a web target, a custom GPU backend or
layout algorithm, dynamic plugins over an unstable ABI, and multi-threaded rendering designed in
advance.

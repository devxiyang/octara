# Research: Rune (Anomaly / OpenCode)

Status: research notes. Based on the `README.md`, `PLAN.md`, `crates/rune` source and dependencies of the `dev` branch of
[anomalyco/rune](https://github.com/anomalyco/rune) at `3686e94` (2026-11-10), and its `research/` directory. Rune is
pre-release, so the content below may change upstream.

## Positioning

Rune is a Rust-native UI framework from Anomaly (the OpenCode team): its own renderer, text, layout, animation and
platform shell, aiming at a SwiftUI-on-iOS-class 120 Hz feel and being "written and verified by agents".
The first acceptance target is rewriting OpenCode Desktop in Rune; `rune-kit` ports components under the component names of
`@opencode/ui`, and the OpenCode theme sits outside the core as the first theme. MIT licensed, no external PRs accepted.

It overlaps heavily with Octara's positioning: Agent First, Rust Only, GPU rendering, headless verification.

## Tech stack

| Layer | Rune's choice | Explicitly rejected |
| --- | --- | --- |
| GPU | wgpu 30, one backend covering macOS, Windows, Linux, Web; native Metal only added when measurement proves it necessary | Hand-written Vulkan / D3D12 |
| Renderer | Custom: instanced SDF uber-quad + glyph atlas, Slug curve rendering for large/animated text and icons, `vello_gpu` sparse strips for complex vectors, backdrop blur | Full compute Vello, Skia |
| Text | Parley 0.12 + HarfRust + skrifa + fontique, GPUI-style two-level line cache (changing width does not re-shape) | cosmic-text, CoreText only |
| Editing / IME | Zed's `sum_tree` + rope; each platform's native text protocol implemented directly | winit IME, hidden native input boxes |
| Layout | Taffy 0.15 running on its own node arena; very long lists use a sum-tree row-height index | Custom layout engine, SwiftUI-style proposal layout |
| State | Keyed retained tree, rebuilt by plain view functions and then diffed (details below) | Fine-grained signals, GPUI Entity, immediate mode |
| Animation | The render thread owns the presented tree; each property is an analytic spring track that can be retargeted with velocity | Driving animation by re-running views on the main thread |
| Accessibility | AccessKit; the same tree is also the Agent Inspector's tree | Custom accessibility bridge |
| Shell | winit 0.30 first; switch to its own platform layer when text input lands (borrowing Zed's `gpui_*`, Apache-2.0) | Long-term reliance on winit, SDL3 |
| Hot reload | subsecond binary hot patching; tokens and live literals need no recompilation | View DSL interpreter |
| View API | Chained builder returning the type-erased `El`; no macro DSL | proc-macro DSL, deeply generic View types |

## Agent tooling

- **Scenes**: components register named states with fake data and fake services, which can be rendered in the real app,
  in offscreen tests and in a gallery.
- **Deterministic headless**: fixed fonts, clock, random seed and environment manifest; outputs PNG and tree JSON.
- **Inspector protocol**: versioned JSON over a socket; queries nodes by role/key/text, returns layout rectangles,
  style, accessibility role, the source `file:line` that produced the node, spring state and per-stage timings;
  supports input injection.
- **Time control**: pause, step, scale, seek; `settle` takes a deadline and lists unfinished items.
- **Tracing and gates**: the render thread emits motion-trace JSONL, layout/motion lint, and performance gates based on
  counts such as instructions, instances and allocations.
- **Misuse-resistant API**: explicit keys, error messages with fix suggestions, `llms.txt`, examples as tests.

## State model: design vs. implementation

The plan described in `PLAN.md` and `research/06-reactivity-animation.md` (2026-10-21):

- State lives in typed `Model<T>`, and `cx.read(model)` automatically subscribes the current view scope;
- Views are plain functions whose output is diffed by key; handlers are queued and get `&mut T`;
- Local state uses explicit string keys (`cx.state("open", ..)`), rejecting hook order and `#[track_caller]` location identity;
- Values that change every frame (animation, scrolling, dragging) do not go through views; they are carried by render-thread
  tracks or `Live<f32>`.

The actual API on `dev` is closer to the Elm architecture: `fn view(s: &S, cx: &mut Cx<S>) -> El`, a single application state,
handlers get `&mut S`, after which the view is re-run. There is no `Model<T>`; skipping unchanged subtrees requires manual
`memo(id, version, build)`. `research/38-view-rerun-and-commit-cost.md` records the extensive optimizations made for this
(partial remount of virtual lists, automatic Markdown caching, skipping apply for idle nodes), and notes (paraphrased):

> Rust arguments are evaluated eagerly and `&S` has no field-level read tracking; automatically skipping arbitrary view
> helpers without `memo` would need reactive state handles or a view macro/compiler rewrite.

Rune's reasons for rejecting signals, and Octara's response, are in [Reactive core](../design/reactive-core.md).

## Implications for Octara

**Directly borrowable:**

- Renderer route: instanced SDF quads + glyph atlas + `vello_gpu` sparse strips. According to Rune's ecosystem survey,
  Linebender moved compute Vello into `research/` in 2026, so it should not be the default renderer.
- Text stack and two-level line cache; IME must implement each platform's native protocol, since winit IME is not enough for production.
- Taffy running directly on its own node arena, maintaining only one tree.
- Animation on the render thread, independent of the state-model choice; any state approach should adopt it.
- Semantic tree shared with AccessKit; performance gates based on counts rather than wall-clock time; Scenes as the unit of testing and preview.
- No macro DSL as the View API.

**Where Octara can differ:**

- Rune only composes a few native layers on Apple platforms and provides no general Native Views / Native Modules SDK.
- Rune's diagnostics focus on rendering, animation and performance; Octara plans a causal chain of interaction → state → update → property.
- State model: Octara uses fine-grained signals, see [Reactive core](../design/reactive-core.md).

## Further reading

The documents in Rune's `research/` most relevant to Octara:

- `01-gpui-zed-warp.md`: GPUI's architectural limits and what can be borrowed
- `02-rust-ui-ecosystem.md`: October 2026 Rust UI ecosystem comparison matrix
- `05-text-ime-a11y-layout.md`, `23-text-quality.md`: text, IME, accessibility
- `06-reactivity-animation.md`, `38-view-rerun-and-commit-cost.md`: state model and re-run cost
- `09-agent-native-and-perf-targets.md`, `14-research-determinism-verification.md`: agent tooling and determinism

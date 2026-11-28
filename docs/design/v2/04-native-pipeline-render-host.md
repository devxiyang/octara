# Octara v2 Spec 04: Native Pipeline, Rendering, GPU Executor & OS Host (`octara-pipeline` + `octara-render` + `octara-gpu` + `octara-host-native`)

> Part of the [Octara v2 Clean-Sheet Architecture](../architecture-v2.md).

---

## 1. `octara-pipeline`: Native `HostEngine` Implementation

### Module Structure

```text
crates/octara-pipeline/src/
  lib.rs              # NativeEngine implementing octara_runtime::HostEngine
  layout/
    mod.rs            # Taffy incremental tree sync (consumes TreeDelta), anchored placement, safe-area clamping
    style.rs          # octara_model::LayoutStyle -> taffy::Style pure conversion
  text/
    mod.rs            # Parley FontContext/LayoutContext, system-fonts feature gate, Arc<str> paragraph cache
    geometry.rs       # Caret/selection rects, soft-wrap visual navigation, hit-to-byte, bidi/RTL ranges
  motion/
    mod.rs            # MotionEngine: spring/fade tracks, exit ghosts, scroll glides, caret blink, password char reveal
  paint/
    mod.rs            # Incremental PaintCompiler (DrawKey cache, ink-reach culling, opt-in check_paint_cache)
    scene.rs          # Frozen PaintScene (separates resting Parts from display-time-sampled TimedParts)
  a11y/
    mod.rs            # AccessKitProjector: octara_model::{Role, Live} -> accesskit::TreeUpdate, text runs, modal scope
```

### 1.1 Incremental `NativeEngine::project(&Tree, &TreeDelta, viewport, now)`
1. **Sync Taffy (`layout/`)**:
   - Create Taffy nodes only for `delta.created`; remove only `RetiredNode::Destroyed`.
   - Re-sync children arrays only for `delta.topology_dirty`.
   - Convert `LayoutStyle` and call `taffy.mark_dirty` only for `delta.layout_dirty`.
   - Measure `Text` / `TextInput` leaves via Parley cache (keyed by `(Arc<str> ptr/rev, TextStyle, font_gen)` — zero shaping on unchanged text).
   - Run `taffy.compute_layout(root, available)` only when Taffy has dirty nodes or `viewport` changed.
   - When Taffy is clean, reproject the dirty scrollport or editor frames, along with affected links and dependent anchored overlays.
   - Resolve anchored overlays (`delta.overlay_dirty` or anchor moved) against `safe_bounds(viewport)`.
   - Clamp scroll offsets against new content/viewport sizes (returning clamped feedback to `Tree`).
2. **Advance Motion (`motion/`)**:
   - Start/retarget analytic tracks when resolved visual/layout properties changed on existing nodes (never on `born == current_layout`).
   - On `RetiredNode::Exiting`, capture a paint snapshot and start the exit curve. Independently fading descendants have their own snapshots and tracks. Report `HostReply::ExitFinished(ticket)` when a track settles or motion is skipped. The tree retains the branch while any participant is pending.
   - Compute next animation deadline (`Option<Duration>`) for the host's frame clock.
3. **Compile Frozen `PaintScene` (`paint/`)**:
   - Re-evaluate per-node `DrawKey` for dirty inputs. Reuse unchanged `Arc<PaintNode>` subtrees and update the affected paths through balanced child spans.
   - Sample timed spans for motion and caret blinking. Resting spans contribute shared display-list parts.
   - Skip subtrees whose `ink` reach misses the viewport/scroll clip.
   - Produce `(Arc<DisplayList>, Option<Arc<PaintScene>>)`: when only pure paint tracks move (opacity, color, transition progress), `octara-host-native` samples `PaintScene::sample(display_target_time)` at vsync without re-running `App` or Taffy.
   - Debug full-uncached self-check (`debug_assert_eq!`) is gated behind `NativeEngineConfig::check_paint_cache` (enabled in conformance/unit tests, off by default in interactive debug runs).
4. **Build Incremental `accesskit::TreeUpdate` (`a11y/`)**:
   - Consumes shared modal visibility rules from `octara_tree::semantics::SemanticScope`.
   - Stage changed semantics, geometry, text runs and dependent relations. Commit publishes their delta. `NativePublication::snapshot_full()` builds the complete update from the committed semantics for AccessKit activation or an explicit request.
   - Maps `octara_model::Role` / `Live` cleanly to `accesskit::Role` / `Live` (the ONLY place in the entire workspace that touches `accesskit` types besides `accesskit_winit`).

Rollback restores committed projected frames, text presentations and paint subtrees. The font registry, shared shaping cache and retained Taffy state stay warm for replay of the pending tree delta.

---

## 2. `octara-render`: Pure Thread-Agnostic Renderer (Native + WebGPU)

**Hard constraint**: Zero `std::thread`, zero `winit`, zero OS FFI, zero swapchain management. Compiles cleanly on both native desktop/mobile AND `wasm32-unknown-unknown` (when `octara-host-web` enables `feature = "webgpu"`).

### Module Structure

```text
crates/octara-render/src/
  lib.rs
  plan.rs             # Pure functional RenderGraph planner (DisplayList -> FramePlan: passes, backdrops, transitions)
  batch.rs            # Device-independent paint requests
  schedule.rs         # Opaque cores, clip fast paths, and stable radix batching
  instances.rs        # Storage-buffer payloads for vertex pulling
  atlas.rs            # Texture arrays with guillotine packing, per-region LRU, and premultiplied image mips
  paths.rs            # Vector path rasterization (zeno + kurbo -> alpha mask tiles)
  glyphs.rs           # Subpixel glyph rasterization (swash -> alpha/color mask tiles)
  pipeline.rs         # Four wgpu micro-pipelines and depth/blend variants
  encode.rs           # FramePlan + Batches -> wgpu::CommandEncoder pass recording
  custom.rs           # Author paint programs: material, backdrop, transition, retained_canvas, error scopes
  outputs.rs          # PresentationHistory & RetainedCanvas texture cache
  profile.rs          # Optional GPU timestamp query pool & pass timings
  device_loss.rs      # Shared DeviceLoss state machine (used by both Native & WebGPU)
  cpu.rs              # Deterministic CPU Reference Oracle (shares glyph, path, and image mip preparation with GPU)
```

### Key Architectural Wins
- `plan.rs` is a **100% pure function** (`plan_frame(list, target_info) -> FramePlan`) with zero `wgpu::Device` dependency — pass dependencies, group opacity layers, backdrop `COPY_SRC` splits, and transition history bounds are unit-testable in milliseconds without a GPU.
- `cpu.rs` shares exact glyph & path rasterizers with GPU so pixel-diff tests (`CpuRenderer` vs `WgpuRenderer`) stay deterministic.

---

## 3. `octara-gpu`: Native-Only Event-Driven Device Executor

### Module Structure

```text
crates/octara-gpu/src/
  lib.rs
  generation.rs       # DeviceGeneration: atomic loss/stop flags, submission & readback leases, capacity accounting
  executor.rs         # DeviceExecutor worker thread, event-driven GPU completion wakeup
  schedule.rs         # Fair per-device job admission (Draw > SurfaceDrain > Resource > Capture > Canvas > Profile)
  readback.rs         # Async buffer mapping -> PaintedPicture + background PNG encoder thread
```

### 3.1 Event-Driven Completion Wakeup (Eliminating 1ms Spin-Poll)

v1's `DeviceExecutor` looped on `pending.recv_timeout(1ms)` + `device.poll(PollType::Poll)` whenever any GPU work was in flight (1,000 thread wakeups/sec on mobile/low-power).

v2 replaces fixed 1ms polling with a **unified `Condvar` + callback wakeup**:
1. `DeviceExecutor` owns a `Mutex<Inbox>` + `Condvar` (`wake_cv`).
2. Whenever a submission or readback is dispatched:
   - Register `queue.on_submitted_work_done(|| inbox.signal_gpu_event())` and `buffer.map_async(..., || inbox.signal_gpu_event())`.
   - Both host `Command` sends AND GPU completion callbacks notify `wake_cv` immediately.
3. On backends where `wgpu` requires periodic `device.poll(PollType::Poll)` to pump internal driver callbacks (e.g., certain Vulkan/Metal driver versions):
   - Use **adaptive exponential backoff** starting only after a frame is submitted (`2ms → 4ms → 8ms`, capped at `16ms` when idle-waiting, immediately reset to event-driven on callback arrival) instead of burning CPU at 1,000 Hz!
4. Surface reconfiguration (`DrainSurfaces`) remains a clean explicit barrier: `DeviceExecutor` drains in-flight queue submissions, hands exclusive surface-configure permission back to the host thread, and resumes on `Command::Resume`.
5. PNG compression (`png` crate) and file I/O run on a dedicated low-priority background thread in `readback.rs`, completely off both the UI thread and the GPU encoding thread.

---

## 4. `octara-host-native`: Pure-Rust winit Orchestrator + `OsHost` / `OsWindow` Traits

**Hard goal**: **Zero `#[cfg(target_os)]` business logic** inside the core event loop (`shell.rs`) or `HostWindow` state machine (`window.rs`).

### Module Structure

```text
crates/octara-host-native/src/
  lib.rs              # Public run / run_app / run_app_with_first_frame entrypoints
  config.rs           # Typed ShellConfig (sole place parsing OCTARA_* env vars)
  shell.rs            # Pure-Rust winit ApplicationHandler orchestrating App + DeviceExecutor + OsHost
  window.rs           # Pure-Rust HostWindow: surface lifecycle, FrameStamp admission, presentation state
  presentation.rs     # Per-window Opportunity / TargetGeneration / FrameReceipt state machine
  trace.rs            # Optional structured CPU/GPU/input trace writer
  os/
    mod.rs            # OsHost & OsWindow trait definitions + single factory `create_os_host()`
    apple_frame_clock.rs  # Shared CADisplayLink driver with on-demand drawable acquisition
    macos/            # AppKit NSTextInputClient, async Metal startup drawable acquisition,
                      # NSView attachment, module view composition, NSMenu bar, AppKit prompts, secure input
    ios/              # UIKit Scene, UITextField/UITextView sessions, interactive swipe-back, safe area
    desktop/          # Linux & Windows: winit IME adapter, arboard clipboard, rfd file picker, open::that_detached
```

### 4.1 OS services and window capabilities

\`OsHost\` creates platform windows and supplies app-wide clipboard, menu and lifecycle services. Each \`OsWindow\` provides text input and system service requests. The orchestrator uses optional interfaces where the backend supplies the corresponding capability.

| Interface | Responsibility |
| --- | --- |
| \`WindowTextInput\` | Synchronize editor sessions, handle keys and IME, and drain text input events. |
| \`WindowAsks\` | Route and withdraw native prompts. The app can present an ask through its own view. |
| \`WindowNativeViews\` | Project native islands, measure or capture them, and report their events. |
| \`WindowScene\` | Adjust mobile viewports, coordinate scene suspension and back gestures, and report scene events. |
| \`WindowPresentation\` | Drive a platform frame clock and acquire, submit and reconfigure native drawables. |

Windows start hidden. The window and surface request alpha for a transparent theme background or a supported native underlay. The host makes a window visible after the first generation-valid drawable is submitted for presentation.

### 4.2 Platform boundaries

\`shell.rs\`, \`window.rs\` and \`presentation.rs\` call capability interfaces without target-OS conditionals. Apple FFI stays in \`os/macos/\` and \`os/ios/\`. Desktop Winit hosts share the input and surface adapters and opt in to platform-specific services as available.

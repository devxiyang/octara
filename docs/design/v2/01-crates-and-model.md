# Octara v2 Spec 01: Workspace Topology, Naming Conventions & `octara-model`

> Part of the [Octara v2 Clean-Sheet Architecture](../architecture-v2.md).  
> **Rule**: Zero backwards compatibility with v1. Every crate name, module path, type name, and protocol field below is canonical for v2.

---

## 1. Canonical Workspace Layout & `tools/check_layers.py`

### 1.1 Workspace Members (12 Core Crates + Assets/Tools/Apps)

```text
crates/
  octara-model/        # Layer 0: Pure values, IR, 4-channel input, diagnostics (NO heavy/OS deps)
  octara-macros/       # Layer 0: #[ui] provenance and #[derive(Store)] field projections
  octara-icons/        # Layer 0: Lucide vector icon definitions (-> octara-model)
  octara-fonts/        # Layer 0: Bundled fallback font bytes (native/headless)
  octara-svg/          # Layer 0: Optional roxmltree SVG -> Icon/Path parser (-> octara-model)
  octara-release/      # Layer 0: Ed25519 signed packets, channels, crash-safe rollback (independent)

  octara-tree/         # Layer 1: Sparse ECS micro-kernel, StyleResolver, Transaction, TreeDelta,
                       #          InteractionArbiter, Logical EditorSession (-> octara-model)
  octara-runtime/      # Layer 1: TypedSlab ReactiveWorld, Pull-on-Demand Value+Delta DAG,
                       #          Capability Contexts, 5-Phase Turn Engine, HostEngine trait
                       #          (-> octara-model, octara-tree)

  octara/              # Layer 2: Clean public authoring facade (View, El, Styled, each, virtual_each, App, Window)
                       #          (-> octara-model, octara-tree, octara-runtime, octara-macros)
  octara-headless/     # Layer 2: Unstyled behavior parts and Copy controllers (-> octara)
  octara-kit/          # Layer 2: Themed components + streaming Markdown (allowed: octara, octara-headless, octara-icons)

  octara-pipeline/     # Layer 3a: NativeEngine (HostEngine): Taffy, Parley, MotionEngine,
                       #           PaintCompiler, AccessKitProjector (-> octara-model, octara-tree, octara-runtime)
  octara-render/       # Layer 3a/3b: Thread-agnostic RenderGraph(plan), batch, atlas, wgpu encode,
                       #              custom shaders, CPU Reference Oracle (-> octara-model)
  octara-gpu/          # Layer 3a: Native-only event-driven DeviceExecutor, surface/readback leases,
                       #           async background PNG encoder (-> octara-model, octara-render)
  octara-host-native/  # Layer 3a: Pure-Rust winit orchestrator + OsHost/OsWindow traits
                       #           (backends/{macos, ios, desktop}) (-> octara, octara-pipeline, octara-gpu)

  octara-host-web/     # Layer 3b: DomEngine (HostEngine): Delta DOM, UndoLog, JS batch measure,
                       #           Web IME, Canvas/Blob images, opt feature="webgpu"
                       #           (-> octara, optional octara-render)

  octara-inspect/      # Layer 4: AgentCommand/Receipt protocol, Replay, DST SimHarness,
                       #          UnixSocketTransport & BrowserBridgeTransport
                       #          (-> octara, optional octara-pipeline, optional octara-render)
  octara-cli/          # Layer 4: CLI (vocab, skill export, launch, dev restart, release)
                       #          (-> octara, octara-inspect, octara-release)

apps/chat/             # Reference compound application
examples/              # Native, iOS, Web & GPU Lab examples and live verification harnesses
```

### 1.2 Exact `ALLOWED` Edges for `tools/check_layers.py`

```python
ALLOWED = {
    "octara-model": set(),
    "octara-macros": set(),
    "octara-fonts": set(),
    "octara-release": set(),
    "octara-icons": {"octara-model"},
    "octara-svg": {"octara-model"},
    "octara-tree": {"octara-model"},
    "octara-runtime": {"octara-model", "octara-tree"},
    "octara": {"octara-model", "octara-tree", "octara-runtime", "octara-macros"},
    "octara-headless": {"octara"},
    "octara-kit": {"octara", "octara-headless", "octara-icons"},
    "octara-pipeline": {"octara-model", "octara-tree", "octara-runtime"},
    "octara-render": {"octara-model"},
    "octara-gpu": {"octara-model", "octara-render"},
    "octara-host-native": {"octara", "octara-model", "octara-pipeline", "octara-render", "octara-gpu"},
    "octara-host-web": {"octara", "octara-model", "octara-render"},  # octara-render is optional (feature = "webgpu")
    "octara-inspect": {"octara", "octara-model", "octara-pipeline", "octara-render"},
    "octara-cli": {"octara", "octara-model", "octara-inspect", "octara-release"},
    "octara-chat": {"octara", "octara-icons", "octara-kit", "octara-host-native"},
    "octara-examples": {
        "octara", "octara-fonts", "octara-icons",
        "octara-kit", "octara-pipeline", "octara-render", "octara-gpu",
        "octara-host-native", "octara-inspect", "octara-release",
    },
}
```

Kit, Chat, and the examples use `octara` or `octara::prelude` for authoring. The layer check covers their source paths as well as Cargo production edges.

### 1.3 Removed v1 Crates (Zero Legacy Preservation)
- `octara-primitives` → replaced by `octara-model` (with `png`, `zeno`, `roxmltree` stripped out).
- `octara-style` → style IR moves to `octara-model::style`; style resolution moves to `octara-tree::style` (eliminates unnecessary crate split).
- `octara-text` → Parley shaping & font registry moves into `octara-pipeline::text` (native-only; Web never compiles it).
- `octara-kernel` → replaced by `octara-tree` (micro-kernel) + `octara-pipeline` (Taffy/Parley/Motion/Paint/AccessKit).
- `octara-shell` → split cleanly into `octara-gpu` + `octara-host-native` + `octara-inspect`.
- `octara-web` → replaced by `octara-host-web`.

---

## 2. Canonical Naming Conventions

Strict consistency across all crates prevents cognitive friction:

| Concept | Canonical v2 Name | Banned / Retired v1 Name |
| --- | --- | --- |
| Window tree identity | `TreeId` | `TreeIdentity` |
| Monotonic tree revision | `TreeRev(u64)` | raw `u64` / `tree_revision` |
| Window viewport revision | `ViewportRev(u64)` | raw `u64` |
| Window settled revision | `WindowRev` | `WindowRevision` |
| Node handle | `NodeId` (SlotMap key) | — |
| Authored tree write batch | `Transaction` / `TxOp` / `TxBuilder` | `TreeBatch` / `TreeEdit` |
| Tree commit output | `TreeReceipt`, `TreeDelta` | separate dirty flags / full snapshots |
| Transient runtime write | `RuntimeOp` | 63 scattered `Kernel::set_*(&mut self)` |
| Reactive handle id | `ReactiveId` | `Idx` |
| Capability contexts | `BuildCx`, `ReadCx`, `WriteCx`, `CleanupCx`, `TaskCx` | ambient `Runtime::current()`, `app_state()` |
| Anonymous local signal | `cx.signal(init)` | `state("throwaway_key", ...)` |
| Reload/restart-safe state | `cx.persisted("key", init)` | overloading `state()` for both |
| Pluggable layout/host engine | `HostEngine` (`NativeEngine`, `DomEngine`) | `LayoutMode` enum inside `Kernel` |
| Native OS backend trait | `OsHost`, `OsWindow` | `WinitWindow` with 133 `#[cfg]` |
| Input channels | `InteractionEvent`, `EditorEvent`, `HostReply`, `AgentCommand` | 25-variant flat `InputEvent` / `Hosted*` |
| Semantic role / live | `octara_model::semantics::{Role, Live}` | re-exporting `accesskit::{Role, Live}` in core |

---

## 3. `octara-model` Detailed Module & Type Specification

### 3.1 Allowed Dependencies (Strict Minimalism)
- `bitflags`, `color`, `kurbo`, `serde`, `serde_json`, `slotmap`, `unicode-segmentation`
- Target-only on `wasm32-unknown-unknown`: `getrandom` (`features = ["wasm_js"]`)
- **Explicitly forbidden**: `png`, `zeno`, `roxmltree`, `accesskit`, `taffy`, `parley`, `wgpu`, `swash`.

### 3.2 Module Tree

```text
crates/octara-model/src/
  lib.rs
  identity.rs      # TreeId, NodeId, RowKey, TargetGuard, WindowId, TreeRev, ViewportRev, MeasureTicket, FrameStamp, EditorSessionId, NativeId
  geometry.rs      # Point, Vec2, Size, Rect, Edges, LayoutBox, WindowViewport, Side, Align, Placement, Placed
  theme.rs         # Color, ColorToken, ColorValue, Fill, Gradient, BoxShadow, Breakpoint, Theme, FontSizeToken, RadiusToken, ShadowToken
  style.rs         # LayoutStyle, Visual, StyleProp, State, When, Display, FlexDirection, FlexWrap, AlignItems, JustifyContent, Position, Overflow, Length, LayoutLength, GridPos
  motion.rs        # Curve, Spring, PropClass, Motion, Track (pure math, zero runtime/tree deps)
  semantics.rs     # Role, Live, Relations<T>, NumericRange, A11yAction, TextClip
  input.rs         # InteractionEvent, EditorEvent, HostReply, DroppedFile, ContactTarget, PointerId, PointerKind, Button, Modifiers, Key, KeyEvent, Chord, WheelPhase, BackPhase, TextEdit, CursorStyle
  text.rs          # Wrap, TextAlign, TextStyle, RichSpan, TextDocument, TextGeometry
  paint.rs         # DisplayList, Part, Item, Quad, Shadow, GlyphRun, Glyph, Path, PathCmd, PathPaint, Image, Sample, ObjectFit, CanvasOp, PaintProgram, PaintInput, PaintMount, CustomPaint, TransitionPaint, Picture
  native.rs        # NativeSlot (Overlay | Underlay), NativeChange, NativePlacement, NativeEvent, NativeMeasureRequest, NativeMeasureTicket, NativeSnapshotRequest
  ask.rs           # AskId, Ask, AskRoute, Answer, ServiceRequest, ServiceOutcome, ServiceError
  capture.rs       # PaintSample, PaintOutcome, PaintFallback, PaintReceipt, PaintedPicture, FrameReceipt, CaptureSource
  diagnostics.rs   # Code, Severity, Diagnostic, codes::* registry
```

### 3.3 Key Protocol Types

#### Identity & Versioning (`identity.rs`)
```rust
slotmap::new_key_type! {
    pub struct NodeId;
    pub struct WindowId;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TreeId {
    pub namespace: [u64; 2], // 128-bit process-random app incarnation
    pub window: u64,
    pub generation: u64,     // bumped on full tree replacement / reload
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TreeRev(pub u64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ViewportRev(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EditorSessionId {
    pub tree: TreeId,
    pub node: NodeId,
    pub session_gen: u64, // bumped on focus re-entry or document reset
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TargetGuard {
    pub tree: TreeId,
    pub target: GuardTarget, // Window | Node(NodeId)
}
```

#### Lightweight Semantics (`semantics.rs` — Zero `accesskit` Dependency)
```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Window,
    #[default]
    GenericContainer,
    Label,
    Heading,
    Button,
    CheckBox,
    RadioButton,
    Switch,
    TextInput,
    MultilineTextInput,
    PasswordInput,
    ComboBox,
    EditableComboBox,
    List,
    ListItem,
    ListBox,
    ListBoxOption,
    Menu,
    MenuItem,
    Tab,
    TabList,
    TabPanel,
    Dialog,
    AlertDialog,
    Status,
    Alert,
    Main,
    Group,
    Document,
    Link,
    Image,
    Slider,
    SpinButton,
    ProgressIndicator,
    WebView,
    ScrollView,
    Splitter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Live {
    Off,
    Polite,
    Assertive,
}
```
`octara-pipeline::a11y` maps `octara_model::{Role, Live}` ↔ `accesskit::{Role, Live}` on native targets; `octara-host-web` maps them directly to ARIA attributes.

#### Diagnostics Registry (`diagnostics.rs`)
The following v1 runtime diagnostics are **permanently deleted** because v2 enforces them at compile time:
- ~~`WRITE_IN_DERIVE`~~ (`ReadCx` has no write methods)
- ~~`WRITE_IN_BUILD`~~ (`BuildCx` has no write methods)
- ~~`STRICT_READ_UNTRACKED`~~ (`BuildCx` reads through explicit `read()` or `sample()`, never tracked `get()`)
- ~~`SIGNAL_BORROW`~~ (`&mut WriteCx` / `&mut ReadCx` prevents aliasing at compile time)
- ~~`VIEW_READS_STATE`~~ (pure collection closures receive no `Cx`; signal params pass explicitly via `*_with`)
- ~~`REGISTER_IN_DERIVE` / `REGISTER_IN_CLEANUP`~~ (`ReadCx` and `CleanupCx` lack registration methods)

Remaining runtime diagnostics (truly dynamic conditions):
- `CYCLE` (dynamic reactive dependency cycle)
- `FLUSH_RUNAWAY` (post-commit watch loop exceeding turn budget)
- `LAYOUT_LOOP` (causal project loop exceeding `WINDOW_ROUNDS = 16`)
- `READ_DISPOSED` / `WRITE_DISPOSED` (using a `Copy` handle whose owning scope was disposed)
- `WRONG_RUNTIME` (passing a `ReactiveId` into a different `App`)
- `DUPLICATE_PERSISTED_KEY` / `PERSISTED_TYPE_CHANGED`
- `DUPLICATE_ROW_KEY` / `LIST_ITEM_UNAVAILABLE` / `JOIN_FANOUT`
- `STALE_TARGET` / `NOT_SETTLED` / `APP_FAILED` / `MISSING_THEME_TOKEN` / `ELEMENT_CONTENT`.

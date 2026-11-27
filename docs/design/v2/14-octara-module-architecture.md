# 14 — Cycle Terminology Cleanup & `octara::module` Platform Extension Architecture (`0.0.8`)

> **Design premise**: Give Octara first-class, zero-fork platform extensibility across macOS, iOS, Desktop, Web, and Headless DST without bloating core crates, introducing circular dependencies, or relying on linker magic.  
> **Zero-baggage rule**: Use `AppError` / `Cycle` / `CycleReceipt` directly (no compatibility aliases), and migrate hardcoded native slots (`webkit`, `video`) and file pickers onto the single unified `octara::module` SPI.

---

## 1. Runtime Cycle

The atomic `Ingress → React → Commit → Project → Publish` execution pass is a **`cycle`**, with a shared error type at the application boundary:

| Legacy Symbol / Path | New Canonical Symbol / Path | Visibility |
| --- | --- | --- |
| `crates/octara-runtime/src/turn.rs` | `crates/octara-runtime/src/cycle.rs` | `octara-runtime` |
| `crates/octara-runtime/src/turn/` | `crates/octara-runtime/src/cycle/` | `octara-runtime` |
| `TurnError` | **`AppError`** | Public (`octara::AppError`, `octara::prelude::AppError`, `octara_runtime::cycle::AppError`) |
| `TurnReceipt` | **`CycleReceipt`** | Public (`octara::CycleReceipt`, `octara_runtime::cycle::CycleReceipt`) |
| `run_turn` / `drive_turn` / `begin_turn` | `run_cycle` / `drive_cycle` / `begin_cycle` | Crate-internal |
| `ReplayError::Turn` | `ReplayError::Cycle` | Public (octara-inspect) |
| `AppError::AppFailed` display text | `"application stopped after a failed cycle"` | Public |

**Requirements**:
1. Do not add deprecated type aliases.
2. Update all call sites across `octara-runtime`, `octara`, `octara-headless`, `octara-host-native`, `octara-host-web`, `octara-inspect`, `apps/`, `examples/`, and `docs/`.
3. Use `ReplayError::Cycle` in `crates/octara-inspect/src/replay.rs`.

---

## 2. `octara::module` Spine Architecture (SPI-7)

### 2.1 Zero New Core Crates & Strict Downward DAG

The module spine adds **zero new core crates**. It lives inside existing core layers and is re-exported through the single authoring facade:

```text
octara-model::module      (Layer 0: pure value types, zero OS / runtime / winit / web-sys deps)
        │
octara-runtime::module    (Layer 1: traits, ModuleRegistry, ModuleDispatcher, HostSurface,
        │                           NativeViewRegistry, Scope cancellation, Cycle ingress drain)
        ▼
  octara::module          (Layer 2: public authoring & plugin facade)
     ▲          ▲
     │          │
Hosts (L3)     Optional Official & Third-Party Modules (modules/*)
octara-host-native    modules/octara-webview
octara-host-web       modules/octara-video
(host supplies        modules/octara-picker
 HostSurface &        modules/octara-haptics
 lifecycle bus;       ...
 zero knowledge of    (depend ONLY on `octara` + target-gated OS/Web crates;
 specific modules)     NEVER depend on `octara-host-native` or `octara-host-web`)
```

**Layer enforcement (`tools/check_layers.py`)**:
- Introduce an explicit optional plugin layer (`modules/*`) whose production dependencies may point downward only to `octara` (and optionally `octara-icons` / `octara-kit` for UI wrappers), never to `octara-model` / `octara-runtime` / `octara-tree` directly and never to `octara-host-native` / `octara-host-web`.
- Core crates (`octara-model` through `octara-host-web`, `octara-inspect`, `octara-cli`) MUST NOT depend on any crate under `modules/*`.
- Apps and examples opt into only the modules they use via `Cargo.toml` + `AppDefinition::with_module(...)`.

---

### 2.2 Layer 0 — Pure Protocol & Value Types (`octara-model::module`)

`octara-model::module` remains 100% platform-agnostic and `wasm32`-clean (zero `objc2`, `web-sys`, `winit`, or raw pointers in serialized wire types):

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ModuleId(pub &'static str); // e.g. "octara.webview", "octara.picker", "octara.haptics"

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CallTicket(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SubscriptionId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostSurfaceKind {
    MacOs,
    Ios,
    Windows,
    Linux,
    Web,
    Headless,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionStatus {
    Granted,
    Denied,
    NotDetermined,
    Restricted,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleError {
    Unavailable,
    PermissionDenied,
    ActivationRequired,
    Busy,
    Cancelled,
    InvalidArgs(String),
    Failed(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AirspacePolicy {
    /// Native view mounts as an underlay beneath transparent Octara GPU pixels;
    /// `NativePlacement.covered` routes occluded overlay pointer hits back to Octara.
    #[default]
    UnderlayPreferred,
    /// When a host cannot composite Octara overlays above an overlay native view,
    /// capture a snapshot into the GPU layer and temporarily hide the live native view
    /// while any overlay intersects its bounds.
    SnapshotOnOcclude,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostLifecycleEvent {
    AppLaunched { urls: Vec<String>, files: Vec<PathBuf> },
    AppForeground,
    AppBackground,
    MemoryWarning,
    OpenUrls(Vec<String>),
    OpenFiles(Vec<PathBuf>),
    DockReopen { has_visible_windows: bool },
    WindowAttached { window: WindowId },
    WindowDetached { window: WindowId },
    WillTerminate,
}
```

`ModuleId` borrows from static input when deserializing. For dynamic input, decode the ID as a string and resolve it against the application's registered IDs.

---

### 2.3 Layer 1 — Runtime Contracts (`octara-runtime::module`)

#### 2.3.1 Target-Safe `HostSurface<'a>`
Because `web_sys::HtmlElement` on `wasm32` is a JS heap reference rather than a C pointer, `HostSurface<'a>` cleanly separates raw native OS handles from opaque borrowed host objects without pulling OS crates into `octara-runtime`:

```rust
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum RawNativeSurface {
    AppKit { ns_window: NonNull<c_void>, ns_view: NonNull<c_void> },
    UiKit { ui_view: NonNull<c_void>, ui_view_controller: NonNull<c_void> },
    Win32 { hwnd: NonNull<c_void>, hinstance: Option<NonNull<c_void>> },
    Wayland { display: NonNull<c_void>, surface: NonNull<c_void> },
    Xlib { display: NonNull<c_void>, window: u64 },
}

pub struct HostSurface<'a> {
    kind: HostSurfaceKind,
    window: Option<WindowId>,
    native: Option<RawNativeSurface>,
    opaque: Option<&'a dyn Any>, // Web host passes `&web_sys::HtmlElement`
}

impl<'a> HostSurface<'a> {
    pub fn kind(&self) -> HostSurfaceKind { ... }
    pub fn window(&self) -> Option<WindowId> { ... }
    pub fn raw_native(&self) -> Option<RawNativeSurface> { ... }
    pub fn downcast_ref<T: 'static>(&self) -> Option<&'a T> { ... }
}
```

#### 2.3.2 Strongly Typed Calls & Events (Zero In-Process JSON)
In-process dispatch uses `TypeId` + `Box<dyn Any>` with zero JSON allocation or stringly-typed field mismatches. Optional inspection metadata respects `SENSITIVE`:

```rust
pub trait ModuleCall: 'static {
    const MODULE: ModuleId;
    const METHOD: &'static str;
    /// When true, Inspect/Trace recorders MUST redact arguments and outputs as `"<redacted>"`.
    const SENSITIVE: bool = false;
    type Output: 'static;

    /// Optional structured summary for Inspect / DST logs (never emitted when `SENSITIVE == true`).
    fn inspect_args(&self) -> Option<serde_json::Value> { None }

    /// Optional structured summary for a successful result.
    fn inspect_output(_value: &Self::Output) -> Option<serde_json::Value> { None }
}

pub trait ModuleEvent: Clone + 'static {
    const MODULE: ModuleId;
    const EVENT: &'static str;
    const SENSITIVE: bool = false;

    fn inspect_payload(&self) -> Option<serde_json::Value> { None }
}
```

#### 2.3.3 Thread-Safe Bridge (`ModuleDispatcher`) & Deterministic Cycle Drain
- `ModuleRegistry` is owned by `AppRuntime` on the single UI thread (zero `Rc` leaks, zero UI-thread lock contention).
- Async OS callbacks and worker threads receive a lightweight `Clone + Send + Sync` **`ModuleDispatcher`** backed by a bounded MPSC channel + the runtime's host `wake` callback:
  - `dispatcher.resolve::<C>(ticket, Result<C::Output, ModuleError>)`
  - `dispatcher.resolve_erased(ticket, Result<Box<dyn Any + Send>, ModuleError>)`
  - `dispatcher.emit::<E>(event)`
  - `dispatcher.emit_native_event(window, id, event)`
  - `dispatcher.resolve_native_snapshot(request, snapshot)`
- At the start of every `Cycle` (`Ingress`), `AppRuntime::drain_module_messages()`:
  1. Drops resolutions whose originating `ScopeId` has retired (and guarantees `OctaraModule::cancel(ticket)` was dispatched when the scope retired).
  2. Delivers all surviving call completions and module events into the current `Cycle`, coalescing resulting reactive signal writes into a single commit and layout pass.
- Synchronous replies can carry any `'static` output. Replies and events posted by workers require `Send` data. Workers receive `ModuleError::Busy` when the inbox is full and `ModuleError::Cancelled` after shutdown. A host running on the UI thread can drive a lifecycle event directly through ingress while draining the worker inbox.
- Lifecycle broadcasts visit installed modules in `ModuleId` order, then live subscribers in registration order. Typed observations in Inspect Replay are summaries. The original Rust payload remains available only within the process.

#### 2.3.4 `OctaraModule` Lifecycle & Registration Trait
```rust
pub trait OctaraModule: 'static {
    fn id(&self) -> ModuleId;

    /// Called once during `AppDefinition::launch` on the UI thread before windows mount.
    /// Modules may create root-scope `Signal`s, `provide` read-only handles, and register
    /// `NativeViewFactory` instances into `cx.native_views()`.
    fn install(&mut self, cx: &mut ModuleInstallCx<'_>) -> Result<(), ModuleError> {
        let _ = cx;
        Ok(())
    }

    /// Dispatch a typed call erased to `Box<dyn Any>`.
    /// Use `cx.downcast::<MyCall>(req)` / `cx.reply(...)` / `cx.dispatcher()` for sync or async replies.
    fn call(&mut self, ticket: CallTicket, req: Box<dyn Any>, cx: &mut ModuleCallCx<'_>);

    /// Best-effort cancellation when the caller's reactive `ScopeId` retires or window closes.
    fn cancel(&mut self, _ticket: CallTicket) {}

    /// Receive unified host lifecycle events (App launch URLs, foreground/background, reopen, etc.).
    fn on_lifecycle(&mut self, _event: &HostLifecycleEvent, _cx: &mut ModuleLifecycleCx<'_>) {}

    /// Clean release on `AppRuntime::shutdown`.
    fn shutdown(&mut self) {}
}
```

**Explicit registration only**:
- `AppDefinition::with_module(mut self, module: impl OctaraModule) -> Self`
- Duplicate `ModuleId` registration fails fast with `AppError::DuplicateModule(ModuleId)` (or replaces in test override helpers).
- **Forbidden**: global `ctor`, `inventory`, or `linkme` linker-section auto-registration. Dead-code elimination, target isolation, and test mock substitution remain 100% explicit and predictable.

#### 2.3.5 `NativeViewFactory` & Typed Native View Protocol
Replaces hardcoded `"webview"` / `"video"` switches in `octara-host-native` and enables native views across macOS, iOS, Windows, Linux, Web, and Headless DST:

```rust
pub struct NativeViewEmitter {
    window: WindowId,
    id: NativeId,
    dispatcher: ModuleDispatcher,
}

impl NativeViewEmitter {
    /// Emit a typed native view event delivered to the owning view node's typed listener.
    pub fn emit<E: Send + 'static>(&self, name: &'static str, payload: E, inspect: Option<serde_json::Value>)
        -> Result<(), ModuleError>;
    /// Complete an asynchronous native view snapshot request.
    pub fn finish_snapshot(&self, request: NativeSnapshotRequest, snapshot: NativeSnapshot) -> Result<(), ModuleError>;
}

pub trait NativeViewFactory: 'static {
    fn kind(&self) -> &'static str; // e.g. "octara.webview", "octara.video"

    fn airspace_policy(&self) -> AirspacePolicy {
        AirspacePolicy::UnderlayPreferred
    }

    fn supports_slot(&self, _surface: HostSurfaceKind, _slot: NativeSlot) -> bool {
        true
    }

    fn create(
        &mut self,
        surface: HostSurface<'_>,
        id: NativeId,
        slot: NativeSlot,
        props: Box<dyn Any>,
        emitter: NativeViewEmitter,
    ) -> Result<Box<dyn NativeViewInstance>, ModuleError>;
}

pub trait NativeViewInstance: 'static {
    fn update(&mut self, props: Box<dyn Any>);
    fn place(&mut self, placement: &NativePlacement);
    fn call(&mut self, ticket: CallTicket, command: Box<dyn Any>, reply: ModuleDispatcher);
    fn measure(&mut self, _viewport: WindowViewport) -> Option<Size> { None }
    fn snapshot(&mut self, request: NativeSnapshotRequest, reply: ModuleDispatcher);
    fn busy_reason(&self) -> Option<String> { None }
    fn set_occluded_by_snapshot(&mut self, _hidden: bool) {}
}
```

**Typed Props & Inspect Dual-Track**:
- `NativeChange` / `NativeCommand` in the runtime hot path carry typed payloads (`Box<dyn Any>` / `Arc<dyn Any + Send + Sync>` for cloned immutable props) paired with optional `inspect: Option<serde_json::Value>` snapshots produced by the module's view builder so `octara-inspect`, `Replay`, and `HeadlessDriver` remain fully observable without forcing JSON serialization on every frame or RPC call.
- **Airspace Occlusion Contract**:
  1. `AirspacePolicy::UnderlayPreferred`: Host places the native view beneath the transparent Octara GPU surface and forwards `NativePlacement.covered` rectangles so hits on Octara popovers/dialogs/tooltips stay inside Octara.
  2. `AirspacePolicy::SnapshotOnOcclude`: When any live Octara overlay (`LayerPolicy`) intersects an overlay native view's `rect`, the host triggers `snapshot(...)`, composites the captured `Picture` into the Octara paint slot, and calls `set_occluded_by_snapshot(true)` until the overlay closes.

---

### 2.4 Authoring API (`octara::module`) & DST Ergonomics

#### Application Bootstrap
```rust
let app = AppDefinition::new()
    .with_module(octara_webview::module())
    .with_module(octara_video::module())
    .with_module(octara_picker::module())
    .with_module(octara_haptics::module())
    .window(WindowDefinition::new(options, root));
```

#### Calling & Subscribing in Components
- **In `WriteCx` (event handlers)**:
  - `w.call_module(req, |w, res: Result<C::Output, ModuleError>| { ... }) -> CallTicket`
  - `w.notify_module(req)` (fire-and-forget for `Output = ()`)
- **In `TaskCx` (`cx.spawn` async tasks)**:
  - `let result: Result<C::Output, ModuleError> = task.call_module(req).await;`
  - Dropping the future or retiring the owning `ScopeId` cancels the pending ticket and invokes `OctaraModule::cancel(ticket)`.
- **In `BuildCx` (component setup)**:
  - `cx.on_module_event::<DeepLinkOpened>(|w, ev| { ... })` (auto-unsubscribed when `ScopeId` retires).
  - `cx.on_lifecycle(|w, ev: &HostLifecycleEvent| { ... })` (convenience wrapper over the core lifecycle stream).

#### Deterministic Simulation Testing (`HeadlessDriver`)
- Every official module crate MUST provide a pure-Rust `mock()` constructor (zero OS/GPU dependencies, compiles on all targets including headless CI).
- `HeadlessDriver` exposes direct module control:
  - `driver.stub_module_call::<C>(handler)` — intercept any typed `ModuleCall` without registering a full mock module.
  - `driver.emit_module_event(event)` — inject platform events (Deep Link, Push Notification, App Background, Memory Warning) deterministically on the virtual clock.
  - `driver.dispatch_lifecycle(HostLifecycleEvent::...)` — test OS lifecycle transitions deterministically.

---

### 2.5 Declarative Manifest (`octara-module.toml`) & `octara-cli` Integration

Modules requiring OS bundle metadata include a purely declarative `octara-module.toml` at the crate root (zero build-time script execution):

```toml
[module]
id = "octara.picker"
schema = 1

[ios.info_plist]
NSPhotoLibraryUsageDescription = "${photo_library_usage_description}"

[macos.entitlements]
"com.apple.security.files.user-selected.read-write" = true

[apple.frameworks]
ios = ["PhotosUI", "UniformTypeIdentifiers"]
macos = ["AppKit"]

[web.headers]
permissions_policy = []
```

- Applications supply required template variables in `octara.toml` under `[modules."octara.picker"]`.
- `octara-cli` resolves the workspace dependency graph via `cargo metadata`, collects every `octara-module.toml`, validates all `${...}` placeholders (failing with a clear diagnostic if missing), and merges `Info.plist`, `.entitlements`, and Web manifests deterministically.

---

## 3. Core vs. Official Module Boundary

### Stays in Core (`octara-model` / `octara-runtime` / `octara-host-*`)
Capabilities required by virtually every UI tree or tightly coupled to the window/input/focus loop:
1. Window presentation & geometry (`WindowPresentation`, `WindowGeometry`, `WindowViewport` incl. `keyboard_inset`, `SystemPreferences`, `AppStatus`).
2. Core input & editing (pointer, keyboard, wheel, touch/pinch/rotate, IME, drag & drop, `PasteEvent`, `ClipboardItem` read/write, `OpenUrl`).
3. Native context menus & modal alerts (`Ask::Menu`, `Ask::Prompt`).
4. Unified `HostLifecycleEvent` bus and `NativeViewRegistry` plumbing.

### Moves into Official First-Party Modules (`modules/*`)

| Module Crate | Replaces / Consolidates | Four-Platform Backends |
| --- | --- | --- |
| **`modules/octara-webview`** | Removes `octara-host-native/src/os/webkit.rs` & hardcoded `"webview"` slots | macOS/iOS `WKWebView` (+ `WKUIDelegate`, navigation policy, `WKURLSchemeHandler`, typed Promise RPC), Web `<iframe>`, Windows WebView2 / Linux WebKitGTK slots, Headless `mock()` |
| **`modules/octara-video`** | Removes `octara-host-native/src/os/video.rs` & hardcoded `"video"` slots | macOS/iOS `AVKit`, Web `<video>`, Headless `mock()` |
| **`modules/octara-picker`** | Replaces `Ask::Paths`, `Ask::NewPath`, and `ServiceRequest::PickFile` | macOS `NSOpenPanel`/`NSSavePanel`, iOS `PHPickerViewController` + `UIDocumentPickerViewController`, Desktop `rfd`, Web `<input>`/`showDirectoryPicker`, Headless `mock()` |
| **`modules/octara-haptics`** | New | iOS `UIImpactFeedbackGenerator`/`UINotificationFeedbackGenerator`, macOS `NSHapticFeedbackManager`, Web Vibration API, Headless `mock()` |
| **`modules/octara-notifications`** (Phase 3) | New | macOS/iOS `UNUserNotificationCenter`, Web Notification API, Headless `mock()` |
| **`modules/octara-share`** (Phase 3) | New | iOS `UIActivityViewController`, macOS `NSSharingServicePicker`, Web Share API, Headless `mock()` |
| **`modules/octara-secure-store`** (Phase 3) | New | Apple Keychain, OS credential vault, Web Crypto + IndexedDB, Headless `mock()` |
| **`modules/octara-desktop-shell`** (Phase 3) | New | macOS/Desktop tray icon (`NSStatusItem`), global hotkeys, floating `NSPanel` / always-on-top |

---

## 4. Phased Implementation Plan & Verification Gates

### Wave 1 — Cycle Naming Cleanup + `octara::module` Spine Foundation
1. **Cycle Rename**: Apply the terminology changes in §1 across the entire workspace (zero aliases).
2. **Layer 0 & Layer 1 Spine**: Implement `octara-model::module` and `octara-runtime::module` (`ModuleRegistry`, `ModuleDispatcher`, `HostSurface`, `ModuleCall`, `ModuleEvent`, `OctaraModule`, `NativeViewFactory`, `NativeViewInstance`, `NativeViewRegistry`, `HostLifecycleEvent`, `AirspacePolicy`, `ScopeId` auto-cancellation, `Ingress` drain).
3. **Facade & Authoring**: Export `octara::module`; wire `AppDefinition::with_module`, `WriteCx::call_module` / `notify_module`, `TaskCx::call_module`, `BuildCx::on_module_event` / `on_lifecycle`.
4. **Host & DST Plumbing**: Wire `HostSurface`, `NativeViewRegistry`, `ModuleDispatcher` wake integration, and `HostLifecycleEvent` forwarding into `octara-host-native`, `octara-host-web`, and `HeadlessDriver` (`stub_module_call`, `emit_module_event`, `dispatch_lifecycle`).
5. **Layer Gate**: Extend `tools/check_layers.py` to enforce the `modules/*` optional plugin layer rules.
6. **Wave 1 Gate**: `python3 tools/check_layers.py`, `cargo clippy --workspace --all-targets -- -D warnings`, dual Wasm checks, `aarch64-apple-ios-sim` check, full workspace + CLI doc tests green, plus unit/DST tests proving Scope cancellation, `SENSITIVE` redaction, typed `Any` dispatch, lifecycle fan-out, and `NativeViewRegistry` custom slot mounting.

### Wave 2 — Official Module Extractions (`webview`, `video`, `picker`, `haptics`) & `octara-module.toml`
1. Extract **`modules/octara-webview`** from `octara-host-native/src/os/webkit.rs`; add `WKUIDelegate` (new window / alerts / file input), navigation policy hook, custom scheme handler, typed Promise JS↔Rust RPC, Web `<iframe>` backend, and Headless `mock()`.
2. Extract **`modules/octara-video`** from `octara-host-native/src/os/video.rs`; remove `objc2-web-kit` and `AVKit` links from `octara-host-native`!
3. Build **`modules/octara-picker`** covering macOS, iOS (`PHPicker` + `UIDocumentPicker`), Desktop (`rfd`), Web, and Headless `mock()`; migrate `apps/chat` and `examples/` off `Ask::Paths` / `Ask::NewPath` / `ServiceRequest::PickFile`, then delete those legacy variants from core `Ask` / `ServiceRequest`.
4. Build **`modules/octara-haptics`** (macOS / iOS / Web / Headless `mock()`).
5. Implement `octara-module.toml` discovery, placeholder validation, and plist/entitlements merging in `octara-cli`.
6. **Wave 2 Gate**: Full workspace + `modules/*` Clippy, tests, dual Wasm, `aarch64-apple-ios-sim`, and verify `octara-host-native` has zero `objc2-web-kit` / `AVKit` dependency edges.

### Wave 3 — Airspace Snapshot Fallback, Four-Platform Polish & Ecosystem Modules
1. Implement `AirspacePolicy::SnapshotOnOcclude` automatic GPU snapshot swap when overlays intersect overlay native views.
2. Complete four-platform core gaps: iOS touch gestures/accessibility/status-bar style, Web `window.visualViewport` keyboard inset / `beforeunload` / ARIA focus trap, macOS URL scheme / file drop / trackpad pinch-rotate.
3. Add `modules/octara-notifications`, `modules/octara-share`, `modules/octara-secure-store`, and `modules/octara-desktop-shell`.

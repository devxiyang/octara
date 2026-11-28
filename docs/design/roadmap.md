# Octara Master Roadmap (`0.0.8` → `0.1.0` → `0.2.0`)

> **Mission**: Build a complete, zero-fork, pure-Rust cross-platform UI and application framework combining fine-grained compile-time-safe Signals, a sparse ECS micro-kernel, native GPU/DOM fidelity, deterministic simulation testing (DST), and an extensible `octara::module` ecosystem.
>
> **Core Principle**: Keep core crates (`octara-model` → `octara-host-*`) minimal, fast to compile, and strictly layered. Platform-specific capabilities and OS integrations live in optional, explicitly registered `modules/*` crates with mandatory pure-Rust Headless `mock()` backends.

---

## 1. Target Platform Priority & Minimum OS Baseline

| Priority Tier | Platforms | Minimum OS / Runtime Baseline | Strategy |
| --- | --- | --- | --- |
| **Tier 1 (Primary)** | **macOS**, **iOS**, **Web (Wasm/DOM + opt-in WebGPU)**, **Linux (Wayland/X11)** + **WebView** (`WKWebView`, `WebKitGTK`, Web `<iframe>`) | **macOS 15.4+**, **iOS 18.4+**, evergreen browsers (ES2022 + Wasm), modern Linux (Wayland/X11 + XDG Portal) | First-class feature parity, native ergonomics, accessibility, packaging, and CI gates through `0.1.0`. Zero pre-15.4 / pre-18.4 Apple legacy fallbacks. |
| **Tier 2 (Deferred)** | **Windows** (`DX12`, `WebView2`, Win32/Mica), **Android** (`android-activity`, JNI, TalkBack) | Windows 11 / modern Android | Architectural seams (`RawNativeSurface::Win32`, `HostSurfaceKind::Windows`, reserved `Android` variant) kept clean with zero core lock-in; full production backends land in `0.2.0`. |

> **Apple SDK Floor (`macOS 15.4+` / `iOS 18.4+`)**: `octara-cli` defaults `LSMinimumSystemVersion = 15.4` and `MinimumOSVersion = 18.4`. Unconditionally rely on modern Apple APIs (`CADisplayLink` on macOS/iOS, `WKWebsiteDataStore::dataStoreForIdentifier:`, iOS `WKUIDelegate` open-panel callback, `SMAppService`, `PHPickerViewController`, `UniformTypeIdentifiers`, `UIWindowScene`, `NWPathMonitor`) without legacy `@available` shims.

---

## 2. Eight Production Framework Pillars

| # | Pillar | Core vs. Module Boundary | Target Milestone |
| --- | --- | --- | --- |
| **1** | **Four-Platform Core Interaction & Forms** (unified gestures, Pull-to-Refresh, soft-keyboard & OTP traits, scroll-dismiss keyboard, Drag Out, `WindowLevel`/`NSPanel`, Linux Wayland IME/HiDPI/Portal) | Core (`octara-model` / `tree` / `runtime` / `host-*`) | `0.0.9` |
| **2** | **Full Accessibility & Internationalization** (macOS/Linux AccessKit, Web ARIA Focus Trap/`inert`, **iOS `UIAccessibility` VoiceOver**, BiDi/RTL layout, Dynamic Type `text_scale`, `locale`, `high_contrast`) | Core (`octara-model` / `tree` / `pipeline` / `host-*`) | `0.0.9` |
| **3** | **Airspace & WebView Completeness** (`AirspacePolicy::SnapshotOnOcclude`, `WKWebsiteDataStore`, `WKUserScript`, download policy, Linux `WebKitGTK`) | Airspace in Core (`0.0.9`); WebView in `modules/octara-webview` (`0.0.10`–`0.0.11`) | `0.0.9` – `0.0.11` |
| **4** | **High-Frequency Platform Modules** (`notifications`, `secure-store`, `desktop-shell`, `storage`, `share`, `network`) | Optional `modules/*` (SPI-7) | `0.0.10` |
| **5** | **Production Complex Components** (`DataTable`, `TreeView`, `SplitPane`, gesture `BottomSheet`/`Drawer`, `Toast` stack, `DatePicker`/`ColorPicker`, `CommandPalette`, `Form Validation`) | `octara-headless` + `octara-kit` | `0.0.11` |
| **6** | **Modern Graphics, Motion & Export** (GPU `backdrop-filter` blur, spring physics & FLIP layout transitions, 120Hz frame pacing telemetry, vector PDF/print export from `DisplayList`) | `octara-pipeline` + `octara-render` + `octara-gpu` | `0.0.11` |
| **7** | **Advanced Hardware Modules** (`biometrics`, `audio`, `camera`, `location`, declarative deep-link routing helpers) | Optional `modules/*` (SPI-7) | `0.0.11` |
| **8** | **Engineering DX, Packaging & DevTools** (`octara dev` state-preserving fast restart via `RestartState`, `octara bundle` for macOS/iOS/Web/Linux, visual DevTools HUD over `octara-inspect`, API freeze) | `octara-cli` + `octara-inspect` | `0.1.0` |

---

## 3. Milestone Plan

### `0.0.8` — Cycle Naming Cleanup, `octara::module` Spine & Initial Official Modules *(In Progress)*
- **Spec**: [14 — Cycle Terminology Cleanup & `octara::module` Platform Extension Architecture](v2/14-octara-module-architecture.md)
- **Delivered / In Flight**:
  - **Wave 1 *(Complete)*:** `turn.rs` → `cycle.rs`, `TurnError` → `AppError`, `TurnReceipt` → `CycleReceipt`; full `octara::module` SPI-7 spine (`octara-model::module` + `octara-runtime::module` → `octara::module`), `HostSurface`, `ModuleDispatcher`, `ScopeId` cancellation, `HostLifecycleEvent` bus, `NativeViewFactory`/`NativeViewRegistry`, `HeadlessDriver` DST stubs, `tools/check_layers.py` plugin layer.
  - **Wave 2 *(Complete)*:** Extract `modules/octara-webview` (upgraded Apple `WKWebView` + Web `<iframe>` + Headless `mock()`), `modules/octara-video` (removing `objc2-web-kit`/`AVKit` from `octara-host-native`), `modules/octara-picker` (macOS/iOS `PHPicker`+`UIDocumentPicker`/Linux/Web/Headless, deleting legacy `Ask::Paths`/`ServiceRequest::PickFile`), `modules/octara-haptics`, and `octara-cli` declarative `octara-module.toml` merger.

---

### `0.0.9` — Core Gestures, Soft-Keyboard Traits, RTL & P0 120Hz CPU+GPU Rendering Overhaul *(Released — `f0e572c`)*
- **Spec**: [15 — Tier-1 Core Interaction, Accessibility, Forms, RTL & Airspace Closure](v2/15-four-platform-core-polish.md) (Wave A + Wave B1 / C7)
- **Delivered**:
  - Unified gestures (`GestureEvent::{Pinch, Rotate, LongPress, DoubleTap}`, `PullToRefresh`, `ScrollDismissesKeyboard`), `InputTraits` (`KeyboardType`, `ReturnKeyType`, `TextContentType`), BiDi/RTL `Direction` & `SystemPreferences::{locale, text_scale, high_contrast}`.
  - End-to-end CPU+GPU Scroll-Space Separation (`SpatialTable`, retained GPU instance buffers, 5K `DamageRect` partial rasterization, dormant `AccessKitProjector`, O(1) bulk retirement), unified Apple `CADisplayLink`, monotonic presentation, and causal VSync input coalescing.

---

### `0.0.10` — Zero-`winit` Direct Native Hosts, Zero-`TaffyTree` ECS Layout Kernel, Unified Native IME & Zero-Bloat Core *(In Progress)*
- **Specs**: [15 — Tier-1 Core Closure](v2/15-four-platform-core-polish.md) (C3–C6), [17 — Direct Native Platform Hosts (Zero `winit` Architecture)](v2/17-native-platform-hosts-without-winit.md), [18 — Native ECS Layout & Spatial Kernel (Zero-`TaffyTree` Architecture)](v2/18-native-layout-and-spatial-kernel.md), [19 — Unified Native IME, Text Editing & Clipboard Hygiene](v2/19-unified-native-ime-and-text-editing.md) & [20 — Zero-Bloat Core Dependencies & Native OS Primitives](v2/20-zero-bloat-core-dependencies.md)
- **Scope**:
  1. **iOS & Web Production Closure (C3 + C4, Wave B2 — Done)**: iOS `UIAccessibility` VoiceOver bridge + `StatusBarStyle` + scene-local keyboard layout; Web `visualViewport`, `document.title`/`theme-color`, `beforeunload`, unified multimodal `WriteClipboard`, and modal ARIA focus trap + `inert`.
  2. **100% `winit` & `accesskit_winit` Removal (Waves D1 Done, D2 Running)**: Pure platform-neutral `OsHost` / `OsWindow` / `PlatformEvent` SPI in `octara-host-native`; direct macOS `15.4+` AppKit host (`NSApplication` / `NSWindow` / `NSPanel` / `NSTextInputClient` / `accesskit_macos`), direct iOS `18.4+` UIKit host (`UIApplicationMain` / `UIWindowSceneDelegate` / `UIViewController`), and direct Linux Wayland (`smithay-client-toolkit`, `zwp_text_input_v3`, `zwp_pointer_gestures_v1`, `wp_fractional_scale_v1`, `wl_data_device`, XDG Portal) + X11 (`x11rb`) host.
  3. **Desktop Drag-Out, Window Levels/Panels & Airspace Closure (C5 + C6, Wave D2)**: `DragItem` / `el.on_drag_start` (`NSDraggingSession`, Web `DataTransfer`, Linux `wl_data_device`), `WindowLevel` + native macOS `NSPanel`, and `AirspacePolicy::SnapshotOnOcclude`.
  4. **Zero-`TaffyTree` Native ECS Layout & Spatial Kernel (Wave D3)**:
     - **Delete `TaffyTree` completely** (`taffy_tree` feature removed); build Octara's hot/cold SoA `LayoutKernel` (`8 B` `LayoutMeta` + dense `SecondaryMap` + shared `DenseProjected` rollback journal across `layout`/`text`/`paint`/`a11y`) implementing Taffy's zero-copy `LayoutPartialTree` / `LayoutContainingBlock` / `LayoutFlexboxContainer` / `LayoutGridContainer` traits directly over `octara-tree` and interned `Arc<ResolvedStyle>`, with O(1) leaf/hidden fast paths and zero `FxHashMap` fragmentation.
     - **Mathematically Sound Dual-Layer `RelayoutBoundary` + Cross-Platform `Contain`**: Zero-config automatic tight-box+clipped boundaries + first-class W3C `Contain::LAYOUT | SIZE | PAINT` (projected to native containment and Web CSS `contain`), plus isolated anchored overlay layout roots and automatic escalation.
     - **Rigid-Body `local_changed()` vs `world_changed()` Separation + Epoch-Pruned Projection + Lazy/Incremental `FlatTree` + Content-Addressed `Shape` Probe Cache + `check_layout_cache` Oracle**: Eliminate full-window `collect_frames`, eager spatial rebuilds, and streaming-growth sibling cascades (`1,832 → <= 16` geometry visits on 160-row hover; 0 sibling text/a11y re-projections on rigid-body row shifts; 0.00 ms spatial work on non-pointer frames), verified bit-for-bit against cold full-tree layout in DST.
  5. **Unified Native IME, Text Editing & Clipboard Hygiene (Wave D4)**:
     - Collapse `winit`-era duplicate `EditorEvent` variants into minimal orthogonal `Edit` / `Select` / `Command(EditorCommand)` / `KeyIntercept` (with `TextEdit::DeleteSurrounding` and CJK `MarkedClause` underlines) and execute every `EditorCommand` in one shared `octara-tree`/`octara-runtime` path.
     - Extract one platform-neutral `HostTextBridge` in `octara-host-native` (shared `EditorMirror`, optimistic prediction reconciliation, UTF-8↔UTF-16 mapping, 4000 B Wayland `surrounding_text`, `TextGeometry` queries), deleting duplicated per-platform shadow state machines (`os/ios/draft.rs` & AppKit command duplication) and enforcing the deterministic 4-tier key routing law.
     - Delete dead `arboard` dependency workspace-wide (every Tier-1 platform uses native OS clipboard directly).
  6. **Zero-Bloat Core Dependencies & Native OS Primitives (Wave D5)**:
     - Delete `tokio` & `reqwest` (+ `rustls`/`hyper`, ~100+ transitive crates) from `octara-host-native`, replacing `media.rs` with a pluggable `MediaTransport` SPI, direct Apple `NSURLSession`, and Octara's own QoS worker pool.
     - Remove `objc2-map-kit` & `objc2-core-location` from core `octara-host-native`, replace `image` on Apple with hardware `ImageIO` (`CGImageSource` / `CGImageDestination`), purge Linux redundant wrappers (`clipboard-rs`, `wl-clipboard-rs`, `x11-dl`, `open`), replace `icu_locale` in Layer-1 `octara-tree` with a 35-line pure-Rust BCP-47 RTL table (0 CLDR blob in `.wasm`), and self-own `block_on` (deleting `pollster`).

---

### `0.0.11` — High-Frequency System Modules & WebView Persistence
- **Spec**: [16 — High-Frequency System Modules & WebView Depth](v2/16-high-frequency-modules.md)
- **Scope (Tier-1 Platforms: macOS, iOS, Web, Linux + Headless `mock()`)**:
  1. **`modules/octara-notifications`**: Local notifications, permission state machine, remote push token registration, tap/action deep-link wake (`UNUserNotificationCenter`, Linux `org.freedesktop.Notifications` D-Bus, Web Notification API).
  2. **`modules/octara-secure-store`**: Encrypted credential/secret store with mandatory `SENSITIVE = true` redaction (Apple Keychain, Linux Secret Service D-Bus, Web Crypto AES-GCM + IndexedDB).
  3. **`modules/octara-desktop-shell`**: System tray (`NSStatusItem` / Linux StatusNotifierItem D-Bus) with reactive menus & click events, global hotkeys (macOS / Linux XDG GlobalShortcuts portal), and launch-at-login (`SMAppService` / Linux autostart `.desktop`).
  4. **`modules/octara-storage`**: Standard app directories (`AppData` / `Cache` / `Temp` / `Documents`) + atomic KV store (Native filesystem + Web IndexedDB/OPFS) + one-line `RestartState` persistence integration.
  5. **`modules/octara-share`**: Native share sheets (`UIActivityViewController`, `NSSharingServicePicker`, Linux portal/fallback, Web Share API).
  6. **`modules/octara-network`**: Reactive `ReadSignal<NetworkStatus>` + change events (`NWPathMonitor`, Linux NetworkManager/netlink, Web `navigator.onLine`).
  7. **`modules/octara-webview` Depth**: `WebViewProfile::{Persistent, Ephemeral}` (`WKWebsiteDataStore`), cookie/cache management, `WKUserScript` injection (`DocumentStart` / `DocumentEnd`), CSP policy helpers, and download interception (`WKDownloadDelegate`).

---

### `0.0.12` — Direct Apple Metal Renderer, 2.5D/3D Compositor, Complex Components, `WebKitGTK` & Hardware Modules
- **Specs**: [21 — Direct Apple Metal Renderer (`octara-render-metal`) & 2.5D / Zero-Copy 3D Compositor](v2/21-direct-metal-renderer-and-25d-compositor.md) & planned `v2/22-components-and-hardware-modules.md`
- **Scope**:
  1. **Direct Apple Metal Backend & 2.5D / Zero-Copy 3D Compositor**:
     - First-party `octara-render-metal` (`objc2-metal` + MSL implementing `SPI-1` `SurfaceRenderer`, **0 `wgpu` / 0 `naga`** on macOS & iOS, memoryless TBDR depth, `-4–8 MB` iOS binary).
     - `Mat4` 2.5D perspective layer transforms (1:1 CSS `matrix3d` parity), GPU real-time `backdrop-filter` / Liquid Glass blur, and zero-copy `ExternalTexture` bridge (`MTLTexture`/`IOSurface`/`CVPixelBuffer` on Apple, `wgpu::Texture` on Linux/Windows/WebGPU, `<canvas>` on Web).
  2. **Complex Components (`octara-headless` + `octara-kit`)**:
     - Virtualized `DataTable` (sticky headers/columns, resizable columns, sorting, multi-select), `TreeView`, `SplitPane` (draggable & collapsible).
     - Gesture-driven `BottomSheet` / `Drawer` (velocity snap & rubber-band dismiss), `Toast` queue stack, `DatePicker` / `TimePicker` / `ColorPicker`, `CommandPalette`, and declarative `Form` validation with `aria-invalid` / error summary bindings.
  3. **Motion & Export**:
     - Spring physics curves & FLIP layout transitions in `MotionEngine`; 120Hz frame pacing telemetry.
     - Vector PDF / print export directly from `DisplayList`.
  4. **Linux WebView Backend**:
     - Add **`WebKitGTK`** backend inside `modules/octara-webview` (matching the typed navigation, scheme handler, script injection, and Promise RPC surface of Apple `WKWebView`).
  5. **Advanced Hardware Modules (`modules/*`)**:
     - `modules/octara-biometrics` (Touch ID / Face ID / Linux Polkit-fprint / WebAuthn).
     - `modules/octara-audio` (recording, playback, level metering stream, audio session management).
     - `modules/octara-camera` (`NativeViewFactory` live preview + photo capture + QR/barcode scanning).
     - `modules/octara-location` (permissioned one-shot and streaming coordinates, `SENSITIVE = true`).

---

### `0.1.0` — Production Beta: Packaging, Fast Restart, DevTools & Core API Freeze
- **Planned Spec**: `v2/23-packaging-devtools-and-v01-freeze.md`
- **Scope**:
  1. **`octara dev` State-Preserving Fast Restart**: File watcher + automatic `RestartState` snapshot → incremental rebuild → instant relaunch restoring window geometry and reactive state.
  2. **`octara bundle` One-Command Packaging**:
     - Automatically merges every `octara-module.toml` in the dependency graph.
     - **macOS**: `.app` / `.dmg` with `Info.plist`, `.entitlements`, codesign & notarization hooks.
     - **iOS**: `.app` / `.ipa` and `octara run ios` Simulator launch.
     - **Linux**: `.desktop`, AppImage / `.deb` bundle.
     - **Web**: `wasm-opt`, font/asset lazy splitting, and PWA manifest generation.
  3. **Visual DevTools (`octara-inspect`)**: Live node tree explorer, box-model/safe-area/clip overlay inspector, reactive signal graph inspector, frame timing & draw-call HUD.
  4. **Full Tier-1 CI & Core API Freeze**: Automated regression matrix across macOS (Metal + CPU parity), iOS Simulator, Headless Chrome CDP, Linux, and DST fuzzing; freeze public core contracts (`El`, `Store`, `Readable`/`Writable`, `BuildCx`/`ReadCx`/`WriteCx`/`TaskCx`, `AppError`, `CycleReceipt`, `OctaraModule`, `NativeViewFactory`) for `0.1.x` stability.

---

### `0.2.0` — Tier-2 Platform Expansion: Windows & Android *(Deferred)*
- **Windows**: DX12 GPU verification, native titlebar Mica/Acrylic material, IME candidate positioning, multimodal clipboard, `WebView2` backend in `modules/octara-webview`, Windows Credential Manager / Toast Notifications / Tray / Global Hotkeys in `modules/*`, `.msi`/`.exe` packaging in `octara-cli`.
- **Android**: `HostSurfaceKind::Android` & `RawNativeSurface::Android`, `android-activity` / `SurfaceView` / JNI host bridge, TalkBack accessibility, Android `WebView` / Media / Picker / Biometrics / Haptics / Notifications module backends, `.apk`/`.aab` packaging in `octara-cli`.

---

## 4. Release Discipline & Quality Gates

Every milestone from `0.0.8` through `0.1.0` must satisfy all of the following before tagging and publishing:
1. `python3 tools/check_layers.py` passes with zero core → module edges and zero authoring facade bypasses.
2. `cargo clippy --workspace --all-targets -- -D warnings` passes cleanly.
3. Cross-target checks pass for default `wasm32-unknown-unknown`, `wasm32-unknown-unknown --features webgpu`, `aarch64-apple-ios-sim`, and Linux target builds where applicable.
4. Full `cargo test --workspace` + `cargo test -p octara-cli --doc` + DST performance ceilings pass with zero failures.
5. Every official `modules/*` crate ships with a pure-Rust Headless `mock()`, declarative `octara-module.toml` (when OS metadata is required), and DST integration tests.
6. Architecture iron laws remain intact: zero `Rc` in `octara-runtime`, zero `std::thread` in `octara-render`, zero `Rc::new`/`RefCell`/`todo!()` in `octara-headless`/`octara-kit`/`octara-markdown`, zero `#[cfg(target_os)]` in native core orchestration.

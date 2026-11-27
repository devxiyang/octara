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

### `0.0.9` — Tier-1 Core Interaction, Accessibility, Forms, RTL & Airspace Closure *(Next — Core First)*
- **Spec**: [15 — Tier-1 Core Interaction, Accessibility, Forms, RTL & Airspace Closure (`0.0.9`)](v2/15-four-platform-core-polish.md)
- **Why Core First**: Finalizing gestures, input traits, iOS VoiceOver, Web `visualViewport`/`inert`, Drag Out, `WindowLevel`/`NSPanel`, Airspace `SnapshotOnOcclude`, and RTL stabilizes the core public API before scaling out the `modules/*` ecosystem.
- **Scope**:
  1. **Unified Gesture Engine**: `GestureEvent::{Pinch, Rotate, LongPress, DoubleTap}` across macOS trackpad (`magnifyWithEvent:` / `rotateWithEvent:`), iOS multi-touch, Linux touchpad, and Web Pointer/Wheel; `PullToRefresh` primitive and mobile overscroll physics.
  2. **Forms & Soft-Keyboard Semantics**: `KeyboardType`, `ReturnKeyType`, `AutoCapitalize`, `Autocorrect`, `TextContentType` (password/OTP autofill), and `ScrollDismissesKeyboard::{Never, Immediately, Interactively}` mapped to iOS `UITextInputTraits` and Web `inputmode`/`enterkeyhint`/`autocomplete`.
  3. **iOS Native Closure**: **`UIAccessibility` / VoiceOver** semantic tree bridge; `StatusBarStyle::{Auto, Light, Dark, Hidden}` synced with `WindowPresentation.theme`; portrait/landscape + Dynamic Island safe-area verification.
  4. **Web Production Closure**: `window.visualViewport` → `WindowViewport.keyboard_inset`; `document.title` & `<meta name="theme-color">` sync; `beforeunload` wired to `Window::on_close_request`; multimodal clipboard write (text/image); complete modal ARIA focus trap + `inert`.
  5. **macOS & Linux Desktop Closure**:
     - **Drag Out**: macOS `NSDraggingSession`, Linux Wayland/X11 drag source, and Web `draggable`/`dataTransfer`.
     - **Window Levels & Panels**: `WindowLevel::{Normal, Floating, AlwaysOnTop}` and non-activating floating panel (`NSPanel` semantics).
     - **Linux Native Polish**: Wayland `text-input-v3` IME, fractional HiDPI scaling, XDG Desktop Portal dark-mode/preferences integration.
  6. **Airspace Occlusion**: Implement `AirspacePolicy::SnapshotOnOcclude` automatic GPU snapshot freeze and live-view hide whenever an Octara overlay intersects an overlay native view.
  7. **Internationalization & Accessibility Preferences**: BiDi/RTL `Direction::{Ltr, Rtl}` with logical spacing/alignment (`start`/`end`), plus `SystemPreferences::{locale, text_scale, high_contrast}`.
  8. **P0 120Hz Presentation Pacing, VSync Input Coalescing & Scroll Hot-Path Optimization**: Monotonic GPU frame presentation (eliminating `finish_draw` stale-rev discard & `Retry(+16ms)` cascade), `CAMetalDisplayLink` `preferredFrameLatency = 1` + ProMotion `CAFrameRateRange` + idle grace window, causal VSync `MouseWheel`/`CursorMoved` coalescing with real `WheelPhase`, and Release CPU/GPU profiling & optimization of `SetScroll`, `virtual_each` + `octara-markdown` mounts, and bulk list switches.

---

### `0.0.10` — High-Frequency System Modules & WebView Persistence
- **Spec**: [16 — High-Frequency System Modules & WebView Depth (`0.0.10`)](v2/16-high-frequency-modules.md)
- **Scope (Tier-1 Platforms: macOS, iOS, Web, Linux + Headless `mock()`)**:
  1. **`modules/octara-notifications`**: Local notifications, permission state machine, remote push token registration, tap/action deep-link wake (`UNUserNotificationCenter`, Linux `org.freedesktop.Notifications` D-Bus, Web Notification API).
  2. **`modules/octara-secure-store`**: Encrypted credential/secret store with mandatory `SENSITIVE = true` redaction (Apple Keychain, Linux Secret Service D-Bus, Web Crypto AES-GCM + IndexedDB).
  3. **`modules/octara-desktop-shell`**: System tray (`NSStatusItem` / Linux StatusNotifierItem D-Bus) with reactive menus & click events, global hotkeys (macOS / Linux XDG GlobalShortcuts portal), and launch-at-login (`SMAppService` / Linux autostart `.desktop`).
  4. **`modules/octara-storage`**: Standard app directories (`AppData` / `Cache` / `Temp` / `Documents`) + atomic KV store (Native filesystem + Web IndexedDB/OPFS) + one-line `RestartState` persistence integration.
  5. **`modules/octara-share`**: Native share sheets (`UIActivityViewController`, `NSSharingServicePicker`, Linux portal/fallback, Web Share API).
  6. **`modules/octara-network`**: Reactive `ReadSignal<NetworkStatus>` + change events (`NWPathMonitor`, Linux NetworkManager/netlink, Web `navigator.onLine`).
  7. **`modules/octara-webview` Depth**: `WebViewProfile::{Persistent, Ephemeral}` (`WKWebsiteDataStore`), cookie/cache management, `WKUserScript` injection (`DocumentStart` / `DocumentEnd`), CSP policy helpers, and download interception (`WKDownloadDelegate`).

---

### `0.0.11` — Complex Components, Modern Graphics, `WebKitGTK` & Advanced Hardware Modules
- **Planned Spec**: `v2/17-components-graphics-and-hardware-modules.md`
- **Scope**:
  1. **Complex Components (`octara-headless` + `octara-kit`)**:
     - Virtualized `DataTable` (sticky headers/columns, resizable columns, sorting, multi-select), `TreeView`, `SplitPane` (draggable & collapsible).
     - Gesture-driven `BottomSheet` / `Drawer` (velocity snap & rubber-band dismiss), `Toast` queue stack, `DatePicker` / `TimePicker` / `ColorPicker`, `CommandPalette`, and declarative `Form` validation with `aria-invalid` / error summary bindings.
  2. **Graphics, Motion & Export**:
     - GPU real-time `backdrop-filter` frosted blur container.
     - Spring physics curves & FLIP layout transitions in `MotionEngine`; 120Hz frame pacing telemetry.
     - Vector PDF / print export directly from `DisplayList`.
  3. **Linux WebView Backend**:
     - Add **`WebKitGTK`** backend inside `modules/octara-webview` (matching the typed navigation, scheme handler, script injection, and Promise RPC surface of Apple `WKWebView`).
  4. **Advanced Hardware Modules (`modules/*`)**:
     - `modules/octara-biometrics` (Touch ID / Face ID / Linux Polkit-fprint / WebAuthn).
     - `modules/octara-audio` (recording, playback, level metering stream, audio session management).
     - `modules/octara-camera` (`NativeViewFactory` live preview + photo capture + QR/barcode scanning).
     - `modules/octara-location` (permissioned one-shot and streaming coordinates, `SENSITIVE = true`).

---

### `0.1.0` — Production Beta: Packaging, Fast Restart, DevTools & Core API Freeze
- **Planned Spec**: `v2/18-packaging-devtools-and-v01-freeze.md`
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

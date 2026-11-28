# 17 — Direct Native Platform Hosts (Zero `winit` Architecture)

> **Design premise**: Eliminate `winit` and `accesskit_winit` completely from the Octara workspace (`0` bytes in `Cargo.toml` and `Cargo.lock`). Each Tier-1 native platform owns its event loop, window, view, delegate, input, IME, gestures, drag-and-drop, accessibility, and frame clock directly behind one pure-Rust, platform-neutral `OsHost` / `OsWindow` SPI inside `octara-host-native`.
>
> **Apple SDK floor**: `macOS 15.4+` / `iOS 18.4+`.  
> **Tier-1 native targets**: macOS (AppKit), iOS (UIKit), Linux (Wayland + X11).  
> **Tier-2 target**: Windows (`0.2.0` direct Win32 `windows-rs` host; minimal `OsHost` stub until then—zero `winit`).

---

## 1. Why Remove `winit` Completely

1. **Delegate & View Ownership Conflicts on Apple**:
   - On macOS and iOS, Octara already writes direct `objc2-app-kit`, `objc2-ui-kit`, and `objc2-quartz-core` code for `CADisplayLink`, `NSTextInputClient`/`UITextInput`, `NSPasteboard`, `NSVisualEffectView`, `NSApplicationDelegate`, `UIWindowScene`, `UIAccessibility`, `UIPinchGestureRecognizer`, `StatusBarStyle`, and `CATransaction` native view commits.
   - Keeping `winit` underneath forces awkward subview wrapping, delegate forwarding/swizzling, and blocks clean first-class `NSPanel` (`NSWindowStyleMaskNonactivatingPanel`), `NSDraggingSession`, `UIViewController::preferredStatusBarStyle`, and App Store scene lifecycle ownership.
2. **Protocol Lag on Linux**:
   - `winit 0.30.x` lacks Wayland touchpad pinch/rotate (`zwp_pointer_gestures_v1`), lags on `zwp_text_input_v3` IME, fractional scaling, and `wl_data_device` drag-out. Direct Wayland (`wayland-client` / `smithay-client-toolkit` + `wayland-protocols`) with X11 (`x11rb`) fallback provides first-class Linux desktop fidelity.
3. **Cleaner Dependency Graph & Binary Size**:
   - Removing `winit` and `accesskit_winit` deletes an entire redundant windowing abstraction layer, shrinks compile times and unwind tables, and lets `octara-host-native/src/shell.rs` speak strictly Octara-owned types.

---

## 2. Platform-Neutral Native Host SPI (`octara-host-native/src/os/mod.rs`)

`shell.rs`, `window.rs`, and `os/mod.rs` MUST contain **zero `winit` imports** and **zero `#[cfg(target_os)]` branching in core orchestration**.

### 2.1 Core Types
- **`HostWake`**: `Arc<dyn Fn() + Send + Sync>` — wakes the platform event loop from worker threads (`octara-gpu`, `ModuleDispatcher`, `TaskScheduler`, timers).
- **`PlatformEvent`** (emitted by `OsHost` into `ShellDriver`):
  - `Launched` / `Resumed` / `Suspended` / `WillTerminate`
  - `Lifecycle(HostLifecycleEvent)` (`OpenUrls`, `OpenFiles`, `DockReopen`, `AppForeground`, `AppBackground`, `MemoryWarning`)
  - `SystemPreferencesChanged(SystemPreferences)`
  - `WindowCloseRequested(WindowId)` / `WindowDestroyed(WindowId)`
  - `WindowResized { window: WindowId, size: [u32; 2], scale: f32 }` / `WindowMoved(WindowId)`
  - `WindowFocused { window: WindowId, active: bool }`
  - `WindowRedrawRequested(WindowId)` / `FrameClockTick { window: WindowId, target_present: Instant, interval: Duration }`
  - `Interaction { window: WindowId, event: InteractionEvent }` (pointer, modifiers, wheel with real `WheelPhase`, touch, gesture, hover/leave)
  - `Key { window: WindowId, event: KeyEvent, text: Option<String> }`
  - `Editor { window: WindowId, event: EditorEvent }` (IME preedit/commit, native text commands)
  - `FileDrop { window: WindowId, phase: FileDropPhase, position: Option<Point>, paths: Vec<PathBuf> }`
  - `AccessibilityAction { window: WindowId, action: A11yAction }` / `AccessibilityEnabledChanged { window: WindowId, enabled: bool }`
  - `HostReply { window: WindowId, reply: HostReply }`
  - `UserWake` (coalesced wake for background tasks, timers, GPU frame completion, module messages)

### 2.2 `OsHost` & `OsWindow` Traits
- **`OsHost`**:
  - `fn run(self: Box<Self>, driver: Box<dyn ShellDriver>) -> Result<(), String>;` — enters the native run loop (`NSApplication::run`, `UIApplicationMain`, Wayland/X11 `calloop`/poll loop).
  - `fn wake(&self) -> HostWake;`
  - `fn create_window(&mut self, id: WindowId, options: &WindowOptions, context: OsWindowContext) -> Result<Box<dyn OsWindow>, String>;`
  - Global services: `set_menus`, `set_app_badge`, `request_user_attention`, `open_url`, `read_clipboard`, `write_clipboard`, `system_preferences`, thread QoS setup hooks.
- **`OsWindow`** (owns the native window handle directly — no `Arc<winit::window::Window>`):
  - `fn raw_handles(&self) -> Result<(RawDisplayHandle, RawWindowHandle), String>;` (via `raw-window-handle` 0.6 for `wgpu::SurfaceTargetUnsafe::RawHandle`)
  - `fn host_surface(&self) -> HostSurface<'_>;` (directly supplies `AppKit` / `UiKit` / `Wayland` / `Xlib` / `Win32` handles to `octara::module`)
  - `fn physical_size(&self) -> [u32; 2];` / `fn scale_factor(&self) -> f32;`
  - `fn set_cursor(&mut self, cursor: Option<CursorIcon>);`
  - `fn apply_presentation(&mut self, title: &str, color_scheme: ColorScheme, material: WindowMaterial, status_bar: StatusBarStyle, level: WindowLevel);`
  - `fn window_geometry(&mut self, viewport: WindowViewport) -> WindowGeometry;` / `fn set_window_geometry(&mut self, req: WindowGeometryRequest);`
  - `fn focus(&mut self);` / `fn minimize(&mut self);` / `fn toggle_maximize(&mut self);` / `fn start_window_drag(&mut self);` / `fn show(&mut self);`
  - `fn start_drag_out(&mut self, items: Vec<DragItem>) -> bool;`
  - `fn request_redraw(&mut self);` / `fn pre_present_notify(&mut self);`
  - Sub-capabilities: `text_input()`, `asks()`, `presentation()`, `scene()`, `accessibility()`, `native_views()`.

---

## 3. Direct Platform Backends

### 3.1 macOS `15.4+` (`crates/octara-host-native/src/os/macos/`)
- **App & Loop**: Direct `NSApplication::sharedApplication(mtm)` + `OctaraAppDelegate` (`NSApplicationDelegate`) + `CFRunLoopSource` / `dispatch_async_f` wake on main queue (`HostWake`).
- **Window**: `OctaraWindow` (`NSWindow` subclass) and `OctaraPanel` (`NSPanel` subclass with `NSWindowStyleMaskNonactivatingPanel` when `options.non_activating_panel == true`), `NSWindowDelegate` for resize, move, close request, occlusion/focus, fullscreen, and screen change.
- **View**: `OctaraView` (`NSView` subclass, `wantsLayer = true`, `CAMetalLayer` backing):
  - Input: `mouseDown`/`Up`/`Moved`/`Dragged`, `rightMouse*`, `otherMouse*`, `mouseEntered`/`Exited`, `scrollWheel:` (exact `NSEventPhase` & `momentumPhase` → `WheelPhase`), `magnifyWithEvent:` & `rotateWithEvent:`, `keyDown:`/`flagsChanged:`, `performKeyEquivalent:`.
  - Text/IME: direct `NSTextInputClient` conformance on `OctaraView` (zero subview/swizzle hacks).
  - Drag & Drop: direct `NSDraggingDestination` (file drop with exact view coordinates) and `NSDraggingSource` (`beginDraggingSessionWithItems:event:source:` for C5 Drag-Out).
  - Frame Clock: unified `CADisplayLink` via `NSView::displayLinkWithTarget:selector:` (auto-tracks `NSScreen`, adaptive `CAFrameRateRange`, `targetTimestamp`).
  - Accessibility: direct `accesskit_macos::Adapter` (dormant until queried; zero `accesskit_winit`).

### 3.2 iOS `18.4+` (`crates/octara-host-native/src/os/ios/`)
- **App & Scene**: Direct `UIApplicationMain` + `OctaraAppDelegate` (`UIApplicationDelegate`) + `OctaraSceneDelegate` (`UIWindowSceneDelegate` handling `scene:willConnectToSession:`, `scene:openURLContexts:`, `sceneWillEnterForeground:`, `sceneDidEnterBackground:`, memory warning) + main-queue `HostWake`.
- **ViewController & View**:
  - `OctaraViewController` (`UIViewController` subclass): directly implements `preferredStatusBarStyle`, `prefersStatusBarHidden`, `viewSafeAreaInsetsDidChange`, `traitCollectionDidChange:` (dark mode, Dynamic Type `text_scale`, `high_contrast`), and serves as the authentic `ui_view_controller` in `HostSurface::UiKit` for `octara-picker` (`PHPicker` / `UIDocumentPicker`).
  - `OctaraView` (`UIView` subclass with `CAMetalLayer` layer class): direct multi-touch (`touchesBegan`/`Moved`/`Ended`/`Cancelled`), `UIPinchGestureRecognizer` + `UIRotationGestureRecognizer`, `UITextInput` + `UITextInputTraits`, `UIKeyboardLayoutGuide` scene-local keyboard inset, `UIAccessibilityContainer` VoiceOver bridge, and unified `CADisplayLink`.

### 3.3 Linux (`crates/octara-host-native/src/os/linux/`)
- **Primary Backend — Direct Wayland (`wayland-client` + `smithay-client-toolkit` + `wayland-protocols` + `calloop`)**:
  - `xdg_shell` toplevel window (`title`, `min`/`max` size, maximize/minimize/fullscreen, interactive move via `xdg_toplevel::move_`, `WindowLevel` hints where supported).
  - `wp_fractional_scale_v1` + `wp_viewporter` for crisp fractional HiDPI scaling.
  - `zwp_text_input_v3` for native Wayland IME (`set_cursor_rectangle`, preedit, commit).
  - `zwp_pointer_gestures_v1` for native touchpad `Pinch` & `Rotate` gestures (eliminating the `winit 0.30` Linux gap!).
  - `wl_data_device` for native clipboard (multimodal text/PNG) and Drag-In / Drag-Out.
  - XDG Desktop Portal `org.freedesktop.appearance` (`color-scheme`, `reduced-motion`, `contrast`) into `SystemPreferences`.
  - `accesskit_unix` (dormant until AT-SPI activates).
- **Fallback Backend — Direct X11 (`x11rb`)**:
  - Clean X11 connection & event loop fallback when `WAYLAND_DISPLAY` is absent (window geometry, DPI/Xft scale, XInput/core pointer/wheel/keyboard, EWMH window levels & close/fullscreen, ICCCM/XDND clipboard & drops).

### 3.4 Windows (`crates/octara-host-native/src/os/windows/`)
- Zero `winit`: clean `OsHost` stub returning `Err("Windows direct Win32 host is scheduled for Octara 0.2.0")` so the workspace has **zero `winit` dependency edges on every target** while keeping the `os/windows/` slot ready for `0.2.0`'s direct `windows-rs` implementation.

---

## 4. Phased Implementation Plan & Verification Gates (`0.0.10`)

### Wave D1 — Platform-Neutral SPI + Direct Apple Hosts (macOS 15.4+ AppKit & iOS 18.4+ UIKit)
1. Refactor `octara-host-native/src/os/mod.rs`, `shell.rs`, and `window.rs` to eliminate every `winit` type from the shared host orchestration (`PlatformEvent`, `HostWake`, `OsHost::run`, `OsWindow::raw_handles`).
2. Replace macOS and iOS backends with direct AppKit (`OctaraAppDelegate`, `OctaraWindow`, `OctaraPanel`, `OctaraView`, `accesskit_macos`) and direct UIKit (`OctaraAppDelegate`, `OctaraSceneDelegate`, `OctaraViewController`, `OctaraView`, `UIAccessibility`), deleting `winit` & `accesskit_winit` from all Apple target dependencies in `crates/octara-host-native/Cargo.toml`.
3. Verify: `cargo tree -p octara-host-native` on macOS & `aarch64-apple-ios-sim` shows **0 `winit` and 0 `accesskit_winit`**; all native AppKit/WebKit/CADisplayLink/iOS Simulator tests and full workspace gates pass.

### Wave D2 — Direct Linux Host (Wayland + X11), Total `winit` Removal, C5 & C6-Airspace
1. Implement direct Linux host (`os/linux/` with Wayland `smithay-client-toolkit` + `zwp_text_input_v3` + `zwp_pointer_gestures_v1` + `wp_fractional_scale_v1` + XDG Portal + X11 `x11rb` fallback) and Windows `0.2.0` stub.
2. Delete `winit` and `accesskit_winit` completely from root `Cargo.toml`, `crates/octara-host-native/Cargo.toml`, and `Cargo.lock` (**0 occurrences workspace-wide**).
3. Land **C5** (Drag-Out `DragItem` / `el.on_drag_start` across macOS `NSDraggingSession`, Web `DataTransfer`, Linux `wl_data_device`, DST; `WindowLevel` + macOS native `NSPanel`) and **C6-Airspace** (`AirspacePolicy::SnapshotOnOcclude` with `DamageRect` & `SpatialTable` compatibility) directly on the clean native hosts.
4. Verify: `git grep -i winit Cargo.toml Cargo.lock crates/ modules/` returns **0 hits**; full macOS, iOS Simulator, Linux `aarch64-unknown-linux-gnu`, dual Wasm + 4 modules Wasm, Headless Chrome CDP, and workspace + CLI doctest gates pass green.

# Octara v2 Spec 08: Native Window Chrome, Rich Text Pills & Code Highlighting, Upgraded `WKWebView`, and Multi-Window Geometry (`0.0.3`)

> Part of the [Octara v2 Clean-Sheet Architecture](../architecture-v2.md).  
> Design premise: **Zero historical baggage, zero backwards compatibility, zero product coupling.**  
> Motivated by real production desktop & mobile usage.

---

## 1. Native Window Chrome, Drag Regions & Vibrancy

### 1.1 `TitlebarStyle`, `WindowChrome`, and `WindowMaterial` (`octara-model` / `octara-runtime` / `octara`)
Add host-independent window chrome and surface descriptors:

```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitlebarStyle {
    #[default]
    System,
    /// Content extends under the titlebar; platform controls (macOS traffic lights) inset cleanly.
    HiddenInset {
        traffic_light_offset: Option<Point>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VibrancyMaterial {
    #[default]
    Sidebar,
    UnderWindowBackground,
    Titlebar,
    Popover,
    Hud,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowMaterial {
    #[default]
    Opaque,
    Vibrancy(VibrancyMaterial),
}

/// Safe chrome insets reported by the host (e.g. macOS traffic-light reservation & titlebar height).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WindowChrome {
    pub titlebar_height: f32,
    pub traffic_light_rect: Option<Rect>,
    pub leading_inset: f32,
    pub trailing_inset: f32,
}
```

- Expose `BuildCx::window_chrome(&self) -> ReadSignal<WindowChrome>` updated via `HostReply::ChromeChanged(WindowChrome)`.
- Keep `WindowMaterial::Opaque` as the default with **zero** extra GPU alpha/compositor overhead.

### 1.2 Declarative Window Drag Regions (`El::window_drag_region`)
- Add `NodeFlags::WINDOW_DRAG_REGION` in `octara-tree` (zero growth to `NodeCore`'s 32 B footprint).
- Expose `El::window_drag_region(self, enabled: bool) -> Self` in `octara`.
- **Hit-test & Arbitration Contract**:
  - On primary pointer press inside a `WINDOW_DRAG_REGION`, if there is **no closer interactive descendant/ancestor** (no `Button`, `TextInput`, `Link`, press/toggle/drag handler, or focusable control inside the drag region), the runtime emits `HostCommand::StartWindowDrag { window }`.
  - Interactive children (tabs, search fields, icon buttons) inside the titlebar keep full click/focus/drag behavior without being swallowed.
  - Double-clicking an unclaimed drag region emits `HostCommand::ToggleMaximizeWindow { window }` (matching macOS titlebar zoom semantics).

### 1.3 Native macOS Implementation (`octara-host-native`)
- **`HiddenInset`**: configure `NSWindowStyleMask::FullSizeContentView`, `titlebarAppearsTransparent = true`, `titleVisibility = NSWindowTitleVisibility::Hidden`; position standard window buttons (`Close`, `Miniaturize`, `Zoom`) using `traffic_light_offset` when provided, and publish accurate `WindowChrome` metrics back to the runtime.
- **Vibrancy**:
  - Window-level `WindowMaterial::Vibrancy(mat)` mounts an `NSVisualEffectView` behind the transparent Metal surface with the mapped `NSVisualEffectMaterial`.
  - Per-region `El::vibrancy(VibrancyMaterial)` (optional region slot) allows translucent sidebar vibrancy while keeping opaque content regions crisp, or cleanly uses window vibrancy with opaque card backgrounds.
  - On non-macOS hosts (`desktop`, `ios`, `web`), `TitlebarStyle` and `WindowMaterial` degrade gracefully without errors or `#[cfg(target_os)]` leakage into `shell.rs` / `window.rs` / `presentation.rs`.

---

## 2. `RichSpan` Rounded Padded Backgrounds & Flexible `octara-kit::markdown` Customization

### 2.1 `RichSpan` Pill Geometry (`octara-model` / `octara-pipeline` / `octara-render` / `octara-host-web`)
Extend `octara_model::text::RichSpan` with:
```rust
pub struct RichSpan {
    pub weight: Option<FontWeight>,
    pub italic: bool,
    pub mono: bool,
    pub underline: bool,
    pub strike: bool,
    pub color: Option<ColorValue>,
    pub background: Option<ColorValue>,
    pub border_color: Option<ColorValue>,
    pub radius: Option<f32>,
    pub padding_x: Option<f32>,
}
```
- **Native Pipeline (`octara-pipeline` + `octara-render`)**:
  - `padding_x` expands the inline background pill horizontally (reserving clean horizontal breathing room / glyph run offsets so adjacent text never collides with the pill edge).
  - `radius` and optional `border_color` (1px hairline border when set) paint the span background as a rounded `Quad` (in both CPU reference renderer and wgpu GPU renderer!) behind the glyphs, with deterministic CPU-vs-Metal parity.
- **Web Host (`octara-host-web`)**:
  - Maps `radius` → `border-radius: {r}px`, `padding_x` → `padding-inline: {p}px`, and `border_color` → `1px solid ...` on inline `<span>` elements.
- **`octara-kit::markdown`**:
  - Update inline `Style::Code` spans to use clean default pill styling (`radius: Some(4.0)`, `padding_x: Some(4.0)`).

### 2.2 Three-Layer Orthogonal Customization in `octara-kit::markdown`
Add `MarkdownStyle`, `MarkdownOptions`, and `markdown_with(build, source, options)` (with `markdown(build, source)` delegating to `MarkdownOptions::default()`), preserving **0 `Rc::new`** and **0 `RefCell`** in `octara-kit/src` (store scope-owned callbacks via `BuildCx` / `Signal` / `Box` or `Arc`):

1. **Declarative Stylesheet `MarkdownStyle`** (every field defaults to current `Theme` tokens; override only what differs):
   - Typography & spacing: `block_gap`, `text_size`, `line_height`, `text_color`, `headings` (H1–H6 size / weight / top spacing), `list_gap`, `quote` (bar width, bar color, text color), `rule` (height, color).
   - Inline styles: `inline_code` (`background`, `text_color`, `border_color`, `radius`, `padding_x`, `font_size`), `link` (`color`, `underline`).
   - Block containers: `code_block` (`background`, `header_background`, `border_color`, `radius`, `padding`, `font_size`, `wrap_lines`, `show_header`, `show_copy`), `table` (`border_color`, `header_background`, `row_background`, `striped_background`, `cell_padding`, `radius`).
2. **Link Interception & Inline Link Styling Hooks**:
   - `on_link`: `impl Fn(&mut WriteCx<'_>, &str) -> bool + 'static` — returns `true` when handled by the app (e.g. opening local files, navigating views/threads), or `false` to fall back to the host's `open_url`.
   - `style_link`: optional `impl Fn(&str, &str, &mut Span) + 'static` — allows per-link inline `Span` customization (e.g. rendering file/view/thread references as rounded inline pills).
3. **Streaming-Preserving Block Extension Hooks**:
   - `code_highlighter`: `impl Fn(&str, &str) -> Vec<Rich> + 'static` — runs only when a code block's text or language changes (incremental per-block memoization + per-line identity via `derive_list`), preserving streaming append efficiency and cross-block selection/copy.
   - `render_code_block`: optional `impl Fn(&mut BuildCx<'_>, &str, ReadSignal<Option<String>>) -> Option<El> + 'static` (or equivalent reactive block snapshot) — returns `None` to fall back to the default incremental highlighted `code_view`, or `Some(El)` to take over specific fenced languages (e.g. `diff`, `mermaid`) while retaining `BlockCache`, streaming updates, and cross-block selection/copy.

---

## 3. `WKWebView` Native Island Upgrade (`Underlay`, Bidirectional JS Bridge & iOS)

### 3.1 `NativeSlot::Underlay` for `"web"` on macOS
- Remove the `"web" => Overlay`-only restriction in `crates/octara-host-native/src/os/macos/native.rs`.
- Allow `"web"` in both `NativeSlot::Underlay` and `NativeSlot::Overlay`.
- In `NativeSlot::Underlay`, `"web"` participates in the exact same Metal alpha-hole + `NativePlacement::covered` hit-test exclusion mechanism as `"map"`, guaranteeing that Octara's `CommandPalette`, `Popover`, `Menu`, `ContextMenu`, and `Dialog` always paint cleanly **above** `WKWebView` and intercept pointer events over covered rects!

### 3.2 First-Class `NativeCommand` & Bidirectional JS Bridge
Add imperative host-routed commands for mounted native islands:
- `octara-model::native::NativeCommand { id: NativeId, name: String, data: Value }`
- `octara-runtime`: `HostCommand::NativeCommand { window: WindowId, command: NativeCommand }` and `WriteCx::send_native_command(target_test_id: &str, name: impl Into<String>, data: Value) -> bool` (resolving live mounted `NodeKind::Native` node in the current window).
- **`WKWebView` Protocol (macOS + iOS)**:
  - **Inbound commands** (`NativeCommand`):
    - `"evaluate_js"`: `{ "script": "...", "request_id"?: "..." }` → evaluates via `evaluateJavaScript:completionHandler:` and emits `NativeEvent { name: "js_result", data: { "request_id": ..., "value"?: ..., "error"?: ... } }`.
    - `"go_back"`, `"go_forward"`, `"reload"`, `"stop_loading"`.
    - `"find"`: `{ "query": "...", "backwards"?: bool, "case_sensitive"?: bool }` & `"clear_find"` (using `WKFindConfiguration` / `findString:withConfiguration:completionHandler:` or JS fallback cleanly).
  - **Outbound JS → Rust bridge**:
    - Register a `WKScriptMessageHandler` named `"octara"` on `WKUserContentController` so page scripts can call `window.webkit.messageHandlers.octara.postMessage(payload)` and deliver `NativeEvent { name: "post_message", data: payload }` to `El::on_native`.
    - Clean up the message handler on `destroy` to prevent Objective-C retain cycles!
  - **Navigation state events**:
    - Observe `URL`, `title`, `loading`, `canGoBack`, `canGoForward` and emit structured `NativeEvent { name: "navigation", data: { "url": ..., "title": ..., "loading": bool, "can_go_back": bool, "can_go_forward": bool } }` (alongside existing `load` / `fail` / `title` events).

### 3.3 iOS `WKWebView` Support (`octara-host-native/src/os/ios/native.rs`)
- Implement `WindowNativeViews` on `IosWindow` hosting `WKWebView` with full parity for `props` (`url`, `html`, `base_url`), `NativeCommand` (`evaluate_js`, `go_back`, `go_forward`, `reload`, `stop_loading`, `find`, `clear_find`), `WKScriptMessageHandler` (`post_message`), and `navigation` / `load` / `fail` / `title` events.
- Verify clean compilation for `aarch64-apple-ios-sim`.

---

## 4. Multi-Window Control, Live `WindowGeometry` & Persistence

Note: `WindowActions::open_window` and `WriteCx::close_window` already exist in `0.0.2`. `0.0.3` completes the window geometry and persistence contract:

### 4.1 `WindowGeometry` & Extended `WindowOptions`
```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub position: Option<Point>,
    pub size: Size,
    pub scale: f32,
    pub maximized: bool,
    pub fullscreen: bool,
}
```
Extend `WindowOptions`:
- `position: Option<Point>`
- `min_size: Option<Size>`
- `max_size: Option<Size>`
- `resizable: bool` (default `true`)
- `maximized: bool` (default `false`)
- `titlebar: TitlebarStyle`
- `material: WindowMaterial`
- `persistence_key: Option<String>` (when set, automatically persists and restores `WindowGeometry` through `RestartState`!)

### 4.2 Reactive Observation & Imperative Window Controls
- `BuildCx::window_geometry(&self) -> ReadSignal<WindowGeometry>` updated by `HostReply::GeometryChanged(WindowGeometry)` whenever the OS window moves, resizes, maximizes, or enters/exits fullscreen.
- `WriteCx` window commands:
  - `set_window_title(&mut self, title: impl Into<String>)`
  - `set_window_geometry(&mut self, geometry: WindowGeometryRequest)`
  - `focus_window(&mut self, id: WindowId)`
  - `minimize_window(&mut self)`
  - `toggle_maximize_window(&mut self)`
- Wire all commands cleanly through `HostCommand` and `octara-host-native` (`shell.rs` + `os/`), preserving zero `#[cfg(target_os)]` in `shell.rs` / `window.rs` / `presentation.rs`.

---

## 5. Verification Gates (`0.0.3`)
1. `python3 tools/check_layers.py` (0 violations)
2. `cargo test --workspace` (including new tests for: drag region hit-test/child non-swallowing/double-click zoom, `RichSpan` pill CPU+GPU parity & Web projection, `MarkdownOptions::with_code_highlighter` streaming & selection, `WKWebView` Underlay covered hit-test + JS bridge/nav state, multi-window geometry & `RestartState` persistence)
3. `cargo test -p octara-cli --doc`
4. `cargo clippy --workspace --all-targets -- -D warnings` (0 warnings, including Cargo manifest warnings)
5. `cargo check --target wasm32-unknown-unknown -p octara-host-web` & `--features webgpu`
6. `cargo check --target aarch64-apple-ios-sim -p octara-host-native` (if iOS target installed)
7. `cargo run -p octara-cli -- skill export skills` (idempotent, 0 diff)
8. `cargo publish --workspace --dry-run` (all 18 crates at `0.0.3` clean!)

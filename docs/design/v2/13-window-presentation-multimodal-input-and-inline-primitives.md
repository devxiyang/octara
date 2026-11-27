# 13 — Window Presentation, Dual-Parent Inheritance, Multimodal Input & Real-Metric Inline Primitives (`0.0.7`)

> **Design premise**: Solve every cross-cutting UI gap at the right architectural layer with **zero backwards-compatibility shims, zero dual-track APIs, and zero app-specific patches**.  
> **Prerequisite**: Builds cleanly on `0.0.6`'s sparse ECS (`DirtyBitmap`, `FlatTree`, `u64` Bloom Filter), final-form Signals (`Store<T>`, `Readable`/`Writable`, 8 B `ReactiveId`), 4 GPU micro-pipelines, and all 6 SPI boundaries.

---

## 1. Five Orthogonal Framework Mechanisms

| # | Mechanism | Architectural Layer | Replaces / Eliminates |
| --- | --- | --- | --- |
| **M1** | **Dual-Parent Tree Invariant** (`physical_parent` vs `logical_parent` + `logical_portals` reverse index) | `octara-tree` (L1) | Manual portal `ThemePatch` re-wrapping; stale library-internal popovers |
| **M2** | **Unified Reactive `WindowPresentation`, `AppStatus` & `WindowViewport.keyboard_inset`** | `octara-model` / `octara-runtime` / `octara-pipeline` / `octara-host-native` (L0–L3) | Startup-frozen `theme` / `material`; drift between `tree.theme`, GPU clear color, and OS window chrome |
| **M3** | **Unified `ClipboardItem` / `PasteEvent` Protocol + Ergonomic `Ask` Path Helpers** | `octara-model` / `octara-tree` / `octara-runtime` / `octara-host-*` (L0–L3) | Text-only clipboard ceiling (`ReadClipboardText`); missing multimodal paste interception |
| **M4** | **Precise Modifier Routing (`LinkEvent`), Ordered SPSC `TextStreamOp`, and Layered Hover Tooltip** | `octara-model` / `octara-tree` / `octara-runtime` / `octara` / `octara-kit` (L0–L2) | Blanket modified-click suppression; polling stream resets; silent `El::tooltip` on desktop |
| **M5** | **First-Class `InlineContent::Pill`, Clean `octara-markdown` Upgrades & `__eh_frame` Monomorphization Hygiene** | `octara-model` / `octara-pipeline` / `octara-host-web` / `octara-markdown` / `octara` / `octara-kit` (L0–L3) | `0.70 em` heuristic pill sizing & detached icons; non-reactive streaming effects; debug unwind bloat |

---

## 2. Mechanism Specifications

### 2.1 M1 — Dual-Parent Tree Invariant (`octara-tree`)
A mounted `Portal` has two distinct parent edges with cleanly separated responsibilities:
- **`physical_parent(node)`** (`NodeCore::parent`): governs paint stacking order (`overlay_root`) and viewport clip escape.
- **`logical_parent(node)`** (`overlay.owner` when present, else `NodeCore::parent`): governs modal scope, focus restoration, event bubbling, and **every inherited cascading property** (`ThemePatch`, inherited typography defaults, inert/disabled ancestry).

**Requirements**:
1. Maintain a sparse reverse index `logical_portals: SecondaryMap<NodeId, SmallVec<[NodeId; 2]>>` updated automatically when portals attach, move, or retire (verified by `Tree::validate_integrity()`).
2. `Tree::theme_scope_at(node)` and inherited style lookups walk `logical_parent(current)` rather than `physical_parent(current)`.
3. Downward style invalidation and subtree `u64` Bloom propagation traverse both physical children and `logical_portals` so changing a container's `ThemePatch` or `ColorScheme` immediately updates every detached popover/dialog/combobox owned by that subtree—including library-internal overlays such as `MarkdownComposer`'s completion popup.

---

### 2.2 M2 — Unified Reactive `WindowPresentation`, `AppStatus` & `WindowViewport`
Eliminate the split between startup-only `WindowOptions` (`theme`, `material`, `title`) and runtime host state:

1. **Single Window Presentation Source of Truth**:
   - Define `WindowPresentation { pub title: Arc<str>, pub theme: Theme, pub material: WindowMaterial }` in `octara-model` / `octara-runtime`.
   - Support declarative reactive bindings on window setup (`WindowDefinition::theme_with`, `material_with`, `title_with`, or unified `presentation_with`) AND imperative updates (`WriteCx::set_window_theme`, `set_window_material`, `set_window_title`) feeding the **same** settled `WindowPresentation` state.
   - In Phase 5 (`Publish`), diff the settled `WindowPresentation` once per cycle:
     - On `theme` change: dispatch `RuntimeOp::ThemeChanged(Box::new(theme))` into `Tree` (invalidating all theme-sensitive subtrees + `logical_portals` + overlay scrollbars), updating GPU clear color (`tree.theme().get(ColorToken::Background)` when opaque) automatically.
     - Emit a single `HostCommand::UpdateWindowPresentation { window, title, color_scheme, material }` to `OsWindow::apply_presentation(...)`:
       - **macOS**: update `NSWindow.appearance` (`NSAppearanceNameDarkAqua` / `NSAppearanceNameAqua`), attach/detach/update `NSVisualEffectView` live without recreating the window, update `transparent_window` clear mode, and update `NSWindow.title`.
       - **Desktop / iOS / Web**: apply supported fields (`title`, color scheme / `color-scheme` CSS hint, opaque/translucent fallback) gracefully without `#[cfg(target_os)]` in core orchestration.
2. **Application Attention & Badge (`AppStatus`)**:
   - Expose `UserAttention::{Informational, Critical}` and `WriteCx::set_app_badge(Option<Arc<str>>)` / `WriteCx::request_user_attention(UserAttention)` via `HostCommand` + `OsHost` / `OsWindow`:
     - **macOS**: `NSApplication.dockTile.badgeLabel` and `requestUserAttention:` (`NSInformationalRequest` / `NSCriticalRequest`).
     - **Desktop (Linux/Windows)**: `winit::window::Window::request_user_attention`; graceful no-op for unsupported badge platforms.
     - **Web / Headless**: graceful no-op + recorded in `HostCommand` receipts for DST verification.
3. **Mobile Soft Keyboard Inset**:
   - Add `pub keyboard_inset: f32` (default `0.0`, logical pixels occluded at the bottom edge by an on-screen keyboard) to `WindowViewport`.
   - Expose reactively through `BuildCx::viewport()` (and `WindowViewport::visible_bounds()`).
   - Wire iOS host (`crates/octara-host-native/src/os/ios/`) to observe `UIKeyboardWillChangeFrameNotification` / `UIKeyboardWillHideNotification` and emit `HostReply::ViewportChanged`.

---

### 2.3 M3 — Unified Clipboard & Paste Protocol + `Ask` Path Ergonomics
Keep `Ask` (native filesystem path dialogs) and `ServiceRequest` (cross-platform data transfer) strictly orthogonal:

1. **Unified Clipboard Value Model (`octara-model`)**:
   ```rust
   #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
   #[serde(rename_all = "snake_case")]
   pub enum ClipboardItem {
       Text(String),
       Image { media_type: String, bytes: Vec<u8> },
       Files(Vec<PathBuf>),
   }

   bitflags! {
       pub struct ClipboardKinds: u8 {
           const TEXT  = 1 << 0;
           const IMAGE = 1 << 1;
           const FILES = 1 << 2;
       }
   }

   #[derive(Clone, Debug, PartialEq, Eq)]
   pub struct PasteEvent {
       pub items: Vec<ClipboardItem>,
   }
   ```
2. **General `HandlerKind::Paste` (`el.on_paste(|w, ev: &PasteEvent| -> bool)`)**:
   - Triggered on host paste gestures (`⌘V` / `Ctrl+V` / Edit→Paste) targeting the focused node and bubbling upward along `logical_parent` until a handler returns `true`.
   - If no handler consumes the event and a `ClipboardItem::Text` payload is present, fall through to the focused editor's default text insertion.
   - Works uniformly across `TextInput`, `MarkdownComposer`, canvases, or window-level drop/paste zones.
3. **Unified `ServiceRequest::ReadClipboard { accept: ClipboardKinds }`** → `ServiceOutcome::Clipboard(Vec<ClipboardItem>)`:
   - Replaces the text-only `ReadClipboardText` ceiling (zero duplicate text-only vs. image-only request proliferation; keep helper `ServiceOutcome::clipboard_text(&self) -> Option<&str>` for one-line text extraction).
   - **macOS**: read `NSPasteboard` file URLs (`NSURL`), PNG/TIFF/JPEG image representations (normalized to PNG/JPEG bytes with `media_type`), and UTF-8 text.
   - **Desktop / iOS / Web**: read supported clipboard payloads (text + image/files where platform APIs allow).
   - **Privacy & DST**: recorders and traces MUST omit raw clipboard transfer bytes.
4. **Native Directory & File Chooser Ergonomics**:
   - Keep `Ask::Paths(PathPromptOptions)` as the single native path chooser (already implemented on macOS `NSOpenPanel` and Desktop `rfd`).
   - Add concise constructors `Ask::pick_directory(prompt: Option<String>)` and `Ask::pick_files(prompt: Option<String>, multiple: bool)` on `Ask` so callers never hand-assemble `PathPromptOptions`.

---

### 2.4 M4 — Precise `LinkEvent` Modifiers, Ordered SPSC `TextStreamOp`, and Layered Hover Tooltip
1. **`LinkEvent.modifiers` & Precise Selection Guard**:
   - Add `pub modifiers: Modifiers` to `LinkEvent`.
   - In `crates/octara-tree/src/arbiter.rs`, stop suppressing link activation for all non-empty modifiers; suppress synthetic link `Press` **only** when `Shift` is extending an active text selection. `Cmd`, `Ctrl`, `Alt`, and non-selecting `Shift` clicks all dispatch `LinkEventKind::Press` with exact `modifiers`.
2. **Ordered SPSC `TextStreamOp` + Synchronous Drain**:
   - Upgrade `crates/octara-runtime/src/graph/text_stream.rs` ring slots from `Option<String>` to `Option<TextStreamOp>` where `enum TextStreamOp { Append(String), Clear, Replace(String) }`, keeping the 128 B cache-line-aligned lock-free SPSC layout intact.
   - Expose `TextStreamSink::try_clear(&mut self) -> Result<(), TextStreamError>` and `TextStreamSink::try_replace(&mut self, text: String) -> Result<(), TextStreamError>` (returning typed `TextStreamOp` or value on full/closed), plus `WriteCx::drain_text_stream(&mut self, buffer: TextBuffer) -> bool` for immediate synchronous catch-up on cycle boundaries.
3. **Layered Global Hover Tooltip (Zero Host-Side Widget Drawing)**:
   - **Layer 1 (`octara-runtime` + `octara-tree`)**:
     - Track the single hovered node with a non-empty `semantics.tooltip` in the window runtime using the virtual clock (default open delay `450 ms`, warm-hop grace `250 ms`, immediate dismiss on pointer press/leave/key/scroll/modal change).
     - Expose `BuildCx::hovered_tooltip(&self) -> ReadSignal<Option<HoveredTooltip>>` (`{ pub node: NodeId, pub text: Arc<str> }`) — zero per-button reactive allocations.
   - **Layer 2 (`octara` + `octara-kit`)**:
     - Provide a clean default `WindowTooltipLayer` (auto-mounted by `Window` unless opted out or overridden via `WindowDefinition`) rendering ONE shared `Portal` anchored to `AnchorTarget::Node(node)` with viewport edge avoidance, inheriting `ThemePatch` (via M1) and honoring `SystemPreferences.reduced_motion`.
     - Zero UI widget painting inside `octara-pipeline` or `octara-host-native`; 100% identical across Native, Web, DST, and `octara audit`.

---

### 2.5 M5 — Layer-0 `InlineContent::Pill`, `octara-markdown` Refinements & `__eh_frame` Hygiene
1. **First-Class `InlineContent::Pill` (`octara-model` + `octara-pipeline` + `octara-host-web`)**:
   - Add `InlinePill` (`icon: Option<NodeIcon>`, `label: Arc<str>`, `style: PillStyle` with `size`, `weight`, `color`, `background`, `border_color`, `radius`, `padding_x`, `padding_y`, `gap`) as a first-class `InlineContent::Pill(InlinePill)` variant in `octara-model`.
   - **`octara-pipeline`**: measure `label` with **real Parley glyph shaping**, compute exact width `padding_x + icon_w + gap + text_w + padding_x` and height `max(icon_h, text_h) + 2 * padding_y`, and paint one symmetric rounded pill (background, optional border, centered icon + label) with accurate baseline alignment.
   - **`octara-host-web`**: project `InlineContent::Pill` as a single native `<span data-octara-pill>` inline-flex element with exact CSS metrics.
   - **`octara-markdown`**: delete the `0.70 em` `pill_label` heuristic and external icon attachment in `render/inline.rs`; emit `InlineContent::Pill` directly.
2. **`octara-markdown` Framework Upgrades**:
   - **`TextRecognizer::file_paths`**: recognize optional `#L<n>`, `#L<n>-L<m>`, and `#L<n>-<m>` fragment line ranges (e.g., `./Cargo.toml#L10-L20`, `src/lib.rs#L42`).
   - **`MarkdownComposer` Rich Completions**: extend `Completion` with optional `icon: Option<Icon>`, `description: Option<Arc<str>>`, `category: Option<Arc<str>>` (builder methods `.icon(...)`, `.description(...)`, `.category(...)`), render structured rows by default, and provide a general custom completion-row builder hook so applications can customize suggestion rows cleanly.
   - **Reactive `MarkdownRenderer::streaming_effects`**: update trait signature (zero backwards compat) to `fn streaming_effects(&self, _read: &mut ReadCx<'_>) -> StreamingEffects` and add `MarkdownConfig::streaming_effects_with(...)`; automatically suppress fade reveal when `SystemPreferences.reduced_motion` is active unless explicitly overridden.
   - **`CodeHighlighter` Line-Ending Contract**: normalize `\r\n` / `\r` cleanly before validating highlighted line count against source lines so Windows/mixed line endings never silently fall back to the built-in highlighter.
3. **Systematic `__eh_frame` Monomorphization Hygiene (`octara` + `octara-kit`)**:
   - Outline non-generic `#[inline(never)]` inner implementations behind thin generic shims on the highest-fanout builders (`El` event/prop/child/style methods, `Styled::when`, and `octara-kit` high-frequency controls).
   - Quantify macOS debug/test binary `__eh_frame` + `__unwind_info` bytes before vs. after on `apps/chat` and `examples` using `size -m` / `otool -l`.

---

## 3. Phased `0.0.7` Rollout & Exit Gates

| Wave | Scope | Hard Exit Gate |
| --- | --- | --- |
| **Wave 1: Core Mechanisms (M1–M4)** | Dual-parent `logical_parent`/`logical_portals` inheritance & invalidation; reactive `WindowPresentation` (`theme`/`material`/`title`) + GPU clear color + macOS `NSWindow.appearance`/Vibrancy + `AppStatus` (badge/attention) + `WindowViewport.keyboard_inset` (iOS); unified `ClipboardItem`/`PasteEvent`/`HandlerKind::Paste` + `ServiceRequest::ReadClipboard` + `Ask::pick_directory`; `LinkEvent.modifiers` & selection-only suppression; ordered SPSC `TextStreamOp` + `drain_text_stream`; Layer-1 `hovered_tooltip` + Layer-2 `WindowTooltipLayer` | Portal theme inheritance & invalidation tests (including `MarkdownComposer` dark-mode popup); reactive theme/material/title & GPU clear-color parity tests; paste bubbling & privacy trace redaction tests; modified link click vs. `Shift+Click` selection tests; FIFO `TextStreamOp` + `drain_text_stream` tests; DST deterministic hover tooltip tests; layers, strict Clippy, dual Wasm & `ios-sim` green |
| **Wave 2: Inline Primitives, Markdown, `__eh_frame` & `0.0.7` Release (M5)** | Layer-0 `InlineContent::Pill` (Parley real-metric Native + inline-flex Web), delete `0.70 em` heuristic; `file_paths` `#L10-L20`; `MarkdownComposer` rich `Completion` + custom row builder; reactive `streaming_effects(&mut ReadCx)` + `reduced_motion` + `CodeHighlighter` CRLF normalization; thin-wrapper `__eh_frame` outlining with before/after byte measurement; bump all 21 workspace members to `0.0.7`, refresh `api.json.gz` & `skills/octara/` | Real-metric `InlineContent::Pill` Native CPU↔GPU parity & Web DOM tests; `#L10-L20` recognizer tests; rich completion & reactive `reduced_motion` tests; measured `__eh_frame` reduction report; all 9/9 release gates (`check_layers`, Clippy `-D warnings`, 4× Wasm, `ios-sim`, `octara-markdown --all-features`, `OCTARA_VERIFY_RELEASE_API=1 cargo test --workspace`, `octara-cli --doc`, 19/19 `cargo publish --workspace --dry-run`) green |

## 4. Debug Unwind Size

Measured on `aarch64-apple-darwin` with Rust 1.100.0 from Wave 1 (`e18b35c`) through Wave 2. Both builds used the workspace's standard debug profile and the same set of test targets.

```sh
env -u CARGO_INCREMENTAL -u CARGO_PROFILE_DEV_DEBUG \
  cargo test -p octara-chat -p octara-examples --no-run --message-format=json
xcrun size -m <test-executable>
```

Sum the two Mach-O sections across Cargo's test-profile executable artifacts. All measurements are in bytes.

| Package | Test executables | `__eh_frame` before → after | `__unwind_info` before → after | Combined before → after | Combined reduction |
| --- | ---: | ---: | ---: | ---: | ---: |
| `apps/chat` | 9 | 22,336,812 → 22,232,384 | 3,977,288 → 3,955,720 | 26,314,100 → 26,188,104 | 125,996 B (0.48%) |
| `examples` | 18 | 12,397,716 → 12,323,632 | 2,188,664 → 2,173,888 | 14,586,380 → 14,497,520 | 88,860 B (0.61%) |

The `chat` harness dropped from 4,398,236 to 4,359,932 bytes combined. The `acceptance` harness dropped from 5,014,456 to 4,901,728 bytes. The `rich_text` harness gained 21,888 bytes and now includes the CPU and GPU comparison for the inline pill.

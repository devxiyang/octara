# 12 — Pluggable SPI Architecture (`0.0.6`)

> **Design premise**: Keep Octara's core crates minimal, composable, and zero-fork extensible.  
> **North star**: Every major engine subsystem exposes a thin, stable, compile-time-typed Rust SPI trait with a complete, production-ready built-in default — allowing applications and external crates to swap renderers, media backends, document renderers, audit passes, release transports, or OS host integrations cleanly without forking Octara.

---

## 1. Three Extension Design Principles

1. **Zero Core Forks**: External crates depend on published Octara crates as standard Cargo dependencies and register implementations at bootstrap (`App::builder().with_*`) or subtree scope (`BuildCx::provide::<Arc<dyn ...>>`).
2. **Complete Built-In Defaults**: Every SPI slot ships with a fast, tested, production-ready default implementation out of the box.
3. **Value Boundaries**: Extension data uses shared values such as `DisplayList`, `ImageSource`, `Rect`, `PaintScene`, and `AuditFinding`. Inspection rules receive read-only audit data. Surface and OS adapters receive the host resources their platform interfaces require.

---

## 2. The Six Core SPI Boundaries

| # | SPI Trait(s) | Crate Home | Built-In Default | Extension Use Cases |
| --- | --- | --- | --- | --- |
| **SPI-1** | `SurfaceRenderer` | `octara-render` ↔ `octara-gpu` | Cross-platform `wgpu` micro-pipelines (`GlyphPipeline`, `Quad`, `Shadow`, `Image`, Early-Z) | Custom GPU compute backends, wide-gamut / HDR compositors, specialized vector/3D canvas pipelines |
| **SPI-2** | `MediaResolver`, `MediaCacheBackend`, `VideoSurfaceProvider` | `octara` / `octara-runtime` | `ImageSource`, bounded RAM caches, opt-in native disk cache, Web `<img>` and `<video>`, native video and poster fallback | URI resolution, persistent caches, hardware-accelerated video, custom document renderers |
| **SPI-3** | `BlockSlotRenderer`, `CodeHighlighter`, `MathRenderer`, `EditorBackend` | `octara-markdown` / `octara-tree` | `octara-markdown` core, standard syntax highlight, vector LaTeX math, `EditorRope` + `MarkdownComposer` | Custom interactive AI artifacts, diagram/chart engines (Mermaid/SVG/GPU charts), specialized IDE code/diff editors, domain-specific widgets |
| **SPI-4** | `AuditRule`, `InspectTransport`, `TraceRecorder` | `octara-inspect` | Text clipping, accessibility names, focus reachability, JSON inspection, replay | Project-specific audit rules, alternate transports and trace sinks |
| **SPI-5** | `ReleaseProvider`, `SecurityPolicy` | `octara-release`, declarative compiler in `octara` | HTTPS update channel, Ed25519 verification, and declarative scene compilation to a `Transaction` | Alternate sources for signed packets and additional acceptance rules |
| **SPI-6** | `OsHost`, `OsWindow` | `octara-host-native` | `winit` with macOS, iOS, Linux and Windows host backends | Platform-specific input, window and display integration |
| **SPI-7** | `OctaraModule`, `ModuleCall`, `ModuleEvent`, `NativeViewFactory` | `octara-model` / `octara-runtime` / `octara::module` | Core `ModuleRegistry`, `HostLifecycleEvent` bus, `NativeViewRegistry`, optional first-party `modules/*` | Pluggable native & web capabilities (WebView, Video, Picker, Haptics, Notifications, Share, Secure Store, Desktop Shell) and custom native views |

---

### SPI-1 Surface renderer registration

Register a factory with `octara_host_native::run_app_with_surface_renderer` for native windows and GPU captures. With the `webgpu` feature, use `octara_host_web::WebApp::launch_with_surface_renderer` for local GPU canvases. The host creates a backend for each GPU device, including replacements after device loss. The backend receives a borrowed `DisplayList`, the required output color encoding, and a command encoder. Use `DisplayList::placed_items` to traverse the shared drawing payloads with their offset and fade. Return paint receipts for the recorded frame or `Ok(None)` to use the built-in renderer for that frame.

### SPI-2 Media registration

Use `BuildCx::provide::<Arc<dyn MediaResolver>>` for logical URI resolution at a view scope. Register a persistent response store with `AppDefinition::with_media_cache_backend` and a custom native video slot provider with `AppDefinition::with_video_surface_provider`. Returning `None` from either resolver or video provider selects the built-in behavior. The persistent cache receives public HTTP bodies and validators only. The runtime enforces source policy before giving the Native loader a cache backend. See [Async Media](10-ai-document-media-and-markdown.md#34-viewport-aware-async-media-zero-cls-probing--3-tier-image-cache).

### SPI-3 Markdown and editor registration

Provide `Arc<dyn BlockSlotRenderer>`, `Arc<dyn CodeHighlighter>`, and `Arc<dyn MathRenderer>` through `BuildCx::provide` for a subtree. Use `markdown_with` and `MarkdownConfig::math_renderer` to override formula rendering for a single document. The built-in Markdown renderer handles blocks and syntax highlighting. Enable the `math` feature for the built-in LaTeX renderer.

Provide an `Arc<dyn EditorBackend>` through `BuildCx::provide` for a subtree, or set a fallback with `TextInput::editor_backend`. For nonsecure inputs using `.value` or `.bind_value`, the backend receives logical text and returns optional `EditorRichText` in UTF-8 byte coordinates. Returning `None` lets the next backend or the input's authored rich text supply the presentation. `MarkdownComposer` supplies a fallback for Markdown styling and atomic completion links. The retained editor settles the value, IME composition, selection, and undo history. See [Rich Composer](10-ai-document-media-and-markdown.md#52-ai-rich-composer-feature--composer).

### SPI-4 Inspection registration

Add an `Arc<dyn AuditRule>` to `Inspector::with_audit_rule` for every audited window or supply it through `BuildCx::provide` for a subtree. Rules read settled facts in retained tree order and emit `AuditFinding` values. Each authored finding can carry its source file and line. The built-in checks run with an `audit` protocol request or through `octara audit`.

`Inspector::with_transport` supplies the wire codec used by `Inspector::exchange`. The built-in Unix socket and Web bridges use `JsonLines` framing. For other formats, the host frames the payload and calls `exchange` on the application thread. Use `Inspector::with_trace_recorder` to receive timestamped inputs and requests after privacy filtering. `Replay` stores the same entries for deterministic playback. Install a configured inspector in `HeadlessDriver::with_inspector` or in a host's inspection hook. A native `ShellHook::before_input` can forward ordinary host input through `Inspector::record_input`.

### SPI-5 Release provider and policy registration

Pass a `ReleaseProvider` to `Store::check` to read a release pointer and stream its signed packet. The `http` feature provides an HTTPS `Channel` that implements this interface. The store bounds the read and verifies the installed Ed25519 key, target, sequence floor and packet digest. Register an `Arc<dyn SecurityPolicy>` with `Store::with_security_policy` to apply additional rules to the verified data during staging and startup, including rollback.

Use `ScenePacket::compile` to produce a declarative tree `Transaction`. For runtime mounting, use `ScenePacket::into_view` with the application's named command callbacks and pass the view to a `WindowDefinition`. This uses the same transaction preflight and callback lifetime as authored views.

### SPI-6 OS host registration

Pass an `OsHost` to `octara_host_native::run_app_with_options` through `NativeAppOptions::os_host`. The event loop initializes the host and asks it for an `OsWindow` for each native window. Supply `WindowNativeViews` on the returned window to handle custom native slots, including kinds selected by `VideoSurfaceProvider`. The same options accept a renderer factory and an inspection hook. By default Octara selects its built-in platform host.

## 3. SPI Implementation Discipline

1. **Strict Downward DAG**: Each extension boundary lives in the crate that uses it. `tools/check_layers.py` enforces the production dependency graph.
2. **Scoped vs. Engine-Wide Registration**:
   - Host-wide backends attach at their owning entry points. The host, inspector and release store accept the implementations they use.
   - Subtree-scoped extensions (`MediaResolver`, `BlockSlotRenderer`, `CodeHighlighter`, `MathRenderer`, `EditorBackend`, `AuditRule`) attach via `BuildCx::provide::<Arc<dyn ...>>`, enabling per-window or per-pane composition.
3. **Graceful Fallback Contract**: Extension hooks that inspect dynamic content (`BlockSlotRenderer::render_slot -> Option<El>`, `CodeHighlighter::highlight -> Option<...>`) degrade cleanly to the built-in default on `None`.

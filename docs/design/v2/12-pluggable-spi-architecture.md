# 12 — Pluggable SPI Architecture: Modular Extension Boundaries

> **Design premise**: Keep Octara's core crates minimal, composable, and zero-fork extensible.  
> **North star**: Every major engine subsystem exposes a thin, stable, compile-time-typed Rust SPI trait with a complete, production-ready built-in default — allowing applications and external crates to swap renderers, media backends, document renderers, audit passes, release transports, or OS host integrations cleanly without forking Octara.

---

## 1. Three Extension Design Principles

1. **Zero Core Forks**: External crates depend on published Octara crates as standard Cargo dependencies and register implementations at bootstrap (`App::builder().with_*`) or subtree scope (`BuildCx::provide::<Arc<dyn ...>>`).
2. **Complete Built-In Defaults**: Every SPI slot ships with a fast, tested, production-ready default implementation out of the box.
3. **Frozen Value Boundaries**: Every SPI exchanges only pure value types from `octara-model` (`DisplayList`, `ImageSource`, `Rect`, `PaintScene`, `TreeDelta`, `Transaction`, `AuditFinding`) — never internal mutable handles or private crate internals.

---

## 2. The Six Core SPI Boundaries

| # | SPI Trait(s) | Crate Home | Built-In Default | Extension Use Cases |
| --- | --- | --- | --- | --- |
| **SPI-1** | `SurfaceRenderer` | `octara-render` ↔ `octara-gpu` | Cross-platform `wgpu` micro-pipelines (`GlyphPipeline`, `Quad`, `Shadow`, `Image`, Early-Z) | Custom GPU compute backends, wide-gamut / HDR compositors, specialized vector/3D canvas pipelines |
| **SPI-2** | `MediaResolver`, `MediaCacheBackend`, `VideoSurfaceProvider` | `octara` / `octara-runtime` | `ImageSource`, bounded RAM caches, opt-in Native disk cache, Web `<img>`/`<video>`, AVKit and Native poster fallback | Custom authenticated CDNs, encrypted caches, hardware-accelerated zero-copy video decoders, custom document/PDF renderers |
| **SPI-3** | `BlockSlotRenderer`, `CodeHighlighter`, `MathRenderer`, `EditorBackend` | `octara-markdown` / `octara-tree` | `octara-markdown` core, standard syntax highlight, vector LaTeX math, `EditorRope` + `MarkdownComposer` | Custom interactive AI artifacts, diagram/chart engines (Mermaid/SVG/GPU charts), specialized IDE code/diff editors, domain-specific widgets |
| **SPI-4** | `AuditRule`, `InspectTransport`, `TraceRecorder` | `octara-inspect` | Node observation, action injection, screenshots, base deterministic simulation testing (DST) | Custom design-system/accessibility audit packs, binary remote bridges, telemetry/session replay recorders |
| **SPI-5** | `ReleaseProvider`, `SecurityPolicy` | `octara-release` / `octara-runtime` | Ed25519 signature verification & declarative `Transaction` packet compiler | Custom update servers, staged rollout / canary delivery, custom security & compliance policies |
| **SPI-6** | `OsHost`, `OsWindow` *(already exists)* | `octara-host-native` | Clean `winit` + standard macOS / iOS / Linux / Windows host backends | Platform-specific shell integrations (menubar panels, global overlays, OS vibrancy, native keychain/SSO) |

---

### SPI-2 Media registration

Use `BuildCx::provide::<Arc<dyn MediaResolver>>` for logical URI resolution at a view scope. Register a persistent response store with `AppDefinition::with_media_cache_backend` and a custom native video slot provider with `AppDefinition::with_video_surface_provider`. Returning `None` from either resolver or video provider selects the built-in behavior. The persistent cache receives public HTTP bodies and validators only. The runtime enforces source policy before giving the Native loader a cache backend. See [Async Media](10-ai-document-media-and-markdown.md#34-viewport-aware-async-media-zero-cls-probing--3-tier-image-cache).

### SPI-3 Editor registration

Provide an `Arc<dyn EditorBackend>` through `BuildCx::provide` for a subtree, or set a fallback with `TextInput::editor_backend`. The backend receives the nonsecure editor's logical text and returns optional `EditorRichText` in UTF-8 byte coordinates. Returning `None` lets the next backend or the input's authored rich text supply the presentation. `MarkdownComposer` supplies a fallback for Markdown styling and atomic completion links. The retained editor settles the value, IME composition, selection, and undo history. See [Rich Composer](10-ai-document-media-and-markdown.md#52-ai-rich-composer-feature--composer).

## 3. SPI Implementation Discipline (Starting in `0.0.5 Wave 1`)

1. **Strict Downward DAG**: All 6 SPI traits live in their canonical lower crates (`octara-model`, `octara-render`, `octara-runtime`, `octara`, `octara-markdown`, `octara-inspect`, `octara-release`, `octara-host-native`), enforced by `tools/check_layers.py`.
2. **Scoped vs. Engine-Wide Registration**:
   - Engine-wide backends (`SurfaceRenderer`, `MediaCacheBackend`, `VideoSurfaceProvider`, `InspectTransport`, `ReleaseProvider`, `OsHost`) attach at `App` construction.
   - Subtree-scoped extensions (`MediaResolver`, `BlockSlotRenderer`, `CodeHighlighter`, `MathRenderer`, `EditorBackend`, `AuditRule`) attach via `BuildCx::provide::<Arc<dyn ...>>`, enabling per-window or per-pane composition.
3. **Graceful Fallback Contract**: Extension hooks that inspect dynamic content (`BlockSlotRenderer::render_slot -> Option<El>`, `CodeHighlighter::highlight -> Option<...>`) degrade cleanly to the built-in default on `None`.

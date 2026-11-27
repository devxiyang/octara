# 10 — Core Architecture Review, Unified Media / Inline Primitives & `octara-markdown` (`0.0.5`)

> **Design premise**: Zero historical baggage, zero backwards compatibility with `0.0.4`'s `octara-kit::markdown` or narrow single-purpose core slots.  
> **North star**: Make Octara the **best-in-class pure-Rust native + Web UI engine for AI-era applications** — unifying rich streaming documents, extensible interactive slots, native multimedia, vector LaTeX math, mention-aware Markdown composition, and the first wave of hardware-sympathetic core upgrades (`TableMask`, compact `GlyphInst`, Rope/Piece Table, viewport-aware media scheduling, and Aho-Corasick/SWAR scanning) without bloating `octara-model`, `octara-kit`, or default `wasm32`.

---

## 1. Whole-Architecture Review: What Holds Up vs. What Must Evolve

Before designing `octara-markdown`, we audited every core layer of Octara `0.0.4` (`octara-model`, `octara-tree`, `octara-runtime`, `octara-pipeline`, `octara-render`, `octara-host-native`, `octara-host-web`, and `tools/check_layers.py`) against AI-era document, multimedia, rich-composer, and hardware-efficiency requirements.

### 1.1 Existing Core Pillars That Already Fit Cleanly (Keep As-Is)

| Existing Mechanism | Current Location | Why It Already Fits `0.0.5` |
| --- | --- | --- |
| Sparse ECS `NodeCore (32 B)` + `SparseSecondaryMap` | `octara-tree/src/tables.rs` | New cold tables (e.g., `text_reveals`, `media_sources`) add **0 bytes** to non-participating nodes. |
| 5-Phase Turn Engine + Capability Contexts (`BuildCx`/`ReadCx`/`WriteCx`/`TaskCx`) + All-`Copy` Handles | `octara-runtime` | `TextBuffer` delta cursors, `TaskCx` async image decoding, and reactive `LinkPillContent` maps slot in cleanly with compile-time safety. |
| `NativeSlot { Overlay, Underlay }` + `NativePlacement` (`clip`, `radius`, `covered` pointer routing) | `octara-model/src/native.rs` | Native `<video>` (`AVPlayerView` / `AVPlayerViewController`) plugs directly into `NativeSlot` with zero new compositor architecture. |
| Vector `Path` / `CanvasOp` / `PathPaint` | `octara-model/src/{path,canvas,paint}.rs` | Pure-Rust LaTeX Math emits crisp DPI-independent vector primitives (`CanvasOp` + `GlyphRun`) directly into `octara-render` (Native GPU + CPU Oracle + Web), **zero WebView** and **zero blurry PNG math**. |
| `LinkSlot::navigation_href` scheme allowlist | `octara-tree/src/tables.rs` | Safely restricts default OS open to `http`/`https`/`mailto`/`tel` while delivering custom schemes (`user://`, `cite://`, `attachment://`, `octara://`) to `on_link`. |

### 1.2 Nine Core Architectural Upgrades in `0.0.5` (And Why They Belong in Core)

Rather than hacking around core limitations inside a Markdown crate, **9 foundational gaps** are resolved cleanly in Octara's core layers so **every Octara application** benefits:

| # | Current Limitation in `0.0.4` | Core Upgrade in `0.0.5` | Affected Layers |
| --- | --- | --- | --- |
| **C1** | `octara-pipeline/src/text/padding.rs` uses Parley `InlineBox` only as a private `height: 0.0` spacer hack for `RichSpan::padding_x`; `TextSlot` has no concept of inline embedded objects. | **Unified `InlineObject` Pipeline**: Delete `padding.rs` and replace it with first-class `InlineAttachment` in `TextSlot` (inline images, vector icons/math `CanvasOp`, custom pills, spoilers, horizontal spacers) with `InlineAlign::{Baseline, CapHeightCenter, Middle, Bottom}` and logical copy/a11y fallback text. | `octara-model`, `octara-tree`, `octara-pipeline`, `octara-host-web`, `octara` |
| **C2** | `EditorSlot` (`octara-tree/src/editor.rs`) has **no `RichSpan` decorations**, **no atomic token ranges**, and copies the entire flat `String` (`O(N)`) on every keystroke & UTF-8↔UTF-16 IME query. | **Rich `EditorSlot` (`Rope` + `decorations` + `atoms`)**: Upgrade editor storage to a **Piece Table / Rope** indexed by `(utf8_bytes, utf16_units, line_breaks)` (`O(log N)` edits, `O(log N)` UTF-8↔UTF-16 IME conversion, `O(1)` CoW snapshots for background highlight/parse), plus cold `decorations`, `attachments`, and `atoms: Arc<[AtomicRange]>`. `EditorSession` enforces caret snap, single-Backspace deletion, and atomic undo; `TextEngine` reuses the exact same `Span` shaping path; Web stays on `<input>`/`<textarea>` for plain inputs and activates rich projection only when `decorations`/`atoms` are present. | `octara-model`, `octara-tree`, `octara-pipeline`, `octara-host-web`, `octara` |
| **C3** | `Image` (`octara-model/src/image.rs`) is sync-only raw `RGBA8` pixels; Web forces every `NodeKind::Image` through `<canvas>`. No URI resolution, auth headers, caching, priority, or CLS prevention. | **Viewport-Aware Async Media Pipeline**: Add `ImageSource::{Pixels, Uri, Encoded, Resolved}` + pluggable `MediaResolver` & 3-tier `ImageCache` (downsampled memory originals + processed variants + optional disk cache that auto-bypasses when custom auth headers exist) + in-flight request dedup + **viewport-priority queue (P0 visible > P1 prefetch; abort P2 scrolled-out requests)** + **progressive header dimension probing (zero CLS jump)** + reactive `ImageState`. Zero HTTP/codec deps in `octara-model` or default `wasm32`; Web `Uri` without auth renders via zero-copy `<img>`. | `octara-model`, `octara-tree`, `octara-runtime`, `octara-host-native`, `octara-host-web`, `octara` |
| **C4** | `OverlaySlot` splits `anchor: Option<(NodeId, Placement)>` and `point: Option<Point>`; it cannot anchor to a live text caret or selection rect despite `TextEngine::caret` already computing viewport rects. | **Unified `AnchorTarget`**: Replace `anchor` + `point` on `OverlaySlot` with `AnchorTarget::{Node(NodeId), Point(Point), Rect(Rect), EditorCaret(NodeId), TextSelection(NodeId)}` + expose `caret_rect` and `selection_rect` on `HostEngine`. Enables zero-lag `@mention`/`/command` popups and floating AI selection toolbars on Native & Web with automatic viewport flip. | `octara-model`, `octara-tree`, `octara-runtime`, `octara-pipeline`, `octara-host-web`, `octara` |
| **C5** | `GlyphRun` has one flat `color: Color`, and every glyph uploads a bloated **156-byte `Inst`** (`112 B` of zeros!) with `156 B` of `VsOut` interpolators. | **Paint-Time `TextReveal` + Compact `GlyphInst` (`24–32 B`) Micro-Pipeline**: Introduce a dedicated `GlyphPipeline` using **Vertex Pulling (`VsOut` = 4 B `inst_id`)** and compact **`24–32 B` `GlyphInst`** carrying packed per-glyph RGBA8 color/alpha (cutting glyph bandwidth **5×** and unlocking free **120fps zero-relayout** `TextReveal::{Fade, Gradient}` streaming frontier sampled in `MotionEngine` + `PaintCompiler` without re-shaping Parley or touching Taffy!). | `octara-model`, `octara-tree`, `octara-pipeline`, `octara-render`, `octara-host-web`, `octara` |
| **C6** | `NodeCore` (`32 B`) has no presence mask for the 18 cold `SparseSecondaryMap` side tables in `Tables`, forcing repeated cache-missing `get()` lookups that return `None`. | **`NodeCore::present` 32-bit `TableMask`**: Embed a 32-bit component presence bitmask directly inside `NodeCore` (staying strictly `<= 48 B`, single cache line). Every style, layout, paint, and a11y pass tests presence in 1 L1 CPU cycle before touching any secondary table. | `octara-tree` |
| **C7** | Selection copy only has one `copy_selection_with` hook; `HandlerKind::Link` only fires primary click; no unified selection or link context-menu protocol. | **Extensible Selection & Link Interaction Protocol**: Add `ContextMenuItem` / `on_context_menu` on `SELECT_REGION`, `TextInput`, `Link`, and `Image`, multi-format `CopyFormat::{Plain, Markdown, Custom}`, and rich `LinkEvent { href, text, kind: Press | SecondaryPress | Hover(bool), rect }`. | `octara-model`, `octara-tree`, `octara-runtime`, `octara-host-native`, `octara-host-web`, `octara` |
| **C8** | High-frequency SSE / token streams arriving from background tasks can wake multiple redundant turns per frame or jitter across frames. | **Lock-Free SPSC Stream Coalescing (`TextStreamSink`)**: Provide a cache-line-isolated (`#[repr(align(128))]`) lock-free SPSC ring buffer feeding `TextBuffer`, coalescing all chunks arriving within one frame into a single `Turn` commit with zero lock contention. | `octara-runtime`, `octara` |
| **C9** | Markdown currently lives inside `octara-kit::markdown`, coupling component-kit consumers to Markdown growth and mixing styling presets with a document engine. | **Dedicated Layer-2 Crate `octara-markdown`**: Promote Markdown into `crates/octara-markdown` depending **only** on `{"octara", "octara-headless", "octara-icons"}` (enforced by `tools/check_layers.py` + `AUTHORING_PACKAGES`) with Cargo feature gates (`highlight`, `math`, `media`, `composer`). **Delete `octara-kit::markdown` completely** (zero duplicate engines, zero compatibility shims). | `tools/check_layers.py`, `crates/octara-markdown`, `crates/octara-kit`, `apps/chat`, `examples` |

---

## 2. Crate Topology & Cargo Feature Matrix

### 2.1 Updated Layer 2 DAG (`tools/check_layers.py`)

```python
"octara-headless": {"octara"},
"octara-kit": {"octara", "octara-headless", "octara-icons"},
"octara-markdown": {"octara", "octara-headless", "octara-icons"},
"octara-chat": {"octara", "octara-icons", "octara-kit", "octara-markdown", "octara-host-native"},
"octara-examples": {
    "octara", "octara-fonts", "octara-icons",
    "octara-kit", "octara-markdown", "octara-pipeline", "octara-render", "octara-gpu",
    "octara-host-native", "octara-inspect", "octara-release",
},
```

**Hard Invariants**:
1. `"octara-markdown"` is added to `AUTHORING_PACKAGES` in `tools/check_layers.py`: **zero imports of `octara-model`, `octara-tree`, or `octara-runtime` internals**. Every core primitive it relies on is a first-class public API on `octara`.
2. **Zero `Rc::new`, zero `RefCell`, zero `todo!()`** in `octara-headless`, `octara-kit`, and `octara-markdown`.
3. **Zero heavy deps in `octara-model` or default `octara-host-web` (`wasm32-unknown-unknown`)**.

### 2.2 `octara-markdown` Cargo Features

| Feature | Default | Included Capabilities | Extra Dependencies |
| --- | :---: | --- | --- |
| *(core)* | Yes | Lossless AST, `DocumentStore` + `FinishedBlocks` $O(1)$ sharing, `AstOverlay`, SWAR boundary scanner, `StreamingFilter` + `InlineRepair` + `StreamingPacer` + `StreamingEffects`, 6 `BlockSlot` types (including `Directive`), `CollectedBlocks`, Aho-Corasick `TextRecognizer` & two-layer `LinkPill`, `||spoiler||`, interactive tasks, Smart Copy, `MarkdownTheme` + `MarkdownRenderer` | Lightweight CommonMark/GFM parser + `aho-corasick`/`memchr` (`wasm32`-tiny) |
| `highlight` | Opt-in | Built-in `CodeHighlighter` trait + trimmable language packs | Syntax engine behind feature gate |
| `media` | Opt-in | Async block/inline `Image` rendering via `octara`'s viewport-aware `ImageSource` + `MediaResolver` + lightbox index, plus `<video>` allowlist via `NativeSlot` / Web `<video>` | Uses `octara` media service (zero HTTP client forced into library) |
| `math` | Opt-in | Pure-Rust LaTeX parser & vector layout (`$inline$` → `InlineObject::Vector`, `$$block$$` → `BlockSlot::Math`), instance-deduped `on_math_error` fallback | Pure-Rust TeX layout crate (zero WebView, works on Native + Wasm) |
| `composer` | Opt-in | `MarkdownComposer`: `@`/`#`/`/` triggers, `AnchorTarget::EditorCaret` popup, atomic mention deletion, live inline Markdown styling, bidirectional Markdown roundtrip | Zero extra deps (built purely on `octara` + `octara-headless`) |

---

## 3. Core Engine Primitives Specification (Wave 1)

### 3.1 `NodeCore::present` (`TableMask`)

In `octara-tree/src/lib.rs`:
- Add `pub present: TableMask` (`u32` bitflags covering all cold `SparseSecondaryMap` tables in `Tables`, including `TEXT_REVEAL` and `MEDIA`).
- Assert `size_of::<NodeCore>() <= 48` at compile time (`const _: () = assert!(size_of::<NodeCore>() <= 48);`).
- All hot table getters (`tree.overlay(node)`, `tree.scroll(node)`, `tree.paint(node)`, `tree.text_reveal(node)`, etc.) short-circuit on `core.present.contains(...)` before probing any `SparseSecondaryMap`.

### 3.2 Unified `InlineAttachment` (Replacing `Padding` Hack)

In `octara-model::text`:

```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InlineAlign {
    #[default]
    Baseline,
    /// Vertically centered against the line's cap-height (ideal for inline images, pill icons, math).
    CapHeightCenter,
    Middle,
    Bottom,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InlineContent {
    /// Pure horizontal advance (replaces the ad-hoc `Padding` spacer).
    Spacer { width: f32 },
    /// Embedded inline image (block vs inline auto-detection in Markdown).
    Image { image: Image, fit: ObjectFit, radius: f32 },
    /// Embedded Lucide/custom vector icon (used inside Link Pills and inline badges).
    Icon { icon: NodeIcon, color: Option<ColorValue> },
    /// DPI-independent vector drawing (used for inline LaTeX math, custom badges, mini sparklines).
    Vector { ops: Arc<[CanvasOp]> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct InlineAttachment {
    /// Logical UTF-8 range in `TextSlot::text` represented by this attachment.
    /// Empty range `pos..pos` inserts at `pos`; non-empty range replaces the visual glyphs
    /// while preserving the underlying UTF-8 slice for copy and accessibility!
    pub range: Range<usize>,
    pub size: Size,
    /// Distance in logical pixels from the top edge to the intrinsic baseline.
    /// With `InlineAlign::Baseline`, `None` aligns the bottom edge.
    pub baseline: Option<f32>,
    pub align: InlineAlign,
    /// Distance from the box's top to its baseline. Defaults to the bottom edge.
    pub baseline: Option<f32>,
    pub content: InlineContent,
}
```

- `octara-pipeline/src/text/padding.rs` is **deleted and replaced** by `octara-pipeline/src/text/inline.rs`.
- Non-empty `attachment.range` hides replaced source bytes during Parley shaping (inserting one `\u{fffc}` box) while `Document::byte()` maps selection/hit-test/copy back to the full logical UTF-8 slice — copying inline LaTeX `$\alpha + \beta$` or a renamed Link Pill preserves the exact source text.
- `octara-host-web` projects `InlineAttachment` as inline-block `<span>` / `<img>` / `<svg>` matching `InlineAlign`.

### 3.3 Rich `EditorSlot` (`Rope` + `decorations` + `atoms`)

In `octara-model::text` & `octara-tree::editor`:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AtomicRange {
    /// Logical UTF-8 byte range treated as one indivisible unit.
    pub range: Range<usize>,
    /// Semantic token tag (e.g., "mention", "channel", "command", "file").
    pub kind: Arc<str>,
}
```

- **Piece Table / Rope Backing (`EditorRope`)**:
  - B-tree chunks aggregating `(utf8_bytes: usize, utf16_units: usize, line_breaks: usize)` per internal node.
  - $O(\log N)$ insert/delete; $O(\log N)$ UTF-8 ↔ UTF-16 ↔ `(line, col)` coordinate conversion for macOS `NSTextInputClient` and Web DOM; $O(1)$ Copy-on-Write snapshots for background syntax highlighting or Markdown parsing; volatile zero-wipe preserved on `secure` editors.
- **Cold Rich Fields**: `decorations: Arc<[(Range<usize>, RichSpan)]>`, `attachments: Arc<[InlineAttachment]>`, `atoms: Arc<[AtomicRange]>`.
- **Single-Source Atomic Invariants in `EditorSession`**:
  1. Caret/click strictly inside an `AtomicRange` snaps to `atom.start` or `atom.end`.
  2. Collapsed Backspace at `atom.end` or Delete at `atom.start` removes `atom.start..atom.end` in one atomic `UndoHistory` step.
  3. `octara-pipeline/src/text` feeds `decorations` + `attachments` into the exact same `Span` + `InlineBox` path as `TextSlot` (auto-disabled when `secure`).
  4. `octara-host-web` keeps fast `<input>`/`<textarea>` when plain, switching to managed `contenteditable` only when `decorations`/`atoms`/`attachments` are non-empty.

### 3.4 Viewport-Aware Async Media, Zero-CLS Probing & 3-Tier Image Cache

In `octara-model::image` (zero heavy deps — pure value types):

```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CachePolicy {
    /// Memory + disk cache allowed (auto-downgraded to `MemoryOnly` if `headers` is non-empty).
    #[default]
    Standard,
    /// Memory caches only; never write to disk (mandatory for authenticated / private media).
    MemoryOnly,
    /// Bypass all caches and reload.
    NoCache,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ImageSource {
    Pixels(Image),
    Uri(Arc<str>),
    Encoded { bytes: Arc<[u8]>, mime: Option<Arc<str>> },
    Resolved {
        uri: Arc<str>,
        headers: Arc<[(Arc<str>, Arc<str>)]>,
        cache: CachePolicy,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ImageState {
    #[default]
    Idle,
    /// Dimensions may already be known from progressive header probing before full decode!
    Loading { probed_size: Option<(u32, u32)> },
    Ready { natural_size: (u32, u32) },
    Failed(Arc<str>),
}
```

Applications author `image_source(source)` and can observe its `ImageState` with `El::observe_image_state`. `image(&pixels)` remains the direct RGBA8 path. Provide an `Arc<dyn MediaResolver>` in a `BuildCx` scope to resolve logical URIs to signed URLs, custom headers or bytes while retaining the original document text.

- **Viewport scheduling**. The app shares in-flight jobs across windows by source, including the complete header list and effective cache policy. Up to four jobs run at once. Visible images take priority over the one-viewport prefetch band, including at nested scrollports. Visible images without dimensions enter the queue at their layout origin. Leaving the band cancels work when the last consumer leaves. A newly visible request can preempt speculative work.
- **Progressive dimensions**. Managed Web and Native requests probe up to 16 KiB of PNG, JPEG, WebP, AVIF or GIF and report dimensions before decode completes when those bytes identify the size. Layout retains known dimensions while a request is idle after cancellation and uses the natural size after decode. Matching dimensions keep layout stable from probe to readiness. Authors can set both dimensions to reserve space before the first bytes arrive.
- **Memory and persistent caches**. The app holds 20 MiB of decoded originals and 30 MiB of target-size and fit variants in LRU caches. Native workers decode originals and downsample oversized variants. A resized image keeps displaying its current pixels while the worker prepares a variant from the cached original. Set an optional `MediaCacheBackend` on `AppDefinition` to persist eligible HTTP response bodies. The Native host offers `DiskMediaCache::new(directory)` with a 100 MiB default and honors HTTP freshness, ETag validation and stale-while-revalidate. Header-bearing sources use `MemoryOnly` unless they explicitly request `NoCache`. Both policies bypass the persistent tier and `NoCache` also bypasses the two RAM tiers. Use `MemoryOnly` for private URLs whose credentials are carried outside the headers.
- **Host decoding**. Web plain URIs use the browser image decoder and a native `<img loading="lazy" decoding="async">`. The browser owns HTTP caching and reports natural dimensions through the decoder. Web encoded and resolved sources use Blob decoding and canvas, with abortable Fetch for resolved URLs. Native ships PNG, JPEG, WebP, and GIF decoding, plus system ImageIO AVIF decoding on macOS and iOS. Linux and Windows can supply AVIF pixels through a custom resolver or loader.
- **Video**. `video(VideoSource)` uses a native slot. Web uses `<video>` with inline playback and optional poster. macOS uses AVPlayerView and iOS uses AVPlayerViewController. Linux and Windows display the authored poster. `VideoSurfaceProvider` can supply a custom slot kind and properties at application startup.

### 3.5 Generalized `AnchorTarget` on `OverlaySlot`

Replace the separate `anchor` + `point` fields in `octara-tree/src/tables.rs` with:

```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AnchorTarget {
    Node(NodeId),
    Point(Point),
    Rect(Rect),
    /// Tracks the live viewport-space caret rect of a `TextInput` node.
    EditorCaret(NodeId),
    /// Tracks the live viewport-space bounding rect of a selection in `SELECT_REGION` or `TextInput`.
    TextSelection(NodeId),
}
```

- `HostEngine` exposes `caret_rect(node)` and `selection_rect(node)`.
- Enables zero-lag `@mention`/`/command` popups and floating AI selection toolbars on Native & Web with automatic viewport flip.

### 3.6 Paint-Time `TextReveal` + Compact `GlyphInst` (`24–32 B`) Micro-Pipeline

In `octara-model::motion` & `octara-render`:

```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TextRevealKind {
    Fade { duration_secs: f32 },
    Gradient { trail_bytes: u32, tint: Option<ColorValue>, glide_bytes_per_sec: f32 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextReveal {
    pub kind: TextRevealKind,
    pub revealed_bytes: usize,
    pub finished: bool,
}
```

- **Dedicated `GlyphPipeline` (`octara-render`)**:
  - Splits text mask & color glyphs out of the 156-byte uber-shader into a lightweight **`GlyphInst` (`24–32 B`)** stored in a read-only `StorageBuffer` (`pos: [f32; 2]`, `size_uv: [u16; 4]`, `color_rgba8: u32`, `clip_id_flags: u32`).
  - Uses **Vertex Pulling** (`VsOut` = `@builtin(position)` + `inst_id: u32` = **4 bytes** instead of 156 bytes — **39× reduction** in TBDR parameter buffer bandwidth, isolating glyph VGPR occupancy from `shadow_alpha`'s `erf()`/`exp()` loop!).
  - **Free 120fps `TextReveal`**: `MotionEngine` + `PaintCompiler` sample `TextReveal` at display timestamp `now` and write per-glyph modulated `color_rgba8` directly for the trailing frontier glyphs — **zero Parley re-shaping, zero Taffy relayout**, deterministic in CPU Oracle & DST, and auto-disabled when `SystemPreferences::reduced_motion` is active.

### 3.7 Lock-Free SPSC Stream Coalescing (`TextStreamSink`) & Context Menu / `LinkEvent`

- **`TextStreamSink`**: `#[repr(align(128))]` cache-line-isolated lock-free SPSC ring buffer between network/task threads and `TextBuffer`, coalescing bursty SSE chunks into one Turn per frame.
- **`LinkEvent { href, text, kind: Press | SecondaryPress | Hover(bool), rect }`** + **`on_context_menu`** + **`CopyFormat::{Plain, Markdown, Custom}`** on `SELECT_REGION`, `TextInput`, `Link`, and `Image`.

---

## 4. `octara-markdown` Crate Architecture (Wave 2)

`crates/octara-markdown` replaces `octara-kit::markdown` completely with a clean 11-module architecture:

```text
crates/octara-markdown/src/
├── ast.rs          # Lossless AST, Block/Inline/Directive/Math/Video/Spoiler, CollectedBlocks
├── store.rs        # DocumentStore, FinishedBlocks O(1) structural sharing, AstOverlay
├── stream.rs       # SWAR boundary scan, StreamingFilter, InlineRepair, Adaptive Jitter StreamingPacer, StreamingEffects
├── slots.rs        # BlockSlot (6 kinds), SlotId (FNV-1a content-addressed), BlockPlacement, BlockSlotRenderer
├── pills.rs        # Aho-Corasick TextRecognizer & LinkVariantRule (static) + LinkPillContent (reactive)
├── theme.rs        # MarkdownTheme presets (document / chat / compact / custom)
├── renderer.rs     # trait MarkdownRenderer + fluent MarkdownConfig + default block builders
├── highlight.rs    # #[cfg(feature = "highlight")] pluggable CodeHighlighter + trimmable language packs
├── math.rs         # #[cfg(feature = "math")] pure-Rust LaTeX -> CanvasOp/GlyphRun + error dedup
├── media.rs        # #[cfg(feature = "media")] async Image + Lightbox + <video> NativeSlot/Web
├── composer.rs     # #[cfg(feature = "composer")] MarkdownComposer (@/#// triggers + roundtrip)
└── lib.rs          # Public API: markdown(), markdown_with(), markdown_stream(), MarkdownComposer
```

### 4.1 Immutable `DocumentStore` + `FinishedBlocks` + `AstOverlay`
- `ParsedDocument { blocks: Arc<[Arc<Block>]>, starts: Arc<[usize]>, collected: CollectedBlocks }`.
- **SWAR / `memchr` Prefix Scanner (`finished_blocks_end`)**: scans backwards at >10 GB/s to the last safe top-level block boundary (guarding against `[label]: url` reference definitions via `may_define_link_reference`). Every streaming tick shares prefix `Arc<Block>` pointers in $O(1)$ (`shared_leading_blocks`) and parses only the unfinished tail.
- **Zero-Mutation `AstOverlay`**: attaches active `SlotId`s, image `occurrence_id`s, and Aho-Corasick `TextRecognizer` links during rendering without mutating cached AST trees.

### 4.2 Three-Stage Streaming Pipeline
1. **Stage 1 — `StreamingFilter`**: per-block `Hidden | Progressive` policy for `tables` (synthetic delimiter row + incomplete row trim), `code_blocks` (`is_complete: false` until closing fence), `media` (suppresses half-typed `![alt](htt`), `math` (`$$`), and `directives` (`:::think` streams live!).
2. **Stage 2 — Tail-Only `InlineRepair`**: repairs unclosed `**`, `*`, `~~`, `` ` ``, `$`, `||`, `[label](url` strictly after `finished_blocks_end()`.
3. **Stage 3 — Adaptive Jitter Buffer `StreamingPacer` + GPU `StreamingEffects`**: EWMA arrival-gap smoothing (max 1.0 s lag), holds partial words & partial table rows (`|` line start), drains in `<= 200 ms` on `finish()`, and feeds `TextReveal` straight into the 24 B `GlyphPipeline`.

### 4.3 Six First-Class Replaceable `BlockSlot`s

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlockPlacement { Root, Blockquote, Callout(CalloutKind) }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotId(pub Arc<str>); // "<kind>:<fnv1a64>:<occurrence>"

#[derive(Clone, Debug, PartialEq)]
pub enum BlockSlot {
    Code { id: SlotId, placement: BlockPlacement, info: CodeInfo, code: ReadSignal<Arc<str>>, is_complete: ReadSignal<bool> },
    StandaloneLink { id: SlotId, placement: BlockPlacement, url: Arc<str>, label: Arc<str> },
    Image { id: SlotId, placement: BlockPlacement, url: Arc<str>, alt: Arc<str>, title: Option<Arc<str>>, occurrence_index: usize },
    Video { id: SlotId, placement: BlockPlacement, src: Arc<str>, poster: Option<Arc<str>> },
    Math { id: SlotId, placement: BlockPlacement, tex: ReadSignal<Arc<str>>, is_complete: ReadSignal<bool> },
    Directive { id: SlotId, placement: BlockPlacement, name: Arc<str>, attrs: Arc<[(Arc<str>, Arc<str>)]>, body: ReadSignal<Arc<str>>, is_complete: ReadSignal<bool> },
}
```

- `Code`, `Math`, and `Directive` hash **only their structural header** + occurrence index so mounted custom widgets update in-place across streaming and fence closure without unmounting or losing state.
- `render_slot(&self, cx: &mut BuildCx, slot: &BlockSlot) -> Option<El>` falls back cleanly on `None`.
- `CollectedBlocks { images, links, slots }` exposes document-order indices for swipe-gallery lightboxes, auth prefetch, and link unfurling.

### 4.4 Aho-Corasick `TextRecognizer`, Two-Layer `LinkPill` & Spoilers
- **Two-Layer `LinkPill`**: static `LinkVariantRule` presentation (compiled into one single-pass Aho-Corasick/DFA matcher, longest-match-first) decoupled from reactive per-URL `LinkPillContent` (`{ label, icon, tint }`) so async metadata updates never invalidate document styles while copy & a11y preserve original URLs.
- **`TextRecognizer`**: single-pass $O(N)$ Aho-Corasick/DFA auto-linking (`src/main.rs:42`, `#128`, `@alice`) applied via `AstOverlay`.
- **Spoilers (`||secret||`)**: inline & block tap/keyboard-to-reveal.

---

## 5. Native Vector LaTeX Math & AI Rich Composer (Wave 3)

### 5.1 Native Pure-Rust LaTeX Math (`feature = "math"`)
- Inline formulas use a baseline-aligned vector attachment whose logical text includes the exact authored `$...$` source for selection and accessibility. Display formulas use `BlockSlot::Math` and center in a horizontal scroll area with the authored `$$...$$` source still selectable.
- The built-in typesetter covers fractions, scripts, roots, large operators and limits, Greek letters, scalable delimiters, matrices, cases, and text and font groups. It emits deterministic `CanvasOp` paths from the bundled STIX Two Math subset. The Native and CPU renderers use the same paths that Web projects as SVG. Theme color tokens resolve at paint time.
- Provide an `Arc<dyn MathRenderer>` through `BuildCx::provide` to change typesetting under a scope. `MarkdownRenderer::math_renderer` overrides that choice on a document, and `BlockSlotRenderer` can replace the display block. A parse or layout error leaves the TeX visible in monospace and calls `on_math_error` once per distinct failing formula in the mounted document.

### 5.2 AI Rich Composer (`feature = "composer"`)

`MarkdownComposer::new` owns a Markdown draft. `MarkdownComposer::bind` uses an existing draft binding, so authored value changes and submitted editor input use the same source. The default triggers are `@`, `#`, and `/`. Configure the minimum query length and support for spaces with `TriggerConfig`. `active_trigger()` reads the query and its UTF-8 range, which follows the retained caret and clears during text selection or IME composition.

`composer.editor` attaches a suggestion provider to an existing `TextInput`. Suggestions use a headless popover with `AnchorTarget::EditorCaret` and support pointer selection, arrow keys, Enter, and Escape. `Completion::action` removes the active query and sends its action key to the callback supplied by `editor_with_completion`.

The retained rope holds the original Markdown. Rich spans style emphasis, code, and links as the draft changes. Private links such as `[@Alice](user://u_1)` and `[#design](channel://c_4)` use inline attachments to hide their delimiters and destinations on screen. Each link occupies one atomic range, so a single Backspace removes it and one undo restores it. Read the exact draft with `composer.markdown()`.

---

## 6. Three-Wave Execution Plan, Deletion Checklist & Release Gates

### 6.1 Mandatory Zero-Baggage Deletion Checklist
- [ ] Delete `crates/octara-kit/src/markdown/` completely (remove re-exports from `crates/octara-kit/src/lib.rs`).
- [ ] Delete `crates/octara-pipeline/src/text/padding.rs` (superseded by unified `inline.rs` `InlineAttachment` pipeline).
- [ ] Remove legacy split `OverlaySlot::{anchor, point}` fields (replaced by `AnchorTarget`).
- [ ] Remove `KIND_MASK` / `KIND_COLOR` branches from the 156 B uber-shader once `GlyphPipeline` (`24–32 B` `GlyphInst` + Vertex Pulling) lands.
- [ ] Migrate `apps/chat` and `examples` directly to `octara-markdown` with zero legacy wrappers.

### 6.2 Wave Breakdown

| Wave | Scope | Mandatory Verification |
| --- | --- | --- |
| **Wave 1: Core Engine Primitives** | `NodeCore::present` (`TableMask`, `<= 48 B`), `InlineAttachment` (Parley + Web), Rich `EditorSlot` (`Rope` + UTF-8↔UTF-16 $O(\log N)$ + `decorations` + `atoms` + Web rich mode), Viewport-aware Async `ImageSource` + `MediaResolver` + Header Probing + 3-tier `ImageCache` + `"video"` `NativeSlot`/Web `<video>`, `AnchorTarget::{EditorCaret, TextSelection}`, Dedicated `GlyphPipeline` (`24–32 B` `GlyphInst` + Vertex Pulling) + Paint-time `TextReveal`, `TextStreamSink` (128 B aligned SPSC), `ContextMenuItem` + `LinkEvent` | Unit + integration tests across `octara-model`/`tree`/`runtime`/`pipeline`/`render`/`host-web`; compile-time `size_of::<NodeCore>() <= 48` & `size_of::<GlyphInst>() <= 32`; DST determinism for `TextReveal` & atomic Rope undo; `wasm32` dep check clean |
| **Wave 2: `octara-markdown` Core, Streaming, Slots & Media** | Create `crates/octara-markdown` + update `tools/check_layers.py`; delete `octara-kit::markdown`; `DocumentStore` + `FinishedBlocks` + `AstOverlay`; SWAR scanner + 3-stage streaming (`StreamingFilter`/`InlineRepair`/`StreamingPacer`/`StreamingEffects`); 6 `BlockSlot` types (incl. `Directive`); `CollectedBlocks`; Aho-Corasick `TextRecognizer` & two-layer `LinkPill` + `||spoiler||` + Smart Copy; `feature = "highlight"` & `feature = "media"` | Prefix-reuse $O(1)$ test (`shared_leading_blocks`), slot-identity stability across streaming & fence close, auth-header disk-cache bypass & scroll-out cancellation test, `check_layers.py` & zero `Rc`/`RefCell`/`todo!()` purity audit |
| **Wave 3: Native Math, Composer, Showcase & Release** | `feature = "math"` (vector LaTeX inline + block + error dedup), `feature = "composer"` (`MarkdownComposer` with `@`/`#`/`/` caret-anchored popup & bidirectional Markdown roundtrip), upgrade `apps/chat` & `examples` to showcase full AI chat experience, bump to `0.0.5`, regenerate `api.json.gz` & `skills/octara/` | All 8 release gates: `check_layers.py`, `cargo test --workspace`, `octara-cli --doc`, `clippy --all-targets -D warnings`, dual `wasm32` checks, `aarch64-apple-ios-sim`, `cargo publish --workspace --dry-run` |

Between waves, workspace tests validate temporary exports of the API catalog and skill against the live checkout. At release, regenerate the bundled catalog and skill. Run `OCTARA_VERIFY_RELEASE_API=1 cargo test -p octara-cli --test integration api_catalog_matches_checkout` to verify the checked-in catalog. The CI workflow for `main` checks both release artifacts.

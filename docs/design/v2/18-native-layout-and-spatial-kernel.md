# 18 — Native ECS Layout & Spatial Kernel (Zero-`TaffyTree` Architecture) (`0.0.10`)

> **Design premise**: Zero historical baggage, zero backwards compatibility with the legacy `TaffyTree` mirror-tree adapter.  
> **North star**: Bring `octara-pipeline` (layout, geometry projection, spatial hit-testing, and text measurement) up to the same silicon-sympathetic, sparse-ECS standard as `octara-tree`, `octara-runtime`, `octara-render`, and the zero-`winit` direct native hosts — grounded in CPU cache-line discipline, property-tree rigid-body separation, content-addressed memoization, lazy spatial materialization, Euler-tour O(1) queries, and 100% W3C Flexbox/Grid/Containment semantic parity with `octara-host-web`.

---

## 1. Architectural Diagnosis: Why the Legacy `TaffyTree` Adapter Must Be Deleted

In Octara v2, `octara-tree` (Sparse ECS `NodeCore <= 48 B`, `SecondaryMap`, `DirtyBitmap`, content-addressed `Arc<ResolvedStyle>`), `octara-runtime` (zero-`Rc` `TypedSlab` DAG), `octara-render` (Vertex Pulling + `DamageRect` + retained GPU buffers), and `octara-host-native` (zero-`winit` direct AppKit/UIKit/Wayland/X11) are all clean-sheet systems.

Only `octara-pipeline`'s layout and spatial projection layer retained a heavy shadow-tree adapter architecture, creating seven structural defects:

1. **Duplicate Mirror Tree & Destroyed Style Interning (`TaffyTree<NodeId>`)**:
   - `LayoutEngine` mirrors every `octara_tree::NodeId` into `taffy::TaffyTree<NodeId>` with a separate `SecondaryMap<NodeId, taffy::NodeId>`, duplicate `Vec<taffy::NodeId>` child arrays, and a per-node owned ~500 B `taffy::Style` copy (destroying `octara-tree`'s `Arc<ResolvedStyle>` interning).
   - `TaffyTree::mark_dirty_recursive` unconditionally dirties every ancestor up to `tree.root()`, making subtree containment architecturally impossible.
2. **Unconditional Full-Window Frame Walk (`collect_frames`)**:
   - Whenever any descendant anywhere in the window becomes layout-dirty, `LayoutEngine` runs layout from `tree.root()` and invokes `collect_frames(..., full = true)`, walking **every live node in the window** (e.g., 1,832 nodes on a single sidebar row hover), allocating a full-window `FxHashSet<NodeId>`, and scanning all entries for stale keys.
   - Every anchored overlay `apply_fit` triggers another root-level layout compute and another full-window `collect_frames`.
3. **Rigid-Body Translation Conflated with Local Deformation**:
   - `TextEngine`, `PaintCompiler`, and `AccessKitProjector` all subscribe to a single `layout.changed()` bitmap. When one variable-height row grows by 16 px (e.g., an AI streaming line wrap), all subsequent sibling rows and their hundreds of descendants suffer rigid-body translation `(0, +16 px)` with zero change to their own size, local coordinates, or line breaks — yet every descendant is flagged in `layout.changed()`, triggering redundant text re-projection, accessibility geometry updates, and paint key evaluation.
4. **Eager Full-Window `O(N)` Spatial Grid Rebuild & Heap Fragmentation (`FlatTree`)**:
   - `FlatTree::project` eagerly rebuilds every spatial hash grid (`FxHashMap<(i32, i32), Vec<u32>>` — hundreds of tiny heap `Vec`s) on every frame with any `layout.changed()`, even on pure streaming/animation/typing frames with **zero pointer events**.
   - `exact_hit` climbs `physical_parent` to root on every candidate checking ancestor clips instead of using O(1) precomputed clip state.
5. **Single-Slot Line-Break Thrashing & Per-Node-Only Measurement (`TextEngine`)**:
   - `Shape` stores a single `broken: Option<(Option<f32>, bool)>` slot and mutates `parley::Layout::break_all_lines(width)` in place. Nested Flex probes (`MaxContent` → `MinContent` → `Definite(w)`) repeatedly clobber and re-break the same shaped paragraph, and recycled virtual-list nodes cannot reuse measurements across `NodeId`s.
6. **Hash-Map Fragmentation Inside `LayoutEngine`**:
   - Despite `octara-tree`'s dense `SlotMap` ECS design, `LayoutEngine` maintains 10+ `FxHashMap<NodeId, T>` tables and allocates per-frame `FxHashSet<NodeId>` and `FxHashMap` undo maps.
7. **Missing Cross-Platform Containment Primitive**:
   - Neither `octara-model` nor `octara-host-web` exposes CSS-standard `Contain`, leaving authors with no declarative way to declare hard layout/size/paint containment boundaries consistently across Native and Web.

---

## 2. Core Architectural Decision: Delete `TaffyTree`, Keep Stateless Taffy Math Kernels

We **delete `taffy_tree` completely** from the workspace:

```toml
taffy = { version = "0.15.1", default-features = false, features = ["std", "flexbox", "grid", "content_size"] }
```

- **Zero `TaffyTree`**, zero `taffy::Style` copies, zero `taffy::NodeId` mapping tables, zero duplicate child `Vec`s.
- `taffy::NodeId` is a transparent `u64` wrapper (`impl From<u64> for taffy::NodeId`), giving a **zero-cost lossless bijection** with `octara_tree::NodeId` (`KeyData::as_ffi` / `KeyData::from_ffi`).
- Octara owns the entire layout kernel (`LayoutKernel`) in `crates/octara-pipeline/src/layout/` and implements Taffy's zero-copy low-level traits (`TraversePartialTree`, `LayoutPartialTree`, `LayoutContainingBlock`, `CacheTree`, `LayoutFlexboxContainer`, `LayoutGridContainer`) directly over `&octara_tree::Tree` + temperature-partitioned `SecondaryMap`s.
- Taffy is demoted to a **pure stateless W3C Flexbox / Grid / OOF math library**, guaranteeing 100% layout parity with browser CSS in `octara-host-web` while handing 100% control over memory layout, dirty propagation, `RelayoutBoundary`, fast-path dispatch, caching, and rollback to Octara.

---

## 3. Clean-Sheet Architecture

### 3.1 Silicon-Sympathetic Hot/Cold ECS `LayoutKernel` (`crates/octara-pipeline/src/layout/`)

To avoid cache-line pollution (combining a 288 B `taffy::Cache` with traversal metadata would inflate each slot to ~520 B — 5–9 cache lines per node), `LayoutKernel` partitions node state by access temperature across dense `SecondaryMap<NodeId, T>` tables:

1. **Hot Table — `LayoutMeta` (`8 bytes`, `#[repr(C)]`, 16 nodes per Apple Silicon 128 B cache line)**:
   ```rust
   bitflags! {
       #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
       pub(crate) struct LayoutFlags: u16 {
           const DIRTY             = 1 << 0;
           const SUBTREE_DIRTY     = 1 << 1;
           const BOUNDARY_IMPLICIT = 1 << 2;
           const BOUNDARY_EXPLICIT = 1 << 3;
           const HAS_ESCAPING_OOF  = 1 << 4;
           const LIVE              = 1 << 5;
           // Reserved seam for future visibility-gated offscreen layout (`content-visibility: auto`).
           const OFFSCREEN_SKIPPED = 1 << 6;
       }
   }

   #[repr(C)]
   #[derive(Clone, Copy, Default)]
   pub(crate) struct LayoutMeta {
       /// Monotonic wrapping epoch incremented on every layout pass; stamps nodes that actually recomputed.
       pub(crate) layout_epoch: u32,
       pub(crate) flags: LayoutFlags,
       /// Cached physical tree depth (O(1) priority ordering without parent-chain walks).
       pub(crate) depth: u16,
   }
   const _: () = assert!(std::mem::size_of::<LayoutMeta>() == 8);
   ```
   Upward dirty climbs, depth sorting, and epoch-pruned projection scans touch **only `LayoutMeta` (8 B/node)** and stay L1d-resident.

2. **Warm Tables — Geometry & Projection**:
   - `layouts: SecondaryMap<NodeId, taffy::Layout>` — unrounded computed box geometry.
   - `frames: DenseProjected<Frame>` — `SecondaryMap<NodeId, Frame>` + reusable rollback journal `Vec<(NodeId, Option<Frame>)>` guarded by a `DirtyBitmap` (zero per-frame `FxHashMap` allocations).
   - `Frame` also caches `nearest_rounded_clip: Option<NodeId>` so `FlatTree::exact_hit` avoids climbing the full ancestor chain when no ancestor has a rounded clip.

3. **Cold Compute Table — On-Demand `taffy::Cache`**:
   - `caches: SecondaryMap<NodeId, taffy::Cache>` — allocated **only** for nodes that actually invoke Taffy Flex/Grid/leaf caching (0 bytes for `Display::None` and O(1) static fixed-size leaves).

4. **Ultra-Sparse Cold Tables**:
   - `boundary_states: SecondaryMap<NodeId, BoundaryState>` (`last_input: taffy::LayoutInput`, `last_output: taffy::LayoutOutput`) — stored **only** on active `RelayoutBoundary` nodes (~5% of nodes).
   - `hoisted_oof: SecondaryMap<NodeId, SmallVec<[NodeId; 2]>>` — stored **only** on containing blocks that actually hoist `Absolute`/`Fixed` descendants (~1% of nodes; 0 bytes on 99% of nodes).
   - `scroll_anchors`, `links`, `placement`, `fits`, `natural_sizes`, `measurements`, `native_sizes` — sparse `SecondaryMap<NodeId, T>` (zero `FxHashMap<NodeId, _>`).

5. **Zero-Copy Style Adapter (`OctaraStyleRef<'a>`)**:
   - A tiny `Copy` struct holding `(&'a LayoutStyle, StyleOverride)` pointing straight into `octara-tree`'s content-addressed `Arc<ResolvedStyle>`.
   - Implements `CoreStyle`, `FlexboxContainerStyle`, `FlexboxItemStyle`, `GridContainerStyle`, `GridItemStyle`, and `OofItemStyle` with **zero `taffy::Style` copies** and a zero-allocation `GridTrackIter` for `grid_columns` / `grid_rows`.

6. **Direct Fast-Path Dispatch inside `LayoutPartialTree::compute_child_layout`**:
   1. Consult `caches` (`taffy::compute_cached_layout`).
   2. `Display::None` → `taffy::compute_hidden_layout` (O(1), no `taffy::Cache` entry needed).
   3. **Static Non-Measure Leaf** (no live children, not a text/editor/icon/image/media/native measure leaf): compute definite/resolved size, padding, and border directly in O(1) without entering Flexbox/Grid algorithms.
   4. **Measure Leaf** (`Text`, `TextInput`, `Icon`, `Image`, `Media`, `Native`): invoke `taffy::compute_leaf_layout` backed by `TextEngine`'s content-addressed probe cache.
   5. **Flex / Grid Container**: delegate to `taffy::compute_flexbox_layout` / `taffy::compute_grid_layout`, followed by `taffy::compute_oof_layout` only when OOF children exist.

---

### 3.2 Mathematically Sound Dual-Layer `RelayoutBoundary` & Cross-Platform `Contain`

#### Layer A — Explicit W3C `Contain` (Native + Web Parity)

Add standard CSS containment to `octara-model::style`, `octara-tree`, `octara`, `octara-pipeline`, and `octara-host-web`:

```rust
bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct Contain: u8 {
        const LAYOUT  = 1 << 0;
        const SIZE    = 1 << 1;
        const PAINT   = 1 << 2;
        const CONTENT = Self::LAYOUT.bits() | Self::PAINT.bits();
        const STRICT  = Self::LAYOUT.bits() | Self::SIZE.bits() | Self::PAINT.bits();
    }
}
```

- `LayoutStyle` gains `pub contain: Contain` (`Default = Contain::empty()`).
- `StyleProp` gains `StyleProp::Contain(Contain)`.
- `Styled` facade exposes `.contain(Contain)`, `.contain_content()`, and `.contain_strict()`.
- **Native semantics**:
  - `Contain::LAYOUT`: establishes a containing block for `Absolute` / `Fixed` descendants (preventing OOF escape) and isolates internal layout from ancestors' `scrollable_overflow_rect`.
  - `Contain::SIZE`: node's intrinsic size does not depend on descendants (treated as empty when sizing itself).
  - `Contain::PAINT`: clips descendants to the node's border/padding box (integrating cleanly with `PaintCompiler` and `FlatTree` culling).
  - `Contain::LAYOUT | Contain::SIZE` unconditionally establishes `BOUNDARY_EXPLICIT`.
- **Web semantics (`octara-host-web/src/css.rs`)**:
  - Emits standard CSS `contain: layout;`, `contain: layout size;`, `contain: strict;`, etc. so Blink, WebKit, and Gecko activate their native relayout and paint containment boundaries with **100% behavioral parity**.

#### Layer B — Zero-Config Automatic `RelayoutBoundary` (Strict Soundness Invariants)

Without requiring any `.contain(...)` annotation from application authors, `LayoutKernel` automatically sets `BOUNDARY_IMPLICIT` on node `B` after `PerformLayout` if and only if **all** of the following invariants hold:

1. **Parent-determined definite size**: `B` received `known_dimensions == (Some(w), Some(h))` (definite on both axes) from its parent during `PerformLayout`, OR `B`'s own `LayoutStyle` specifies definite `LayoutLength::Px` on both width and height with no content-dependent `min`/`max` override.
2. **Overflow isolated**: `B` clips overflow on both axes (`overflow_x != Visible && overflow_y != Visible`), so descendant geometry cannot alter any ancestor's `scrollable_overflow_rect`.
3. **No escaping out-of-flow descendants**: `!HAS_ESCAPING_OOF` (either `B` has no `Absolute`/`Fixed` descendants, or `B` itself is their containing block because `B.position != Static` or `B.contain.contains(Contain::LAYOUT)`).
4. **No own-style or parent-facing mutation**: On dirty marking, if `B` itself was modified (`delta.layout_dirty` contains `B` for a style/size/margin/position change, or `B`'s own children changed while `!B.contain.contains(Contain::SIZE)`), dirtying marks `B`'s parent too; if **only a strict descendant of `B`** changed, upward dirty propagation **stops at `B`** and enqueues `B` as a local relayout root.

#### Sound Escalation Protocol & Anchored Overlay Isolation

When executing local relayout roots in ascending `LayoutMeta::depth` order (skipping any root whose ancestor is already queued):
1. Run `compute_node_layout(root, saved_input)` directly on `root`.
2. Verify `new_output.size == old_output.size`, (for non-contained nodes) `new_output.content_size == old_output.content_size`, and `!HAS_ESCAPING_OOF`.
3. If any parent-observable quantity escaped, clear the boundary bit on `root`, mark `root`'s parent dirty up to the next enclosing boundary, and continue until the dirty frontier is empty.
4. **Anchored Overlay Isolation**: Direct anchored overlay children of `tree.overlay_root()` are unconditionally `BOUNDARY_EXPLICIT`. Calling `apply_fit(overlay)` dirties and recomputes **only that overlay subtree**, never `tree.root()` and never the main document tree.

---

### 3.3 Rigid-Body Translation vs. Local Deformation Separation + Epoch-Pruned Projection

Inspired by Chromium `cc` Property Trees and Wave B1's Scroll-Space Separation, `LayoutKernel` separates **local box deformation** from **pure rigid-body ancestor translation**:

- `LayoutKernel` publishes **three** distinct `DirtyBitmap` signals after each projection:
  1. `scroll_changed()`: scroll offset changed only (already isolated in `0.0.9`).
  2. `local_changed()`: node's own parent-relative `LayoutBox` (`rect`, `padding`, `border`), `local_client`, `local_content`, `clip_axes`, `scroll` range, or live visibility changed (or node created/destroyed).
  3. `changed()` (world bounds changed, superset of `local_changed()`): includes descendants whose own local box is 100% identical and whose `bounds` shifted solely because an ancestor moved by `(dx, dy)` (e.g., rows `#101..#159` shifting down by `+16 px` when row `#100` grows by one line).
- **Downstream Consumer Subscriptions**:
  - **`TextEngine::project`**: subscribes ONLY to **`layout.local_changed()`**! Pure rigid-body `(dx, dy)` translation never alters `local_content` width/height, line breaks, or local glyph origins — eliminating 100% of redundant text re-projections on shifted sibling rows.
  - **`AccessKitProjector::project`**: subscribes ONLY to **`layout.local_changed()`**! AccessKit already encodes node geometry as parent-relative `layout.geometry(node).rect` transforms, so only the shifted row container itself updates its `Transform` while its hundreds of descendants incur **0 accessibility updates**.
  - **`PaintCompiler`**: continues to use `layout.changed()` for absolute `paint_bounds` where needed, while leaving a clean seam to promote `RelayoutBoundary` containers into `SpatialTable` rigid-body translation wrappers in a follow-up wave.
- **Epoch-Pruned Local Frame Projection**:
  - Every layout pass increments `self.layout_epoch` (skipping `0` on wrap) and stamps `LayoutMeta::layout_epoch` only on nodes whose layout actually recomputed.
  - Frame projection walks **strictly from the relayout roots `R`** (plus `delta.scroll_dirty`, `delta.editor_dirty`, `delta.overlay_dirty`) — never from `tree.root()` unless `tree.root() ∈ R`.
  - Descends into children **only when** `child_ctx` changed (`origin`, `scroll_parent`, `clip_parent`, or `nearest_rounded_clip`), OR `child` has `layout_epoch == current_epoch` / `SUBTREE_DIRTY` / topology/visibility change. Clean subtrees are skipped in **O(1)**.
  - Stale frame cleanup is driven directly by `delta.retired` (`RetiredNode::Destroyed`) and visibility transitions — eliminating the full-window `visited: FxHashSet` allocation and full `frames` scan.

---

### 3.4 Lazy-Materialized Incremental `FlatTree` with O(1) Clip & Small-Container Fast Path (`crates/octara-pipeline/src/flat_tree.rs`)

Applying lazy materialization, small-container linear scans, and Euler-tour O(1) queries:

1. **Lazy Spatial Grid Materialization (0.00 ms on Non-Pointer Frames)**:
   - `FlatTree::project` updates `Topology` only when topology/z-index changed and records dirty geometry (`pending_patch: DirtyBitmap` or `needs_full_bounds: bool`), **without eagerly mutating any spatial hash grid during `project()`**!
   - Spatial grids are materialized/patched **lazily on demand** inside `hit()` / `scrollbar_hit()` (via interior mutability `RefCell<SpatialIndex>` on `FlatFrame`, strictly deterministic and rollback-safe).
   - Consequence: 120 Hz streaming Markdown, caret blink, animations, and keyboard typing frames with no pointer events pay **0.00 ms** for spatial indexing.
2. **Small-Container Grid Bypass (`SMALL_GRID_THRESHOLD = 24`)**:
   - Any scrollport or root containing `<= 24` indexed nodes stores a compact flat `Vec<u32>` instead of allocating an `FxHashMap<(i32, i32), Vec<u32>>` — eliminating hundreds of tiny heap allocations for popovers, toolbars, and small panes.
3. **Incremental O(changed) Cell Patching for Large Containers**:
   - Track per-flat-index `CellFootprint` in a parallel `Vec<CellFootprint>`.
   - When topology is unchanged and `pending_patch.len() * 4 < total_nodes`, update `bounds[idx]` and patch **only the changed indices** (`swap_remove` from old cells + `push` into new cells; exact hit order is already guaranteed by `top.is_none_or(|top| top < index)`).
4. **O(1) Euler-Tour Ancestry & Rounded-Clip Short-Circuit**:
   - Replace ancestor-chain walks in `exact_hit` with direct O(1) checks: test precomputed `layout.clip(node)` once, and walk `nearest_rounded_clip` links (empty for 99% of nodes, 0 parent-chain chasing!).
   - Use `FlatNode` Euler-tour intervals (`[idx, subtree_end)`) wherever ancestor-descendant containment checks are needed (`tin[a] <= tin[b] && tout[b] <= tout[a]`).
5. **Observability**: Expose `FlatTreeMetrics { grid_rebuilds, nodes_patched, lazy_skips }` so DST gates verify both lazy skipping and O(changed) patching.

---

### 3.5 Content-Addressed Non-Thrashing `TextEngine` Measurement (`crates/octara-pipeline/src/text/`)

Upgrade measurement caching from per-`NodeId` to **content-addressed `SharedShape` + per-`Document` probe cache**:

1. **Content-Addressed 4-Slot Probe Cache on `Shape` (Shared Across Nodes & Virtual-List Recycling)**:
   ```rust
   #[derive(Clone, Copy)]
   struct MeasureProbe {
       width_bits: Option<u32>, // None => unwrapped / MaxContent, Some(w.to_bits()) => constrained
       max_lines: usize,
       size: Size,
       lines: usize,
   }
   ```
   - Store `probes: SmallVec<[MeasureProbe; 4]>` (and a dedicated `unwrapped: Option<(Size, usize)>` slot) directly inside `Shape` (which is already content-addressed in `TextEngine::shapes` by `(text, ShapeStyle, spans)`!).
   - Because `Shape` is shared across identical strings/styles (buttons, badges, timestamps, repeated labels, and recycled `virtual_each` rows), **all identical text nodes and newly mounted rows reuse previously measured widths/heights in O(1) with zero `break_all_lines` calls**.
2. **`Wrap::None` / `Wrap::Truncate` O(1) Short-Circuit**:
   - Single-paragraph / multi-paragraph `Wrap::None` and `Wrap::Truncate` documents answer any width probe directly from the cached `unwrapped` slot (clamping `size.width.min(width)` for `Truncate`) without borrowing or mutating Parley line breaks.
3. **Warm Final-Width Preservation for `present()`**:
   - Because cached `MaxContent` / `MinContent` probes return immediately without calling `Shape::at(None)` / `Shape::at(Some(0.0))`, `Shape::broken` remains warm at the final definite layout width when `TextEngine::present()` runs — avoiding a redundant post-layout `break_all_lines`.
4. **100% BiDi / Rich-Span / Inline-Box Exactness**:
   - Note that `parley 0.13`'s `Layout::calculate_content_widths()` documents a known limitation on mixed-direction (BiDi) text. Using exact `break_all_lines` memoized inside content-addressed `Shape::probes` guarantees 100% exactness across BiDi, RTL, rich spans, inline pills, and attachments with zero drift from `present()`.
5. **Observability**: Track `TextWork::measure_probe_hits` and `TextWork::line_breaks` so regression tests prove zero thrashing.

---

### 3.6 `DenseProjected<T>` Rollback Journal & `check_layout_cache` Full-Tree Oracle

1. **SQLite-Style Dense Undo Journal (`DenseProjected<T>`)**:
   - Replace `Projected<T>`'s three `FxHashMap`s (`entries`, `undo`, `before_project`) with:
     - `entries: SecondaryMap<NodeId, T>`
     - `undo_log: Vec<(NodeId, Option<T>)>` + `undo_recorded: DirtyBitmap` (O(1) dedup, 0 heap allocation after warmup, sequential reverse replay on `rollback()`)
     - `stage_log: Vec<(NodeId, Option<T>)>` + `stage_recorded: DirtyBitmap` (for per-round `local_changed` / `changed` / `scroll_changed` diffing)
   - On `rollback()`, replay `undo_log` in reverse and union any nodes stamped with the rolled-back `layout_epoch` into `rollback_dirty: DirtyBitmap` so the next cycle re-projects cleanly.

2. **`check_layout_cache` Full-Tree Equivalence Oracle**:
   - Add `pub check_layout_cache: bool` to `NativeEngineConfig` (mirroring `check_paint_cache` and `check_a11y_cache`).
   - When `true`, after every incremental `LayoutKernel::project` pass, construct a cold full-tree `LayoutKernel` on the same `(tree, viewport)` and assert **exact equality** for every live node's `LayoutBox`, `bounds`, `content_bounds`, `clip`, `scroll_range`, `scroll_offset`, and sampled `FlatTree::hit` results.

---

## 4. Reserved Architectural Seams (Post-`0.0.10`, Zero Rework)

The data structures above cleanly support two high-leverage future optimizations without changing any core layout struct:
1. **Visibility-Gated Offscreen Layout (`content-visibility: auto`)**: Uses `LayoutFlags::OFFSCREEN_SKIPPED` on `Contain::STRICT` subtrees outside a ±1 viewport prefetch band.
2. **Non-Scroll Rigid-Body `SpatialTable` Translation Groups**: Elevates `RelayoutBoundary` containers into `PaintCompiler`'s existing `SpatialTable` translation table so sibling-growth shifts incur 0 `PaintNode` rebuilds and 16 B GPU uniform updates.

---

## 5. Hard Exit Gates & Measurable Performance Targets (`0.0.10` Wave D3)

1. **Zero `TaffyTree` Workspace-Wide**:
   - Root `Cargo.toml` removes `"taffy_tree"`; `rg "TaffyTree|taffy_tree" crates/ modules/` returns **0 hits**.
   - `assert_eq!(std::mem::size_of::<LayoutMeta>(), 8)` enforced at compile time.
   - `layout/style.rs` builds zero `taffy::Style` values and zero `Vec<GridTemplateComponent>`.
2. **160-Row Sidebar Hover Regression (Canonical Octoii Workload)**:
   - Hovering a row that toggles an inner action button (changing title width `240 → 212 pt` inside a fixed `44 pt` clipped row):
     - `LayoutWork::frames` drops from **`~1,832` → `<= 16`** (`> 100×` reduction);
     - `LayoutWork::boundaries_hit >= 1` and `LayoutWork::escalations == 0`;
     - `FlatTree` skips work during non-pointer frames and patches only changed nodes on hit (`nodes_patched <= 16`, **0 full grid rebuilds**);
     - `TextWork::measure_probe_hits > 0` with zero redundant `break_all_lines` thrashing;
     - `check_layout_cache = true` + `check_paint_cache = true` + `check_a11y_cache = true` pass with **0 diffs**.
3. **160-Row Variable-Height Growth Regression (Streaming AI Wrap Workload)**:
   - Growing row `#100` by `+16 px` shifts rows `#101..#159` cleanly:
     - `TextEngine` and `AccessKitProjector` report **0 re-projections / 0 updates** on the unaffected descendants of rows `#101..#159` (driven by `local_changed()` separation);
     - `check_layout_cache = true` passes with **0 diffs**.
4. **Anchored Overlay Isolation Test**:
   - Triggering `apply_fit` on a tooltip/popover inside a 160-row window recomputes **only the overlay subtree** (`LayoutWork::frames <= overlay_nodes + 2`, never re-walking the 160 rows).
5. **Cross-Platform `Contain` Parity**:
   - `Contain::LAYOUT | SIZE | PAINT` works identically in Native (`LayoutKernel` + `PaintCompiler`) and Web (`octara-host-web/src/css.rs` emits valid CSS `contain`), verified in both Native unit tests and Chromium DOM tests.
6. **Full Release Matrix**:
   - `python3 tools/check_layers.py`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo test -p octara-cli --doc`, DST performance ceilings, default + WebGPU Wasm checks, 4 module Wasm checks, `aarch64-apple-ios-sim`, and Linux target checks all pass.

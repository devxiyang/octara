# 18 — Native ECS Layout & Spatial Kernel (Zero-`TaffyTree` Architecture) (`0.0.10`)

> **Design premise**: Zero historical baggage, zero backwards compatibility with the legacy `TaffyTree` mirror-tree adapter.  
> **North star**: Bring `octara-pipeline` (layout, geometry projection, spatial hit-testing, and text measurement) up to the same silicon-sympathetic, sparse-ECS standard as `octara-tree`, `octara-runtime`, `octara-render`, and the zero-`winit` direct native hosts — while keeping 100% W3C Flexbox/Grid/Containment semantic parity with `octara-host-web`.

---

## 1. Architectural Diagnosis: Why the Legacy `TaffyTree` Adapter Must Be Deleted

In Octara v2, `octara-tree` (Sparse ECS `NodeCore <= 48 B`, `SecondaryMap`, `DirtyBitmap`, content-addressed `Arc<ResolvedStyle>`), `octara-runtime` (zero-`Rc` `TypedSlab` DAG), `octara-render` (Vertex Pulling + `DamageRect` + retained GPU buffers), and `octara-host-native` (zero-`winit` direct AppKit/UIKit/Wayland/X11) are all clean-sheet systems.

Only `octara-pipeline`'s layout and spatial projection layer retained a heavy shadow-tree adapter architecture, creating six structural defects:

1. **Duplicate Mirror Tree & Destroyed Style Interning (`TaffyTree<NodeId>`)**:
   - `LayoutEngine` mirrors every `octara_tree::NodeId` into `taffy::TaffyTree<NodeId>` with a separate `SecondaryMap<NodeId, taffy::NodeId>`, duplicate `Vec<taffy::NodeId>` child arrays, and a per-node owned ~500 B `taffy::Style` copy (destroying `octara-tree`'s `Arc<ResolvedStyle>` interning).
   - `TaffyTree::mark_dirty_recursive` unconditionally dirties every ancestor up to `tree.root()`, making subtree containment architecturally impossible.
2. **Unconditional Full-Window Frame Walk (`collect_frames`)**:
   - Whenever any descendant anywhere in the window becomes layout-dirty, `LayoutEngine` runs layout from `tree.root()` and invokes `collect_frames(..., full = true)`, walking **every live node in the window** (e.g., 1,832 nodes on a single sidebar row hover), allocating a full-window `FxHashSet<NodeId>`, and scanning all entries for stale keys.
   - Every anchored overlay `apply_fit` triggers another root-level layout compute and another full-window `collect_frames`.
3. **Full-Window `O(N)` Spatial Grid Rebuild (`FlatTree::rebuild_bounds`)**:
   - Whenever `!layout.changed().is_empty()` — even a single text node narrowing inside one row — `FlatTree` clears every spatial hash grid in the window and re-inserts all `N` nodes from scratch.
4. **Single-Slot Line-Break Thrashing (`TextEngine`)**:
   - `Shape` stores a single `broken: Option<(Option<f32>, bool)>` slot and mutates `parley::Layout::break_all_lines(width)` in place. Nested Flex probes (`MaxContent` → `MinContent` → `Definite(w)`) repeatedly clobber and re-break the same shaped paragraph.
5. **Hash-Map Fragmentation Inside `LayoutEngine`**:
   - Despite `octara-tree`'s dense `SlotMap` ECS design, `LayoutEngine` maintains 10+ `FxHashMap<NodeId, T>` tables and allocates per-frame `FxHashSet<NodeId>` scratch sets.
6. **Missing Cross-Platform Containment Primitive**:
   - Neither `octara-model` nor `octara-host-web` exposes CSS-standard `Contain`, leaving authors with no declarative way to declare hard layout/size/paint containment boundaries consistently across Native and Web.

---

## 2. Core Architectural Decision: Delete `TaffyTree`, Keep Stateless Taffy Math Kernels

We **delete `taffy_tree` completely** from the workspace:

```toml
taffy = { version = "0.15.1", default-features = false, features = ["std", "flexbox", "grid", "content_size"] }
```

- **Zero `TaffyTree`**, zero `taffy::Style` copies, zero `taffy::NodeId` mapping tables, zero duplicate child `Vec`s.
- `taffy::NodeId` is a transparent `u64` wrapper (`impl From<u64> for taffy::NodeId`), giving a **zero-cost lossless bijection** with `octara_tree::NodeId` (`KeyData::as_ffi` / `KeyData::from_ffi`).
- Octara owns the entire layout kernel (`LayoutKernel`) in `crates/octara-pipeline/src/layout/` and implements Taffy's zero-copy low-level traits (`TraversePartialTree`, `LayoutPartialTree`, `LayoutContainingBlock`, `CacheTree`, `LayoutFlexboxContainer`, `LayoutGridContainer`) directly over `&octara_tree::Tree` + dense `SecondaryMap<NodeId, LayoutSlot>`.
- Taffy is demoted to a **pure stateless W3C Flexbox / Grid / OOF math library**, guaranteeing 100% layout parity with browser CSS in `octara-host-web` while handing 100% control over tree storage, dirty propagation, `RelayoutBoundary`, fast-path dispatch, caching, and rollback to Octara.

---

## 3. Clean-Sheet Six-Pillar Architecture

### 3.1 Zero-Copy ECS `LayoutKernel` (`crates/octara-pipeline/src/layout/`)

Replace `TaffyTree<NodeId>` and the 10 `FxHashMap` tables with a cache-dense ECS layout kernel backed directly by `SlotMap` indices:

```rust
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum BoundaryKind {
    #[default]
    None,
    /// Automatically inferred: definite parent-determined size + overflow clipped + no escaping OOF.
    ImplicitTight,
    /// Hard boundary: window root, anchored overlay root, or explicit `Contain::LAYOUT | Contain::SIZE`.
    Explicit,
}

pub(crate) struct LayoutSlot {
    pub(crate) layout: taffy::Layout,
    pub(crate) cache: taffy::Cache,
    /// Last full PerformLayout input received from parent (used to restart local relayout directly at a boundary).
    pub(crate) last_input: Option<taffy::LayoutInput>,
    pub(crate) last_output: taffy::LayoutOutput,
    /// Hoisted out-of-flow (absolute/fixed) descendants positioned by this containing block.
    pub(crate) hoisted_oof: SmallVec<[NodeId; 2]>,
    /// Monotonic epoch incremented only when this node's computed `taffy::Layout` actually ran/changed.
    pub(crate) layout_epoch: u64,
    pub(crate) boundary: BoundaryKind,
    /// True if any descendant (including self) has escaping out-of-flow positioning.
    pub(crate) has_escaping_oof: bool,
    pub(crate) dirty: bool,
}
```

Storage layout in `LayoutKernel`:
- `slots: SecondaryMap<NodeId, LayoutSlot>` — dense array indexed by `NodeId` slot (zero hashing).
- `frames: DenseProjected<Frame>` — `SecondaryMap<NodeId, Frame>` + reusable `Vec<(NodeId, Option<Frame>)>` undo log (zero `FxHashMap` allocations per frame).
- Sparse cold tables (`scroll_anchors`, `links`, `placement`, `fits`, `natural_sizes`, `measurements`, `native_sizes`) use `SecondaryMap<NodeId, T>` (allocated strictly on demand) and `DirtyBitmap` for dirty tracking.
- **Zero-Copy Style Adapter (`OctaraStyleRef<'a>`)**:
  - A tiny `Copy` struct holding `(&'a LayoutStyle, StyleOverride)` where `StyleOverride` carries root/overlay-root/anchored-fit overrides.
  - Implements `CoreStyle`, `FlexboxContainerStyle`, `FlexboxItemStyle`, `GridContainerStyle`, `GridItemStyle`, and `OofItemStyle` directly on `&LayoutStyle` (pointing straight into `octara-tree`'s interned `Arc<ResolvedStyle>`).
  - Equal grid tracks (`grid_columns` / `grid_rows`) are served via a zero-allocation `GridTrackIter` — **zero `Vec<GridTemplateComponent>` allocations per node**.
- **Direct Fast-Path Dispatch inside `LayoutPartialTree::compute_child_layout`**:
  1. **Cache hit**: consult `slot.cache` first (`taffy::compute_cached_layout`).
  2. **`Display::None`**: `taffy::compute_hidden_layout` (O(1)).
  3. **Empty / Fixed Non-Measure Leaf** (no live children, not a text/editor/icon/image/media/native measure leaf): compute box size, padding, and border directly in O(1) without entering Flexbox/Grid algorithms.
  4. **Measure Leaf** (`Text`, `TextInput`, `Icon`, `Image`, `Media`, `Native`): invoke `taffy::compute_leaf_layout` with `TextEngine`'s cached intrinsic probe path.
  5. **Flex / Grid Container**: delegate to `taffy::compute_flexbox_layout` / `taffy::compute_grid_layout`, followed by `taffy::compute_oof_layout` when `hoisted_oof` / OOF children exist.
  - Record `slot.last_input`, `slot.last_output`, `slot.layout`, `slot.layout_epoch = current_epoch`, and update `slot.boundary` + `slot.has_escaping_oof`.

---

### 3.2 Mathematically Sound Dual-Layer `RelayoutBoundary` & CSS-Standard `Contain`

#### Layer A — Explicit W3C `Contain` (Native + Web Parity)

Add standard CSS containment to `octara-model::style`, `octara-tree`, `octara`, `octara-pipeline`, and `octara-host-web`:

```rust
bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct Contain: u8 {
        const LAYOUT = 1 << 0;
        const SIZE   = 1 << 1;
        const PAINT  = 1 << 2;
        const CONTENT = Self::LAYOUT.bits() | Self::PAINT.bits();
        const STRICT  = Self::LAYOUT.bits() | Self::SIZE.bits() | Self::PAINT.bits();
    }
}
```

- `LayoutStyle` gains `pub contain: Contain` (`Default = Contain::empty()`).
- `StyleProp` gains `StyleProp::Contain(Contain)`.
- `Styled` facade gains `.contain(Contain)`, `.contain_content()`, and `.contain_strict()`.
- **Native semantics**:
  - `Contain::LAYOUT`: establishes a containing block for `Absolute` / `Fixed` descendants (preventing OOF escape) and isolates internal layout from ancestors' `scrollable_overflow_rect`.
  - `Contain::SIZE`: node's intrinsic size does not depend on descendants (treated as empty when sizing itself).
  - `Contain::PAINT`: clips descendants to the node's border/padding box (integrating cleanly with `PaintCompiler` and `FlatTree` culling).
  - `Contain::LAYOUT | Contain::SIZE` unconditionally establishes `BoundaryKind::Explicit`.
- **Web semantics (`octara-host-web/src/css.rs`)**:
  - Emits standard CSS `contain: layout;`, `contain: layout size;`, `contain: strict;`, etc. so Blink, WebKit, and Gecko activate their native relayout and paint containment boundaries with **100% behavioral parity**.

#### Layer B — Zero-Config Automatic `RelayoutBoundary` (Strict Soundness Invariants)

Without requiring any `.contain(...)` annotation from application authors, `LayoutKernel` automatically marks a node `B` as `BoundaryKind::ImplicitTight` after `PerformLayout` if and only if **all** of the following invariants hold:

1. **Parent-determined definite size**: `B` received `known_dimensions == (Some(w), Some(h))` (definite on both axes) from its parent during `PerformLayout`, OR `B`'s own `LayoutStyle` specifies definite `LayoutLength::Px` on both width and height with no content-dependent `min`/`max` override, AND `B`'s `last_input` is stored.
2. **Overflow isolated**: `B` clips overflow on both axes (`overflow_x != Visible && overflow_y != Visible`), so descendant geometry cannot alter any ancestor's `scrollable_overflow_rect`.
3. **No escaping out-of-flow descendants**: `!B.has_escaping_oof` (either `B` has no `Absolute`/`Fixed` descendants, or `B` itself is their containing block because `B.position != Static` or `B.contain.contains(Contain::LAYOUT)`).
4. **No own-style or parent-facing mutation**: On dirty marking, if `B` itself was modified (`delta.layout_dirty` contains `B` for a style/size/margin/position change, or `B`'s own children were added/removed while `B` is not `Contain::SIZE`), dirtying marks `B`'s parent too; if **only a strict descendant of `B`** changed, upward dirty propagation **stops at `B`** and enqueues `B` as a local relayout root!

#### Sound Escalation Protocol

When executing local relayout roots in ascending depth order (skipping any root whose ancestor is already queued):
1. Run `compute_node_layout(root, saved_input)` directly on `root`.
2. Compare `new_output.size == old_output.size` and (for non-contained nodes) `new_output.content_size == old_output.content_size` and `!root.has_escaping_oof`.
3. If any parent-observable quantity escaped (e.g., an optimistic child spawned a new `Fixed`/`Absolute` descendant that escapes `root`), clear the boundary bit on `root`, mark `root`'s parent dirty up to the next enclosing boundary, and continue until the dirty frontier is empty.
4. **Anchored Overlay Isolation**: `tree.overlay_root()`'s direct anchored overlay children are unconditionally `BoundaryKind::Explicit` (`Position::Absolute` against the viewport). Calling `apply_fit(overlay)` dirties and recomputes **only that overlay subtree**, never `tree.root()` and never the main document tree.

---

### 3.3 Epoch-Pruned Local Frame Projection (Eliminating Full-Window `collect_frames`)

Replace `collect_frames(..., full = true)` with **epoch-pruned projection**:

- Every projection pass advances `self.layout_epoch += 1`.
- When layout runs from a set of relayout roots `R` (e.g. a single hovered sidebar row, or `tree.root()` on viewport resize):
  - Only nodes whose layout actually recomputed during this pass receive `slot.layout_epoch = current_epoch`.
  - Frame projection walks **starting strictly from `R`** (plus `delta.scroll_dirty`, `delta.editor_dirty`, `delta.overlay_dirty`), never from `tree.root()` unless `tree.root()` itself is in `R`.
  - Inside `walk_frames(node, parent_ctx)`:
    - Recompute `Frame` for `node`.
    - Descend into `tree.children(node)` **if and only if**:
      - `child_ctx` changed (`origin`, `scroll_parent`, or `clip_parent` differs from previous frame), **OR**
      - `child` is marked in `dirty_ancestors` / `slot.layout_epoch == current_epoch`, **OR**
      - `child` is newly created or has `topology_dirty` / visibility change.
    - Clean subtrees whose `child_ctx` and `layout_epoch` are unchanged are **skipped in O(1)** — even during a full-root relayout!
  - Stale frame cleanup is driven directly by `delta.retired` (`RetiredNode::Destroyed`) and nodes transitioning to hidden/non-live — **eliminating the O(N) `visited: FxHashSet` allocation and full `frames.entries` scan**.

---

### 3.4 Incremental `FlatTree` Spatial Grid Patching (`crates/octara-pipeline/src/flat_tree.rs`)

Eliminate the `O(N)` spatial rebuild cliff when topology is unchanged and only a localized subset of nodes changed bounds:

- Track each `FlatNode`'s indexed cell range (`CellFootprint { grid_owner: Option<NodeId>, min: Cell, max: Cell, large: bool }`) in a parallel `Vec<CellFootprint>` indexed by flat index.
- In `FlatTree::project`:
  - **Topology changed**: rebuild `Topology` + spatial grids as before (with clean pre-order copy).
  - **Topology unchanged & `!layout.changed().is_empty()`**:
    - If `changed_count * 4 < total_nodes`, run **incremental spatial patch** directly on the staged grid:
      - For each `node` in `layout.changed()`, look up its flat index `idx`, update `bounds[idx]`, and if its `CellFootprint` or `scroll_ancestor` changed, remove `idx` from its previous cells (`swap_remove` + keep cell order or sort-on-hit / preserve descending paint-order invariant via binary search) and insert `idx` into its new cells!
      - *Ordering invariant*: `Grid::hit` expects candidates in ascending flat-index order (iterated in reverse for front-to-back top hit). Because `FlatTree::hit` already compares `top.is_none_or(|top| top < index)` across all candidates in the cell, unordered `swap_remove` + `push` preserves **exact hit-test correctness** while keeping patch cost strictly **O(1) per cell**!
    - Fallback to full `rebuild_bounds` only when a large fraction of the tree moved (e.g., window resize).
- `LayoutWork` and `FlatTree` metrics expose `frames_visited`, `boundaries_hit`, `escalations`, and `spatial_nodes_patched` so DST benchmarks can assert hard O(local) ceilings.

---

### 3.5 Non-Thrashing `TextEngine` Measurement Pipeline (`crates/octara-pipeline/src/text/`)

Eliminate repeated `break_all_lines` thrashing during Flex/Grid intrinsic and constrained probes:

1. **Per-`TextCache` Multi-Slot Measurement Probe Cache**:
   - Store a compact inline cache on `TextCache`:
     ```rust
     struct MeasureProbe {
         width_bits: Option<u32>, // None => unwrapped / MaxContent, Some(w.to_bits()) => constrained
         max_lines: usize,
         size: Size,
         lines: usize,
     }
     ```
     with `probes: SmallVec<[MeasureProbe; 4]>` (covering `MaxContent`, `MinContent`, previous definite width, and current definite width with zero heap allocation).
   - Before borrowing `RefCell<Shape>` or calling `break_all_lines`, check `probes` in O(1).
2. **`Wrap::None` / `Wrap::Truncate` Short-Circuit**:
   - Because `wrap_width` is always `None` for `Wrap::None` and `Wrap::Truncate`, cache the single unwrapped `(Size, lines)` once on `TextCache`; subsequent probes at any width apply `size.width.min(width)` for `Truncate` in **O(1)** without touching Parley.
3. **Preserve Final Line-Break State for `present()`**:
   - Because cached probes avoid calling `Shape::at(None)` / `Shape::at(Some(0.0))` after a width is known, `Shape::broken` stays warm at the final definite layout width when `TextEngine::present()` runs — avoiding a redundant final `break_all_lines`.
4. **Pure-LTR Fast Content Widths (Optional Verified Acceleration)**:
   - Note that `parley 0.13`'s `Layout::calculate_content_widths()` documents a caveat on mixed-direction (BiDi) text. Therefore, Octara uses exact `break_all_lines` backed by the 4-slot `MeasureProbe` cache as the universal ground truth for all BiDi, rich-span, and inline-attachment text, guaranteeing 100% parity with `present()` and zero BiDi edge-case drift.

---

### 3.6 `check_layout_cache` Full-Tree Equivalence Oracle

Mirroring `check_paint_cache: bool` and `check_a11y_cache: bool` in `NativeEngineConfig`:

- Add `pub check_layout_cache: bool` to `NativeEngineConfig` (default `false` in production, enabled in pipeline unit tests and DST suites).
- When `true`, after every incremental `LayoutKernel::project` pass, instantiate a cold full-tree `LayoutKernel` on the same `(tree, viewport)` and assert **exact equality** for every live node's:
  - `LayoutBox` (`rect`, `padding`, `border`),
  - `bounds`, `content_bounds`, `clip`, `scroll_range`, and `scroll_offset`,
  - `FlatTree::hit` consistency on sampled points.
- This gives a **mathematical proof by construction** that `RelayoutBoundary`, `Contain`, fast-path leaf layout, epoch-pruned frame projection, and incremental `FlatTree` patching never diverge from a cold full-tree layout.

---

## 4. Rollback & Multi-Round Causal Projection Safety

Octara's 5-phase Cycle Engine runs bounded multi-round `Project` loops and can `rollback()` on abort:

- `DenseProjected<T>` records the first replaced value per cycle in a reusable `Vec<(NodeId, Option<T>)>` (guarded by a `DirtyBitmap` `recorded` bit so each node is pushed at most once per cycle — **zero `FxHashMap` allocations**).
- On `rollback()`, replay the undo `Vec` in reverse, clear the `recorded` bitmap, and mark any nodes whose `LayoutSlot` ran during the rolled-back cycle in `rollback_dirty: DirtyBitmap` so the next cycle re-projects them accurately.

---

## 5. Hard Exit Gates & Measurable Performance Targets (`0.0.10` Wave D3)

1. **Zero `TaffyTree` Workspace-Wide**:
   - `taffy` in root `Cargo.toml` removes `"taffy_tree"` completely; `rg "TaffyTree|taffy_tree"` returns **0 hits** in `crates/` and `modules/`.
   - `layout/style.rs` builds zero `taffy::Style` values and zero `Vec<GridTemplateComponent>`.
2. **160-Row Sidebar Hover Regression (Canonical Octoii Workload)**:
   - Hovering a row that toggles an inner action button (changing title width `240 → 212 pt` inside a fixed `44 pt` clipped row):
     - `LayoutWork::frames` drops from **`~1,832` → `<= 16`** (`> 100×` reduction);
     - `LayoutWork::boundaries_hit >= 1` and `LayoutWork::escalations == 0`;
     - `FlatTree` patches only the changed nodes (`spatial_nodes_patched <= 16`, **0 full grid rebuilds**);
     - `TextEngine` probe cache hits eliminate redundant `break_all_lines` thrashing;
     - `check_layout_cache = true` passes with **0 diffs** against cold full-tree layout.
3. **Anchored Overlay Isolation Test**:
   - Triggering `apply_fit` on a tooltip/popover anchored inside a 160-row window recomputes **only the overlay subtree** (`LayoutWork::frames <= overlay_nodes + 2`, never re-walking the 160 rows).
4. **Cross-Platform `Contain` Parity**:
   - `Contain::LAYOUT | SIZE | PAINT` works identically in Native (`LayoutKernel` + `PaintCompiler`) and Web (`octara-host-web/src/css.rs` emits valid CSS `contain`), verified in both Native unit tests and Chromium DOM tests.
5. **Full Release Matrix**:
   - `python3 tools/check_layers.py`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (including `check_layout_cache`, `check_paint_cache`, `check_a11y_cache`), `cargo test -p octara-cli --doc`, DST performance ceilings, default + WebGPU Wasm checks, and iOS/Linux target checks all pass.

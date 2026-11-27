# 11 — Core Performance Architecture: CPU Cache, Algorithms, Modern GPU Micro-Pipelines & Inspect SPI (`0.0.6`)

> **Design premise**: Build a **silicon-sympathetic** open-source UI core grounded in real CPU cache lines, branch prediction, OS virtual memory, modern GPU Early-Z/TBDR/TMU fundamentals, optimal asymptotic data structures, and clean extension SPIs — with **zero legacy compatibility tax**.  
> **Prerequisite**: Builds cleanly atop `0.0.5`'s `NodeCore::present` (`TableMask`), `EditorRope`, `GlyphPipeline` (`24 B` `GlyphInst` + Vertex Pulling), and the 6 core SPI boundaries in [12 — Pluggable SPI Architecture](12-pluggable-spi-architecture.md).

---

## 1. Core Performance North-Star Targets (`0.0.6`)

| Metric | Target (`0.0.6` Desktop / Apple Silicon 120Hz) | Hard Enforcement |
| --- | --- | --- |
| Full-window draw calls (text + cards + borders + images) | **`1–4` instanced draws** (`Texture2DArray` + 4 micro-pipelines) | `FramePlan` batch metric |
| Vertex-to-fragment TBDR parameter traffic | **`4 B` per triangle** (`inst_id: u32`, down 39× from `156 B`) | WGSL compile + layout test |
| Variable-height virtual list scroll / height update | **$O(\log N)$** (`SumTree` / Fenwick Tree, 100k rows < 5 µs) | Microbenchmark + DST |
| Reactive hot node footprint & dirty scheduling | **`size_of::<ReactiveHot>() <= 32 B`**; $O(1)$ 4-tier depth-bucket pop | Compile-time assert + bench |
| Transient heap allocations on steady-state frame | **`0` in hot paint/cull path** (Per-Frame Bump Arena) | Allocator counter test |
| Headless inspect & custom `AuditRule` execution | **`< 10 ms`** deterministic pass with `file:line` provenance | `octara-inspect` suite |

---

## 2. CPU Cache, Memory Systems & OS Architecture

### 2.1 Hierarchical 64-Bit Dirty Bitmaps (Replacing `FxHashSet<NodeId>`)
- **Problem**: `TreeDelta`, `LayoutEngine::changed`, and `TextEngine::changed` allocate and hash 7+ `FxHashSet<NodeId>` instances every turn.
- **Design**: Dense slot-indexed **2-level bitmap (`DirtyBitmap`)**:
  - **L0**: `Vec<u64>` (1 bit per `SlotMap` index — 10,000 nodes = **1.25 KiB**, L1d resident).
  - **L1 Summary**: 1 bit per 64 L0 words (= 4,096 nodes per summary `u64`).
  - **Operations**:
    - Mark dirty: $O(1)$ bit OR, zero allocation, zero hashing.
    - Iterate dirty nodes: hardware `u64::trailing_zeros()` (`TZCNT`/`CTZ`), skipping 64 clean nodes per instruction.
    - Set union (`paint_dirty |= layout_changed | text_changed`): word-wise / SIMD bitwise OR.

### 2.2 Contiguous Pre-Order `FlatTree` + `O(1)` Subtree Skip + SIMD SoA Culling
- **Problem**: Tree traversal chases heap-allocated `Vec<NodeId>` in `Tables::children` (pointer chasing across disjoint cache lines).
- **Design**:
  - Maintain a compact contiguous DFS pre-order array rebuilt incrementally on `topology_dirty`:
    ```rust
    #[repr(C)]
    pub struct FlatNode {
        pub node: NodeId,
        pub subtree_end: u32, // exclusive pre-order index; jump over hidden/offscreen subtrees in O(1)
        pub depth: u16,
        pub flags: u16,
    }
    ```
  - Store hot viewport bounds in **16-byte aligned SoA / dense arrays** indexed by pre-order position (`bounds_x/y/w/h`).
  - Viewport culling and hit-testing run **sequential L1-prefetched SIMD scans** (`NEON` / `AVX2` / Wasm `simd128` testing 4 boxes per cycle) and skip clipped/offscreen subtrees via `idx = flat[idx].subtree_end`.

### 2.3 Subtree `u64` Style-Sensitivity Bloom Filter (Servo/Blink-Class Style Pruning)
- Every container subtree maintains a `u64` Bloom signature summarizing every dynamic condition used by any descendant:
  - `When::Dark` / `When::Light`, `When::GroupHover(hash)`, `State::FOCUS_WITHIN`, `Breakpoint` thresholds, `ThemePatch` overrides, and specific `ColorToken` slots.
- On hover, breakpoint change, or theme token mutation, compute `changed_mask: u64`; any container satisfying `(subtree_bloom & changed_mask) == 0` is **pruned in 1 bitwise AND** with zero descent.

### 2.4 Dual Ping-Pong Per-Frame Bump Arena + Cache-Line Isolation (`128 B`)
- **Per-Frame Bump Arena**: Transient per-frame scratch buffers (clip lists, temporary rects, pass item slices, sort keys) allocate linearly from a double-buffered bump arena and reset via `offset = 0` at frame end — **zero global allocator lock contention and zero fragmentation**.
- **Apple Silicon `128 B` False-Sharing Elimination**: All cross-thread atomic counters, SPSC ring pointers, and generation stamps use `#[repr(align(128))]` (covering both x86 `64 B` and Apple Silicon `128 B` cache lines).
- **Instruction Cache (I-Cache) Discipline**: Cold error, fallback, and diagnostic branches annotated `#[cold]` + `#[inline(never)]`; hot math/culling loops written branchlessly (`f32::min`/`f32::max` → single `FMINNM`/`FMAXNM`).

### 2.5 OS Virtual Memory (`mmap`) & Heterogeneous Core QoS Scheduling
- **Zero-Copy `mmap` Fonts**: System/bundled CJK fonts (15–30 MiB) map directly via read-only `mmap` (`MADV_WILLNEED` / `MADV_DONTNEED`), sharing physical OS Page Cache pages across windows/processes with millisecond cold start.
- **P-Core vs. E-Core QoS Affinity**:
  - `USER_INTERACTIVE` (P-Cores): Turn engine, layout, input ingress, frame GPU encode.
  - `UTILITY` / `BACKGROUND` (E-Cores): Offscreen syntax highlighting, image decoding, atlas compaction — keeping UI L2 cache warm and battery drain low.

### 2.6 `octara-runtime` Signals Kernel & Final-Form Public API (`Wave 1` + `Wave 1b`)
Octara's core capability contexts (`BuildCx`/`ReadCx`/`WriteCx`/`CleanupCx`/`TaskCx`), `TypedSlab`, all-`Copy` handles, and unified Value+Delta DAG remain the permanent foundation. `0.0.6` upgrades both the internal kernel (Wave 1) and the four long-term public Signals API debts (Wave 1b):
1. **4-Tier Depth-Bucket + `u64` Bitmap Scheduler (`Wave 1`)**:
   - Replace `ReactiveWorld::pending: BTreeSet<(FlushTier, u32, ReactiveId)>` with `[Vec<Vec<u32>>; 4]` depth buckets + a `u64` non-empty depth mask per tier (`trailing_zeros()` pops minimum depth in $O(1)$) + a 1-bit `scheduled` flag.
2. **Hot/Cold `ReactiveNode` Split (`ReactiveHot <= 32 B`) + Zero-Copy `Check` (`Wave 1`)**:
   - Split `ReactiveNode` into `ReactiveHot` (`<= 32 B`) and `ReactiveCold`, eliminating `sources.clone()` inside `needs_evaluation`.
3. **Computed `Arc<ResolvedStyle>` Interning (`Wave 1`)**:
   - Deduplicate identical resolved `(LayoutStyle, Visual)` pairs in `octara-tree` via content-addressed `Arc<ResolvedStyle>`.
4. **True Fine-Grained `Store<T>` Replacing Fake `Lens` / `BuildCx::slice` (`Wave 1b`)**:
   - Delete `Lens`, `BuildCx::slice`, `WriteCx::set_slice`, and `WriteCx::update_slice` (which previously allocated a `Memo` and cloned sibling fields on every unrelated write).
   - Redesign `#[derive(Store)]` + `cx.store(init) -> Store<T>` so each field owns its own fine-grained reactive revision (`store.field()` returns a zero-clone `Copy` field handle; mutating `store.count` never wakes observers of `store.title`; supports nested `Store` field projections and whole-store reads/writes).
5. **Unified `Readable<T>` / `Writable<T>` + `Rc`-Free `Binding<T>` (`Wave 1b`)**:
   - Unify handle operations in one direction across `Signal<T>`, `ReadSignal<T>`, `Store` fields, `RowSignal<T>`, and `Binding<T>` (`r.get(x)` / `r.with(x, f)` / `w.set(x, v)` / `w.update(x, f)` / `w.mutate(x, f)`), removing the inverted `binding.get(r)` / `binding.set(w, v)` API.
   - Move custom `Binding` accessors into `ReactiveWorld` slab storage, eliminating `Rc<Accessors<T>>` completely.
6. **8-Byte Register-Sized `ReactiveId` & `Signal<T>` (`Wave 1b`)**:
   - Pack `ReactiveId` into **8 bytes** (`const { assert!(size_of::<ReactiveId>() == 8); assert!(size_of::<Signal<i32>>() == 8); }`), moving slab `slot` into node storage and halving `Source` edge footprint (`32 B -> 16 B`).
7. **In-Place `memo_with`, Exact `mutate`, and Zero-Byte `Trigger` (`Wave 1b`)**:
   - `cx.memo_with(init, |r, prev: &mut T| -> bool) -> ReadSignal<T>`: enables zero-allocation buffer reuse (`Vec`/`String`/indices) without requiring `T: PartialEq`.
   - `w.mutate(target, |v: &mut T| -> bool) -> bool`: marks changed **only** when the closure returns `true`, eliminating unconditional false invalidations on early return.
   - `cx.trigger() -> Trigger`: zero-payload invalidation handle (`r.track(t)` / `w.notify(t)`) replacing `Signal<()>` with zero slab allocation.

---

## 3. Optimal Data Structures & Algorithms

| Subsystem | Current Complexity | `0.0.6` Structure / Algorithm | Target Complexity |
| --- | --- | --- | --- |
| Variable-height `virtual_each` offsets & scroll-to-row lookup | $O(N)$ linear scan | **Fenwick Tree / Prefix-Sum B-Tree (`SumTree`)** aggregating `(count, total_height, max_width)` | **$O(\log N)$** update, offset query & `scroll_y` binary descent |
| Keyed list reconciliation (`each` reorder) | Greedy moves | **$O(K \log K)$ Longest Increasing Subsequence (LIS)** | Theoretical minimum DOM/tree moves (preserves focus & IME) |
| GPU draw-batch ordering | Sequential pass push | **64-bit Sort Key + $O(N)$ Stable Radix Sort** (within transparency/depth barriers) | $O(N)$ branchless batch minimization |
| Spatial hit-testing | Linear tree walk | **Coarse 2D Spatial Grid + `FlatTree` subtree bounds** | $O(1)$ expected cell lookup |

---

## 4. Modern Open-Source GPU Micro-Pipelines (`octara-render`)

Finish modernizing the built-in `wgpu` renderer behind `SPI-1` (`SurfaceRenderer`), leaving specialized compute/tile/wide-gamut/FX renderers cleanly pluggable via `SurfaceRenderer`:

### 4.1 Full-Pipeline Vertex Pulling + 4 Specialized Micro-Pipelines
Eliminate the legacy `156 B` uber-shader (`crates/octara-render/src/shader.wgsl`) completely:
- **Vertex Pulling (`VsOut` = `4 B`)**: Every vertex shader outputs only `@builtin(position)` + `@location(0) @interpolate(flat) inst_id: u32` (39× less TBDR Parameter Buffer traffic). Fragment shaders pull typed instances via GPU scalar/uniform L1 cache broadcast.
- **4-Vertex `TriangleStrip`** with bit-twiddled corners (`vec2f(f32(vi & 1u), f32(vi >> 1u))`) — 33% fewer vertex invocations than 6-vertex triangles.
- **4 Isolated Micro-Pipelines** (preventing worst-case VGPR register contamination and warp divergence):
  1. **`GlyphPipeline`** (`24 B` `GlyphInst`, ~6 VGPRs, 100% occupancy; shipped in `0.0.5`, upgraded to `Texture2DArray` in `0.0.6`).
  2. **`QuadPipeline`** (`64 B` `QuadInst`: packed RGBA8 `u32`, `f16x4` radii, `u16` index into cold `GradientBuffer`, `u16` `clip_id`).
  3. **`ShadowPipeline`** (isolated `erf()`/`exp()` registers + **4-Strip Hollow Frame** geometry skipping interior pixels occluded by opaque cards → **80–95% fewer shadow fragments**).
  4. **`ImagePipeline`** (hardware fixed-function TMU `textureSampleLevel` + **Mipmap** generation replacing software 4-tap `textureLoad` → **4×–16× less texture cache traffic** and shimmer-free downscaling).

### 4.2 Hardware Early-Z / Hi-Z Two-Pass Opaque Overdraw Elimination
Enable a `Depth24Plus` / `Depth16Unorm` attachment on the render target with monotonic painter depth `z`:
- **Pass 1 — Opaque Inscribed Cores (Front-to-Back, `Blend = None`, `DepthWrite = true`, `DepthCompare = Greater`)**:
  - Every fully opaque (`fill.a == 1.0`) quad/image/card emits its inscribed solid core `[x+r, y+r, w-2r, h-2r]` with **zero SDF math and zero framebuffer RMW blend**, populating hardware Hi-Z / TBDR HSR.
- **Pass 2 — AA Edges, Borders, Translucent Surfaces, Shadows & Glyphs (Back-to-Front, `DepthWrite = false`, `DepthCompare = GreaterEqual`)**:
  - Hardware Early-Z discards 100% of occluded background text, shadows, borders, and images before a single fragment instruction executes.

### 4.3 Vertex-Clamped Rectangular Clips, `Texture2DArray` Atlases & Zero-Copy Planning
- **Clip Fast Paths**: Collapse nested axis-aligned rectangular clips (`radius == 0`) on CPU and clamp vertices directly in the **Vertex Shader** (0 fragment clip instructions for ~90% of scroll containers); strip rounded clips (`clip_head = 0`) when `inst.rect` lies strictly inside the rounded clip's inscribed inner safe box.
- **`Texture2DArray` + Skyline Allocator**: Replace multi-page bind-group-breaking `Bindings([Option<PageId>; 3])` with **`Texture2DArray`** (page index = `u16` array layer → **1–4 draw calls per frame**), paired with a **Skyline / Guillotine allocator + per-region LRU eviction**.
- **Zero-Copy Frame Planning**: Eliminate `DisplayList::items()` full-frame cloning; iterate `DisplayList::parts()` directly into `FramePlan`, and keep `SurfaceRenderer` clean for pluggable external GPU backends (including optional `feature = "webgpu"`).

---

## 5. `octara-inspect` Audit SPI & Deterministic Performance Gates

1. **Pluggable `AuditRule` SPI (`SPI-4`)**:
   - Define the serializable `AuditFinding { code, severity, node, provenance, message }` in `octara-model`. Re-export it alongside `AuditRule` in `octara-inspect`.
   - Ship clean built-in open-source checks (`ClippedText` reusing `TextEngine::clipped`, `MissingSemantics`, `DeadFocusTarget`), allowing applications and external crates to register rich custom design-system, contrast, overlap, and telemetry audit packs cleanly.
2. **Deterministic Performance Regression Gates (DST)**:
   - CI replays canonical stress scenarios (10k-row scroll, 120Hz streaming Markdown, CJK IME composition) under virtual clock and asserts hard ceilings on `draw_calls`, `uploaded_bytes`, `shapes_built`, and `heap_allocs`.
   - Run `cargo test -p octara-examples --test dst_performance -- --nocapture` to print the observed peaks and ceilings. The device-free frame planner measures draw batches, logical upload bytes and heap allocations. The native projection supplies text shaping counts.

---

## 6. Phased `0.0.6` Rollout & Verification

| Wave | Scope | Hard Exit Gate |
| --- | --- | --- |
| **Wave 1: CPU Cache & Algorithmic Core** | `DirtyBitmap` (2-level 64-bit), `ReactiveHot (<= 32 B)`/`ReactiveCold` split + 4-tier depth-bucket `u64` scheduler + zero-copy `Check`, `Arc<ResolvedStyle>` interning, `FlatTree` + `O(1)` skip + SIMD SoA culling, Subtree `u64` Style Bloom Filter, `SumTree`/Fenwick variable-height `virtual_each`, LIS `each` reconciliation, Per-Frame Bump Arena, `128 B` alignment, P/E-Core QoS & `mmap` font loading | 0 hot-path heap allocs verified; `size_of::<ReactiveHot>() <= 32`; 100k-row `virtual_each` update < 5 µs; Bloom filter prunes > 90% untouched subtrees; DST 100% deterministic |
| **Wave 1b: Final-Form Signals Public API** | Delete `Lens`/`BuildCx::slice`; true fine-grained `#[derive(Store)]` (`Store<T>` per-field revision, zero sibling wakeups & zero `Clone`, nested stores); unified `Readable<T>`/`Writable<T>` + `Rc`-free `Binding<T>`; 8 B `ReactiveId`/`Signal<T>` (`Source` 16 B); `cx.memo_with`, `w.mutate`, zero-byte `Trigger`; migrate full workspace | `size_of::<ReactiveId>() == 8`, `size_of::<Signal<i32>>() == 8`, `size_of::<ReactiveHot>() <= 32`; 0 `Rc` in `octara-runtime` `Binding`; sibling-field zero-wakeup & zero-clone `Store` tests green; full workspace & doctests green |
| **Wave 2: Modern GPU Micro-Pipelines** | Delete legacy `156 B` uber-shader; 4 micro-pipelines + full `Vertex Pulling` (`VsOut = 4 B`) + 4-vertex `TriangleStrip`; Early-Z/Hi-Z 2-pass opaque core; hardware TMU + Mipmaps; Vertex rectangular clip + inner-safe rounded clip bypass; 4-Strip hollow shadows/rings; `Texture2DArray` + Skyline Atlas + Radix Sort batching + zero-copy `DisplayList` planning; finalize `SurfaceRenderer` SPI | CPU Oracle ↔ GPU parity tests green; `draw_calls <= 4` on chat showcase; dual `wasm32` & `ios-sim` checks green |
| **Wave 3: Audit, Release & Performance Gates** | `AuditRule`, `InspectTransport`, `TraceRecorder`, `ReleaseProvider`, and `SecurityPolicy` SPIs, built-in text clipping and accessibility checks, DST performance assertions, `octara audit`, refreshed API catalog and exported skill, and workspace version `0.0.6` | All nine release gates and three DST performance scenarios pass |

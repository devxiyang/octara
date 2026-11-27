# Octara v2 Spec 03: Sparse ECS Micro-Kernel (`octara-tree`)

> Part of the [Octara v2 Clean-Sheet Architecture](../architecture-v2.md).

---

## 1. Responsibility & Hard Boundaries

`octara-tree` is a **pure, engine-agnostic retained UI micro-kernel**.
- **Owns**: node identity & topology, sparse properties, style resolution (`StyleResolver`), atomic `Transaction` validation, `TreeDelta` production, `InteractionArbiter`, focus/modal/overlay rules, logical UTF-8 text & rich spans, logical controlled `EditorSession` & selection regions.
- **Does NOT own**: Taffy layout trees, Parley text shaping/glyphs/line-wrap, AccessKit tree projections, GPU/CPU display lists, animation tracks/ghosts/blink timers, DOM elements, or OS window handles.

### Module Structure

```text
crates/octara-tree/src/
  lib.rs             # Tree struct, NodeCore, NodeFlags, read API, two write gates
  tables.rs          # Sparse SecondaryMap side tables (StyleSlot, TextSlot, EditorSlot, SemanticsSlot, ...)
  transaction.rs     # Transaction, TxOp, NodeSpec, NodeRef, TreeReceipt, RejectedTransaction, Preflight
  delta.rs           # TreeDelta, RetiredNode, GeometryDemand, delta merging
  runtime_op.rs      # RuntimeOp enum (sole write gate for transient runtime state)
  style.rs           # StyleResolver (base + When overrides + inherited text + theme resolution)
  arbiter.rs         # InteractionArbiter (unified pointer contact, outside-dismiss, touch-scroll routing)
  focus.rs           # Focus traversal, tab_index, FocusGroup, and layer focus policies
  overlay.rs         # Layer ordering, portal ownership, groups, and modal isolation
  editor.rs          # Logical EditorSession (controlled edit, composition, selection, undo stack, secure wipe)
  selection.rs       # Document selection Address=(NodeId, byte_offset), logical text extraction
  semantics.rs       # Shared modal accessibility scope rules (consumed identically by AccessKit & DOM)
```

---

## 2. Sparse ECS Memory Layout

### 2.1 Hot Table (`SlotMap<NodeId, NodeCore>`)

```rust
bitflags::bitflags! {
    #[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
    pub struct NodeFlags: u32 {
        const ATTACHED           = 1 << 0;
        const EXITING            = 1 << 1; // logical exit: invisible to focus, hit-test, a11y
        const HIDDEN             = 1 << 2; // a11y hidden
        const FOCUSABLE          = 1 << 3;
        const FOCUS_PREFERRED    = 1 << 4;
        const PRESERVE_FOCUS     = 1 << 5;
        const AUTOFOCUS          = 1 << 6;
        const HOVER_ALONE        = 1 << 7;
        const SELECT_REGION      = 1 << 8;
        const SELECT_NONE        = 1 << 9;
        const FOLLOW_TAIL        = 1 << 10;
        const FOLLOW_TAIL_ACTIVE = 1 << 11;
        const HAS_OVERLAY_MOUNT  = 1 << 12;
        const HAS_ANCHOR         = 1 << 13;
        const HAS_NATIVE         = 1 << 14;
        const HAS_CUSTOM_PAINT   = 1 << 15;
    }
}

#[repr(C)]
pub struct NodeCore {
    pub parent: Option<NodeId>, // 8B
    pub kind: NodeKind,         // 1B (Root | Box | Text | TextInput | Image | Icon | Canvas | Native | Link)
    pub state: State,           // 2B
    pub flags: NodeFlags,       // 4B
    pub style_rev: u32,         // 4B
    pub layout_rev: u32,        // 4B
    pub paint_rev: u32,         // 4B
    pub semantics_rev: u32,     // 4B
}
// Target assertion in unit tests: assert!(std::mem::size_of::<NodeCore>() <= 48);
```

### 2.2 Sparse Side Tables (`SecondaryMap<NodeId, T>`)

| Side Table | Allocated For | Payload |
| --- | --- | --- |
| `children` | Non-empty `Box`/`Root` | `Vec<NodeId>` (`O(1)` indexed child access) |
| `styles` | Styled nodes | `base: SmallVec<[StyleProp; 8]>`, `overrides: SmallVec<[(When, Vec<StyleProp>); 2]>`, `layout: LayoutStyle`, `visual: Visual`, `inherited: Inherited`, `inherited_from: Inherited`, `group: Option<&'static str>` |
| `texts` | `Text` nodes | `text: Arc<str>`, `runs: Vec<(Range<usize>, RichSpan)>`, `links: Vec<NodeId>`, `text_rev: u64` |
| `links` | `Link` nodes | `range: Range<usize>`, `href: Arc<str>` |
| `editors` | `TextInput` nodes | `value: String` (zeroed via `write_volatile` on drop if `secure`), `placeholder: Arc<str>`, `document_rev: u64`, `session_gen: u64`, `composition: Option<Composition>`, `selection: (usize, usize)`, `undo: UndoHistory`, `multiline: bool`, `rows: (u16, u16)`, `submit_on_enter: bool`, `secure: bool`, `revealed: bool` |
| `images` | `Image` nodes | `image: Image`, `fit: ObjectFit` |
| `icons` | `Icon` nodes | `icon: Arc<NodeIcon>` |
| `canvases` | `Canvas` nodes | `ops: Arc<[CanvasOp]>` |
| `natives` | `Native` nodes | `kind: &'static str`, `slot: NativeSlot`, `props: serde_json::Value`, `props_rev: u64`, `live: bool` |
| `paints` | Custom shader/transition nodes | `custom: Option<CustomPaintBinding>`, `transition: Option<TransitionBinding>` |
| `semantics` | Nodes with explicit semantics/a11y | `role: Option<Role>`, `label: Option<Arc<str>>`, `description: Option<Arc<str>>`, `tooltip: Option<Arc<str>>`, `a11y_value: Option<Arc<str>>`, `relations: Relations<Arc<str>>`, `numeric_range: Option<NumericRange>`, `checked: Option<bool>`, `required: Option<bool>`, `invalid: Option<bool>`, `expanded: Option<bool>`, `level: Option<u8>`, `live: Option<Live>`, `cursor: Option<CursorStyle>`, `tab_index: i32`, `focus_group: Option<FocusGroup>`, `group_current: Option<NodeId>` |
| `handlers` | Interactive nodes | `SmallVec<[(HandlerKind, HandlerId); 2]>` |
| `commands` | Command owners | `key_context: Option<Arc<str>>`, `commands: SmallVec<[(Arc<str>, HandlerId); 2]>` |
| `overlays` | Overlays, layers, and logical scopes | `anchor: Option<(NodeId, Placement)>`, `placed: Option<Placed>`, `layer: Option<LayerPolicy>`, `group: Option<LayerGroupId>`, `mount: Option<OverlayMount>` |
| `scrolls` | Scroll containers | `offset: Vec2`, `requested: Option<(u64, Vec2)>` |
| `provenance` | Debug/test/row-keyed nodes | `test_id: Option<Arc<str>>`, `row_key: Option<RowKey>`, `component: Option<Component>`, `origin: &'static Location<'static>` |

### 2.3 Secondary Indices (Maintained Only by `Tree::apply` / `apply_runtime`)
- `test_id_index: FxHashMap<Arc<str>, SmallVec<[NodeId; 1]>>` (only attached, non-exiting nodes)
- `row_children: FxHashMap<(NodeId, RowKey), NodeId>`
- `layer_nodes: FxHashSet<NodeId>` (modals / dismissible layers)
- `anchored: Vec<NodeId>` (parents before nested overlays)
- `native_nodes: FxHashSet<NodeId>`

---

## 3. Atomic Transaction Protocol (`transaction.rs`)

### 3.1 Structures
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PendingNode { batch_ns: u128, index: u32 }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NodeRef {
    Existing { tree: TreeId, id: NodeId },
    Pending(PendingNode),
}

pub enum TxOp {
    Change(NodeRef, NodeChange),
    Place { parent: NodeRef, index: usize, child: NodeRef },
    Retire { node: NodeRef, mode: Retirement }, // Exit | VisibleExit | Destroy
}

pub struct Transaction {
    pub tree: TreeId,
    pub base_rev: TreeRev,
    pub batch_ns: u128,
    pub specs: Vec<NodeSpec>,
    pub ops: Vec<TxOp>,
}
```

### 3.2 Preflight Validation Algorithm (`O(|tx| * tree_depth)`)
Preflight runs strictly **read-only** over `&self` + a local `StagedTopology` map of touched nodes only:
1. Verify `tx.tree == self.id` (`WrongTree`) and `tx.base_rev == self.rev` (`StaleRevision`).
2. Check `self.rev.0.checked_add(1)` (`RevisionExhausted`).
3. Validate every `NodeRef` in `tx.ops`:
   - `Existing { tree, id }`: must match `self.id`, exist in `nodes`, not be `EXITING`, and not be an internal node (`Root`, `overlay_layer`, `Link`) when being retired or reparented.
   - `Pending(p)`: `p.batch_ns == tx.batch_ns` and `p.index < tx.specs.len()`.
4. Validate content compatibility of every `NodeChange` against the target's `NodeKind` (`WrongContent`).
5. Validate `TextEdit` UTF-8 boundaries against current `TextSlot` (`InvalidTextEdit`).
6. Build `StagedTopology`:
   - Check no duplicate placement of the same child in the same batch (`DuplicatePlacement`).
   - Check every `parent` is `Kind::Root | Kind::Box` (`WrongContent`).
   - Check no duplicate `RowKey` among the final children of any touched parent (`DuplicateRow`).
   - Walk ancestors in `StagedTopology` overlaid on `self.nodes` for every placed edge: reject if `child` reaches itself (`Cycle`).
7. On any error: return `Err(RejectedTransaction { reason, tx: Box::new(tx) })` returning ownership of all authored paint assets to the caller — **zero mutation to `self`**.

### 3.3 Atomic Commit Order
1. Allocate all `specs` → `receipt.created: Vec<NodeId>`.
2. Apply all `TxOp::Change` ops (resolving `Pending -> NodeId`), recording dirty sets in `TreeDelta`.
3. Detach moved children from old parents **before** attaching to new parents (prevents transient cycles during sibling/subtree rotations).
4. Execute `TxOp::Retire`:
   - `Retirement::Destroy`: recursively remove subtree, clean indices & side tables, push custom paint assets to `retired_assets` (never dropped inside `Tree`).
   - `Retirement::VisibleExit` destroys a subtree immediately when it has no positive-duration exit motion or pending descendant exit. Otherwise, `Retirement::Exit` and `VisibleExit` mark the subtree and its owned portals as exiting and remove them from test IDs and interaction eligibility. The root, owned portal roots, and descendants with authored opacity motion or an explicit exit request receive exit tickets. The host reports `HostReply::ExitFinished(ticket)` for each participant. The subtree stays retained until the barrier settles, and replacing an overlay cancels tickets for its previous mount.
5. Attach all placed children at final indices, update `NodeFlags::ATTACHED`, re-resolve inherited styles on affected subtrees, bump `self.rev`.

---

## 4. Sole Transient Write Gate (`RuntimeOp`)

Replaces all 63 scattered `&mut self` methods from v1:
```rust
pub enum RuntimeOp {
    SetState { node: NodeId, state: State, on: bool },
    SetFocus { node: Option<NodeId>, visible: bool },
    SetScroll { node: NodeId, offset: Vec2, clamped: bool },
    RequestScroll { node: NodeId, target: Vec2 },
    ReleaseFollowTail { node: NodeId },
    SetPlacement { node: NodeId, placed: Option<Placed> },
    EditorApply { node: NodeId, update: EditorRuntimeUpdate },
    SetDocSelection { region: NodeId, selection: Option<[(NodeId, usize); 2]> },
    SetNativeLive { node: NodeId, live: bool },
    SetViewportContext { breakpoint: Breakpoint, touch: bool },
    ThemeChanged,
    FinishExit(ExitTicket), // emitted when HostEngine finishes a participating visual exit
}
```
Every `RuntimeOp` returns a `TreeDelta` so `HostEngine` observes transient changes (e.g., hover style change, focus ring, scroll offset, exit completion) through the **exact same incremental delta channel** as authored changes!

---

## 5. `InteractionArbiter` & Logical `EditorSession`

### 5.1 Unified Pointer Contact State Machine (`arbiter.rs`)
Native hit-test and Web DOM `WeakMap` lookup both supply `ContactTarget { hit: TargetGuard, action: Option<TargetGuard> }`. `InteractionArbiter` runs one state machine for both:
1. **`PointerDown`**:
   - Validate `target.tree == tree.id` and node is attached, non-exiting, non-disabled, and inside active modal scope.
   - Determine outside-dismiss candidates (non-modal popovers / modal backdrops) — **deferred until `Up` for touch contacts when an editor keyboard is open** (prevents iOS keyboard-collapse mis-clicks!).
   - Enter `ContactState::Pressed { id, kind, hit, action, down_pos, captured_scroll: None }`; set `State::PRESSED` via `RuntimeOp`.
2. **`PointerMove`**:
   - If displacement exceeds drag/scroll threshold (`Tuning::drag_threshold`):
     - Cancel `PRESSED` preview;
     - If draggable target → emit `Drag` handler;
     - Else if scrollable ancestor → capture `TouchScroll` session and route displacement through `on_touch_scroll(Pre)` → viewport scroll → `on_touch_scroll(Post)` so nested panel+list displacement sums to exact finger delta.
3. **`PointerUp`**:
   - If scroll/drag captured → finalize inertia / drag end; zero press activation.
   - Otherwise: settle deferred outside-dismiss & focus handoff first → **re-verify** original `action: TargetGuard` is still attached, non-exiting, non-disabled, and within current modal scope → if valid, deliver `Press`/`Toggle`/`DoublePress`.
4. **`PointerCancel` / `WindowActive(false)` / target retirement**:
   - Immediately clear `PRESSED` and cancel active contact; late moves/ups for that `PointerId` are rejected.

### 5.2 Logical layer scope and focus

`LayerPolicy` controls outside inertness and independently enables dismissal on Escape, outside press, and focus departure. The top eligible layer handles each dismissal. A nonmodal layer includes its portal owner and can share interaction scope with other layers and triggers through `LayerGroupId`. Portal ownership controls lifetime even when a group shares interaction scope.

`FocusTrapFeatures` independently controls initial focus, Tab wrapping, focus locking, restoration to a live previous control, and container focus after a focused descendant is removed. On an outside press, an accepted focus change to an external control takes precedence over restoration.

### 5.3 Controlled `EditorSession` (`editor.rs`)
- Each focused `TextInput` has an `EditorSessionId { tree, node, session_gen }`.
- Re-focusing the same node or resetting its document bumps `session_gen` — stale native/Web `EditorEvent`s are deterministically rejected.
- `ProposeEdit { session, expected_rev, edit }`:
  1. Reject if `session` mismatches active focused editor or `expected_rev != editor.document_rev`.
  2. Apply `edit` tentatively to produce `proposed_value`, pass to app's `on_input` handler.
  3. Only if app accepts (updates bound value), commit `document_rev += 1`, update selection (preserving reverse anchor/focus direction), and push undo record; otherwise reject and notify host to reconcile.
- Secure fields (`secure: true`): never enter undo history, never enter clipboard/recordings/Inspector dumps, and overwrite UTF-8 bytes with `std::ptr::write_volatile` on replacement and `Drop`.

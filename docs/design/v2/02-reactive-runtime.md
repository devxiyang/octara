# Octara v2 Spec 02: Reactive Runtime, Capability Contexts & Turn Engine (`octara-runtime` + `octara`)

> Part of the [Octara v2 Clean-Sheet Architecture](../architecture-v2.md).

---

## 1. Module Structure

```text
crates/octara-runtime/src/
  lib.rs
  arena/
    mod.rs           # ReactiveWorld, ReactiveId, ReactiveNode header, NodeKind
    slab.rs          # TypedSlab<T>, AnySlab, SlabStore (zero Rc, zero RefCell, zero per-signal Box<dyn Any>)
    scope.rs         # ScopeId, ScopeData, resumable teardown, Context lookup, persisted state store
  graph/
    mod.rs           # Dirty/Check/Clean push-mark, ensure_clean(id) pull engine, dynamic depth maintenance
    value.rs         # Signal<T>, ReadSignal<T>, Memo<T>, Lens/Slice, Watch
    delta.rs         # DeltaLog<D> bounded ring buffer, Cursor, Changes<D>
    list/
      mod.rs         # StateList<K, T>, ReadList<K, T>, ListDelta<K>, per-key RowSignal O(1) index
      seq.rs         # Order-statistic treap Seq<K> (O(log n) insert/remove/rank/nth)
      filter.rs      # filter & filter_with(cx, signal, pred)
      sort.rs        # sort_by & sort_by_with(cx, signal, key_fn)
      map.rs         # map & map_with(cx, signal, proj)
      group.rs       # group_by & flatten
      join.rs        # join with reverse index HashMap<JoinKey, SmallVec<[K; 2]>>
    text_buf.rs      # TextBuffer (Delta node over String + TextEdit log)
  cx/
    mod.rs           # BuildCx, ReadCx, WriteCx, CleanupCx, TaskCx
  task/
    mod.rs           # TaskScheduler, VirtualClock, Sleep, Interval, Channel, Background (native)
    async_memo.rs    # AsyncMemo<T> integrated into ReactiveWorld
  engine.rs          # HostEngine trait, ProjectFeedback, ProjectError, CommitOutput, PlatformServices trait
  turn.rs            # 5-Phase Turn Engine (Ingress -> React -> Commit -> Project -> Publish)
  window.rs          # WindowRuntime (Tree + HostEngine + TxBuilder + handlers + feedback registries)
  app.rs             # AppRuntime (ReactiveWorld + TaskScheduler + multi-window map + panic latching)

crates/octara/src/
  lib.rs             # Public facade + prelude
  app.rs             # App, Window, AppConfig, AppDefinition, AppMode, RestartState
  view/
    mod.rs           # View, IntoView, MountCtx (streams directly into TxBuilder)
    el.rs            # SmallVec-backed El builder, IntoProp, IntoText, atomic build_el
    style.rs         # Value-polymorphic Styled trait (zero Tailwind scale explosion)
    control.rs       # show, scope, overlay, portal
    list.rs          # each, virtual_each (pinned resident keys), reveal_row
    behavior.rs      # Composable handler behaviors
    pace.rs          # Paced streaming text
```

---

## 2. Memory Layout: `ReactiveId` & `TypedSlab<T>`

### 2.1 Universal `Copy` Handles
Every reactive handle in Octara v2 is a 16-byte `Copy` handle wrapping `ReactiveId` (plus zero-sized `PhantomData`):

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ReactiveId {
    pub(crate) runtime: u32, // distinguishes concurrent Apps without thread_local!
    pub(crate) node: u32,    // index in ReactiveWorld::nodes
    pub(crate) gen: u32,     // incremented on slot reuse
    pub(crate) bucket: u16,  // direct index into SlabStore::buckets (O(1), no HashMap lookup on hot read!)
    pub(crate) slot: u32,    // index into TypedSlab<T>::slots (u32::MAX if node has no value payload)
}
```

Handles that are ALL `Copy`:
- `Signal<T>` / `ReadSignal<T>`
- `StateList<K, T>` / `ReadList<K, T>`
- `TextBuffer`
- `Watch`
- `ScopeId`

### 2.2 Type-Partitioned Slab Storage (`SlabStore`)
```rust
struct ValueSlot<T> {
    gen: u32,
    value: Option<T>,
}

struct TypedSlab<T: 'static> {
    slots: Vec<ValueSlot<T>>,
    free: Vec<u32>,
}

trait AnySlab {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn drop_slot(&mut self, slot: u32, gen: u32);
}

struct SlabStore {
    by_type: FxHashMap<TypeId, u16>,
    buckets: Vec<Box<dyn AnySlab>>,
}
```
- **Hot read path**: `buckets[id.bucket as usize].as_any().downcast_ref::<TypedSlab<T>>()` → `slots[id.slot as usize]`. Zero `HashMap` hashing on read, zero `Rc`, zero `RefCell`, zero per-signal heap allocation.
- **Disposal**: `Scope` teardown collects retired `ReactiveId`s, removes values from `TypedSlab<T>` via `drop_slot` (returning boxed payloads if needed so user `Drop` impls run outside graph borrows), and pushes indices onto `free`.

---

## 3. Exact Capability Context Contracts

### 3.1 `ReadCx<'a>`
Passed to `memo(|r| ...)`, dynamic prop/style bindings `|r| ...`, `show(|r| ...)`, `watch` source `|r| ...`.
```rust
pub struct ReadCx<'a> {
    world: &'a mut ReactiveWorld,
    observer: Option<ReactiveId>, // None inside untrack()
    epoch: u64,                   // O(1) edge dedupe per evaluation
}

impl ReadCx<'_> {
    #[track_caller] pub fn get<T: Clone + 'static>(&mut self, sig: impl IntoRead<T>) -> T;
    #[track_caller] pub fn with<T: 'static, R>(&mut self, sig: impl IntoRead<T>, f: impl FnOnce(&T) -> R) -> R;
    #[track_caller] pub fn with2<A: 'static, B: 'static, R>(&mut self, a: impl IntoRead<A>, b: impl IntoRead<B>, f: impl FnOnce(&A, &B) -> R) -> R;
    #[track_caller] pub fn len<K: Eq + Hash + 'static, T: 'static>(&mut self, list: impl IntoReadList<K, T>) -> usize;
    #[track_caller] pub fn get_row<K: Eq + Hash + 'static, T: Clone + 'static>(&mut self, list: impl IntoReadList<K, T>, key: &K) -> Option<T>;
    #[track_caller] pub fn with_row<K: Eq + Hash + 'static, T: 'static, R>(&mut self, list: impl IntoReadList<K, T>, key: &K, f: impl FnOnce(Option<&T>) -> R) -> R;
    #[track_caller] pub fn text(&mut self, buf: TextBuffer) -> String;
    pub fn untrack<R>(&mut self, f: impl FnOnce(&mut ReadCx<'_>) -> R) -> R;
}
```
- **Compile-time guarantee**: `ReadCx` has **NO** `set`, `update`, `push`, `remove`, `signal`, `memo`, `watch`, `spawn`, `provide`, or `shutdown` methods.
- **How `with()` stays zero-`RefCell`**: `ensure_clean(sig)` runs FIRST with `&mut self.world`; once clean, the `&T` reference is borrowed immutably from `self.world.slabs` and passed to `f(&T)` — `f` does not receive `self`, so aliasing is impossible.

### 3.2 `BuildCx<'a>`
Passed to `#[ui]` components, `AppDefinition` root, `show` / `each` / `virtual_each` / `overlay` factories.
```rust
pub struct BuildCx<'a> {
    world: &'a mut ReactiveWorld,
    scope: ScopeId,
    window: Option<WindowId>,
    tx: Option<&'a mut TxBuilder>,
}

impl BuildCx<'_> {
    #[track_caller] pub fn signal<T: 'static>(&mut self, init: T) -> Signal<T>;
    #[track_caller] pub fn persisted<T: Serialize + DeserializeOwned + 'static>(&mut self, key: &'static str, init: impl FnOnce() -> T) -> Signal<T>;
    #[track_caller] pub fn memo<T: PartialEq + 'static>(&mut self, f: impl FnMut(&mut ReadCx<'_>) -> T + 'static) -> ReadSignal<T>;
    #[track_caller] pub fn list<K: Clone + Eq + Hash + 'static, T: 'static>(&mut self) -> StateList<K, T>;
    #[track_caller] pub fn text_buffer(&mut self, init: impl Into<String>) -> TextBuffer;
    #[track_caller] pub fn watch<S: PartialEq + 'static>(
        &mut self,
        source: impl Fn(&mut ReadCx<'_>) -> S + 'static,
        effect: impl Fn(&mut WriteCx<'_>, &S, Option<&S>) + 'static,
    ) -> Watch;
    #[track_caller] pub fn provide<T: Clone + 'static>(&mut self, value: T);
    #[track_caller] pub fn context<T: Clone + 'static>(&self) -> Option<T>;
    #[track_caller] pub fn sample<T: Clone + 'static>(&mut self, sig: impl IntoRead<T>) -> T; // explicit untracked read
    #[track_caller] pub fn on_cleanup(&mut self, f: impl FnOnce(&mut CleanupCx<'_>) + 'static);
    #[track_caller] pub fn spawn<Fut>(&mut self, label: &'static str, f: impl FnOnce(TaskCx) -> Fut + 'static) where Fut: Future<Output = ()> + 'static;
    pub fn window(&self) -> WindowId;
    pub fn viewport(&self) -> ReadSignal<Size>;
    pub fn breakpoint(&self) -> ReadSignal<Breakpoint>;
}
```
- **Compile-time guarantee**: `BuildCx` has **NO** `set`/`update` and **NO** tracked `get` (explicit `read()` or `sample()`).

A `BuildCx::read` closure takes a one-time untracked read of any reactive value, including lists and text buffers. `BuildCx::binding` returns a `Copy` handle that pairs a tracked reader with a scope-owned writer. Signals also convert directly to bindings.

Use `BuildCx::derive_list` when keyed rows come from general reactive reads. The list emits keyed edits through the graph and retains row identity. Prefer list operators for transforming an existing list incrementally.

`BuildCx::menus` contributes reactive menus from the current window scope. Publish sends the active window's menu after a successful turn. `BuildCx::on_chord` registers a scope-owned window shortcut. The focused control can consume the key first. The host resolves `mod` to Command on macOS or Control elsewhere.

### 3.3 `WriteCx<'a>`
Passed to event handlers, `watch` callbacks, `TaskCx::write`, `App::update`.
```rust
pub struct WriteCx<'a> {
    app: &'a mut AppRuntime,
    scope: ScopeId,
    window: Option<WindowId>,
}

impl WriteCx<'_> {
    // Reads (lazily ensures dirty upstream Memos / ListViews are fresh!)
    #[track_caller] pub fn get<T: Clone + 'static>(&mut self, sig: impl IntoRead<T>) -> T;
    #[track_caller] pub fn with<T: 'static, R>(&mut self, sig: impl IntoRead<T>, f: impl FnOnce(&T) -> R) -> R;
    #[track_caller] pub fn len<K: Eq + Hash + 'static, T: 'static>(&mut self, list: impl IntoReadList<K, T>) -> usize;
    #[track_caller] pub fn get_row<K: Eq + Hash + 'static, T: Clone + 'static>(&mut self, list: impl IntoReadList<K, T>, key: &K) -> Option<T>;

    // Writes (mutably borrows `&mut self` -> compile-time protection against re-entrant borrows)
    #[track_caller] pub fn set<T: 'static>(&mut self, sig: Signal<T>, value: T);
    #[track_caller] pub fn set_if_changed<T: PartialEq + 'static>(&mut self, sig: Signal<T>, value: T) -> bool;
    #[track_caller] pub fn update<T: 'static, R>(&mut self, sig: Signal<T>, f: impl FnOnce(&mut T) -> R) -> R;

    // Collection & TextBuffer mutations
    #[track_caller] pub fn insert_row<K: Clone + Eq + Hash + 'static, T: 'static>(&mut self, list: StateList<K, T>, index: usize, key: K, item: T);
    #[track_caller] pub fn push_row<K: Clone + Eq + Hash + 'static, T: 'static>(&mut self, list: StateList<K, T>, key: K, item: T);
    #[track_caller] pub fn update_row<K: Clone + Eq + Hash + 'static, T: 'static>(&mut self, list: StateList<K, T>, key: &K, f: impl FnOnce(&mut T));
    #[track_caller] pub fn remove_row<K: Clone + Eq + Hash + 'static, T: 'static>(&mut self, list: StateList<K, T>, key: &K) -> Option<T>;
    #[track_caller] pub fn move_row<K: Clone + Eq + Hash + 'static, T: 'static>(&mut self, list: StateList<K, T>, key: &K, to: usize);
    #[track_caller] pub fn reconcile_list<K: Clone + Eq + Hash + 'static, T: PartialEq + 'static>(&mut self, list: StateList<K, T>, rows: Vec<(K, T)>);
    #[track_caller] pub fn append_text(&mut self, buf: TextBuffer, text: &str);
    #[track_caller] pub fn splice_text(&mut self, buf: TextBuffer, edit: TextEdit);

    // Window / App commands (enqueued for Commit/Publish, never directly mutating Tree)
    pub fn focus(&mut self, test_id: &str);
    pub fn blur(&mut self);
    pub fn request_command(&mut self, name: &str);
    pub fn reveal_row<K: Eq + Hash + Display + 'static>(&mut self, container_id: &str, key: K);
    pub fn open_window(&mut self, options: WindowOptions, root: impl Fn(&mut BuildCx<'_>) -> View + 'static) -> WindowId;
    pub fn close_window(&mut self, id: WindowId);
    pub fn spawn<Fut>(&mut self, label: &'static str, f: impl FnOnce(TaskCx) -> Fut + 'static) where Fut: Future<Output = ()> + 'static;
    pub fn ask(&mut self, ask: Ask, on_answer: impl FnOnce(&mut WriteCx<'_>, Answer) + 'static);
    pub fn request_service(&mut self, req: ServiceRequest, on_done: impl FnOnce(&mut WriteCx<'_>, ServiceOutcome) + 'static);
}
```

### 3.4 `TaskCx` (Borrow-Safe Async Tasks)
Tasks run on the UI thread at Turn boundaries. `TaskCx` is a small handle that opens a scoped `WriteCx` **only synchronously between `.await` points**:
```rust
impl TaskCx {
    /// Runs `f` synchronously with full WriteCx capabilities. Returns None if task's scope was retired.
    pub fn write<R>(&self, f: impl FnOnce(&mut WriteCx<'_>) -> R) -> Option<R>;
    pub fn sleep(&self, duration: Duration) -> Sleep; // driven by VirtualClock
    pub fn now(&self) -> Duration;
    #[cfg(not(target_family = "wasm"))]
    pub fn background<T: Send + 'static>(&self, f: impl FnOnce() -> T + Send + 'static) -> Background<T>;
}
```
Implementation note: `TaskScheduler` passes a raw non-null scoped pointer (or temporary TLS slot strictly scoped to a single `Future::poll` call — never exposed outside `TaskCx::write` and invalidated immediately on poll exit) so `Future`s remain zero-overhead standard Rust `async` blocks while guaranteeing `&mut WriteCx` can never escape or cross `.await`.

---

## 4. Unified Pull-on-Demand Value + Delta Graph Algorithm

### 4.1 Node Header
```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeState { Clean, Check, Dirty }

pub enum NodeKind {
    ValueSource,
    Memo,
    ListSource,
    ListOp,
    TextBuffer,
    Structure,
    Binding,
    Watch,
}

struct ReactiveNode {
    gen: u32,
    alive: bool,
    kind: NodeKind,
    state: NodeState,
    computing: bool, // cycle detection guard
    depth: u32,      // invariant: depth > max(source.depth)
    rev: u64,        // monotonic revision bumped only when output actually changes
    scope: ScopeId,
    owner_window: Option<WindowId>,
    sources: SmallVec<[ReactiveId; 4]>,
    observers: SmallVec<[ReactiveId; 4]>,
    tracked_epoch: (ReactiveId, u64), // O(1) edge dedupe
    defined_at: &'static Location<'static>,
}
```

### 4.2 Push-Marking Algorithm (`mark_dirty(source_id)`)
When any `ValueSource`, `ListSource`, or `TextBuffer` changes:
1. Bump `node.rev += 1`.
2. For each direct observer `obs` of `source_id`:
   - If `obs.state != Dirty`, set `obs.state = Dirty` and push `obs` to BFS queue.
3. Transitive propagation: for each popped `n` and each observer `obs` of `n`:
   - If `obs.state == Clean`, set `obs.state = Check` and push `obs` to queue.
4. Enqueue any touched `Structure`, `Binding`, or `Watch` observer into `world.pending_observers` (a `BTreeMap<(FlushTier, u32), Vec<ReactiveId>>` ordered by `(tier, depth)` where `FlushTier = Structure < Binding < Watch`).

### 4.3 Pull-Evaluation Algorithm (`ensure_clean(id)`)
Invoked identically by `ReadCx::get/with/len/get_row`, `WriteCx::get/len/get_row`, and `flush()`:
```text
fn ensure_clean(world, id):
    node = world.node(id)
    if node.state == Clean: return
    if node.computing: report(CYCLE); return

    // Step 1: If Check, verify whether any upstream source actually bumped its rev
    if node.state == Check:
        for (src, seen_rev) in node.source_revs:
            ensure_clean(world, src)
            if world.node(src).rev != seen_rev:
                node.state = Dirty
                break
        if node.state == Check:
            node.state = Clean
            return

    // Step 2: Dirty -> recompute according to kind
    node.computing = true
    match node.kind:
        Memo:
            run compute(&mut ReadCx { observer: Some(id) })
            if new_val != old_val:
                replace value in TypedSlab<T>; node.rev += 1
        ListOp:
            // Pull upstream DeltaLog(s) from cursor(s) + any Value param(s)
            changed = op.catch_up(world)
            if changed:
                node.rev += 1
        _: ()
    node.computing = false
    node.state = Clean
    update node.depth = max(src.depth) + 1 (propagating depth bumps down if dynamic edges deepened)
```
**Proof of Glitch-Freedom & Synchronous Freshness**:
- **Glitch-freedom**: During `ensure_clean`, all upstream sources are guaranteed `Clean` before the dependent recomputes. During `flush`, observers execute in ascending `(FlushTier, depth)`.
- **Synchronous freshness**: Reading `ReadList` immediately after `w.push_row(list, k, v)` calls `ensure_clean(read_list)` which catches up `DeltaLog` in `O(log n)` — identical to `Memo`, eliminating v1's eager `CollectionQueue`.
- **Batch coalescing**: 100 consecutive `push_row` calls append 100 lightweight `ListDelta::Insert(k)` entries and mark downstream `Dirty` once (`O(1)` after the 1st push); downstream processes all 100 in a single pass.

### 4.4 Key-Only `DeltaLog<ListDelta<K>>` & Incremental Operators

```rust
pub const DEFAULT_DELTA_LOG_CAP: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListDelta<K> {
    Insert(K),
    Remove(K),
    Move { key: K, to: usize },
    Update(K),
    Reset,
}
```

| Operator | Internal Index State | On `ListDelta<K>` | On Value Param Change (`*_with`) | Complexity |
| --- | --- | --- | --- | --- |
| `filter_with(cx, param, pred)` | `seq: Seq<K>`, `included: FxHashSet<K>` | `Insert/Update(k)`: test `pred(up[k], param)` → emit flip; `Remove(k)`: evict if included; `Move`: mirror rank | Sync pending list deltas; re-test `pred(up[k], new_param)` across upstream; emit **only** flipped `Insert`/`Remove` | `O(k log n)` list / `O(n log n)` param |
| `sort_by_with(cx, param, key_fn)` | `seq: Seq<K>`, `keys: FxHashMap<K, SK>`, `order: BTreeMap<(SK, K), ()>` | `Insert(k)`: compute `sk`, insert; `Update(k)`: look up old `sk` in `self.keys`, compute new `sk` from `&up[k]`, emit `Move` if rank changed + `Update(k)`; `Remove(k)`: evict via `self.keys.remove(&k)` | Sync pending list deltas; recompute `SK`, emit minimal `Move`s | `O(k log n)` list |
| `map_with(cx, param, proj)` | `seq: Seq<K>`, `values: FxHashMap<K, U>` | Mirror topology; `Insert/Update(k)`: compute `proj(&up[k], param)` | Re-project & emit `Update(k)` where `PartialEq` differs | `O(k log n)` |
| `group_by` + `flatten` | Weighted `Seq<G>` for group headers, per-group `Seq<K>` for members, keyed membership, flat `Seq<FlatKey<G, K>>` | Look up the prior group and compute the new group from `&up[k]`. Search the group by fixed-size upstream order labels and maintain the flat index and group headers. | N/A | Expected `O(log n)` under the treap priority model |
| `join(left, right, jk_fn)` | `left_jk: FxHashMap<K, JK>`, `rev_idx: FxHashMap<JK, SmallVec<[K; 2]>>` | Left delta: update `rev_idx` & join value; Right `Insert/Update/Remove(jk)`: iterate `rev_idx[jk]`, emit `Update(k)` (diagnose `JOIN_FANOUT` if fanout exceeds threshold) | N/A | `O(log n + fanout)` |
| Per-key `RowSignal` | `row_observers: FxHashMap<K, SmallVec<[ReactiveId; 2]>>` | On `Update(k)`: dirty **only** `row_observers[k]` | — | `O(1)` |

---

## 5. Five-Phase Turn Engine (`turn.rs`)

```text
Turn::run(app, incoming_event):
  assert!(!app.failed);
  arm panic_latch_guard(app);

  // PHASE 1: INGRESS
  let routed = ingress(app, incoming_event); // validates TargetGuard / EditorSession

  for round in 0..WINDOW_ROUNDS (16):
      // PHASE 2: REACT (Tree is NOT mutably borrowed; all view writes buffer into TxBuilder)
      run_routed_handlers_and_ready_tasks(app, &mut write_cx);
      flush_reactive_graph(app):
          2a. ensure_clean dirty Derivations (Memo + ListOp) in depth order
          2b. run dirty Structure regions (show / each / virtual_each) -> BuildCx -> TxBuilder
          2c. run dirty Bindings on surviving nodes -> TxBuilder
          2d. evaluate dirty Watch sources -> enqueue post-commit effects

      // PHASE 3: COMMIT (ONE atomic Transaction per dirty window)
      for win in dirty_windows:
          let (receipt, delta) = win.tree.apply(win.tx_builder.take())?;
          win.activate_pending_bindings_and_handlers(receipt);
          drop(win.tree.take_retired_assets()); // ALWAYS after tree borrow ends
          win.accumulated_delta.merge(delta);

      run_queued_watch_effects(app); // bounded

      // PHASE 4: PROJECT (Pluggable HostEngine: NativeEngine or DomEngine)
      let mut any_feedback = false;
      for win in windows_with_pending_delta:
          match win.engine.project(&win.tree, &win.accumulated_delta, win.viewport, app.clock.now()):
              Ok(feedback) =>
                  win.apply_geometry_feedback(feedback, &mut write_cx); // size_of, virtual_each, scroll reveal, focus settle
                  any_feedback |= feedback.dirtied_signals;
              Err(err) =>
                  win.mark_project_failed(err);

      if !any_feedback && !app.has_pending_work():
          break;

  // PHASE 5: PUBLISH
  for win in app.windows:
      if converged && win.project_ok:
          let commit = win.engine.commit();
          win.settled_rev = Some(win.current_rev());
          win.advance_observation_receipt(commit);
      else:
          win.engine.rollback();
          win.settled_rev = None;
  present_menus(app);
```

### Atomic Subtree Batching in `El` / `View`
- `El` stores inline styles, overrides, and children in `SmallVec` to minimize mount-time heap allocations.
- `mount(cx: &mut BuildCx, view: View, parent: NodeRef, slot: SlotPos)`:
  1. Allocates `pending = cx.tx.create(NodeSpec { ... })`.
  2. Writes **ALL** `Prop::Static`, static styles, overrides, and `policies` directly onto `NodeRef::Pending(pending)` in the **same** `TxBuilder`.
  3. Recursively mounts static children into the **same** `TxBuilder` using `NodeRef::Pending(pending)` as parent!
  4. Registers dynamic `Prop::Dyn` bindings and handlers in `cx.pending_activations` keyed by `pending` — activated immediately after Phase 3 `Commit` maps `pending -> NodeId`!
  5. Result: Mounting a 200-node subtree = **1 atomic `Transaction`**, **1 `TreeRev` increment**, **1 Preflight pass**.

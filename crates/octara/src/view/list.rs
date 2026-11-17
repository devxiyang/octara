//! Keyed rows from ordinary snapshots or incremental collections, with shared row ownership.

use super::{Mount, Parent, Slot, Slots, View, build_el, col, count_offset};
use crate::diagnostics as codes;
use crate::kernel::NodeId;
use crate::reactive::graph::{Ctx, ReadSignal, ScopeId, Signal, memo, memo_with, rt, untrack};
use crate::reactive::list::{Change, Observer, ReadList};
use crate::reactive::log::{Changes, Cursor};
use crate::reactive::seq::Seq;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::hash::Hash;
use std::panic::Location;
use std::rc::Rc;

/// A supported list input: an incremental [`ReadList`] or ordinary keyed `Vec<(K, T)>`.
/// Keys identify logical rows within this region; values may change without rebuilding them.
/// This conversion trait is sealed. It introduces no application-owned collection state.
pub trait IntoRows<K, T>: source::Sealed<K, T> {}

mod source {
    use super::*;

    pub trait Sealed<K, T> {
        fn into_input(self, at: &'static Location<'static>) -> Input<K, T>;
    }

    pub enum Input<K: 'static, T: 'static> {
        Incremental(ReadList<K, T>),
        Snapshot(Rc<Snapshot<K, T>>),
    }

    pub struct Snapshot<K, T> {
        keys: Vec<K>,
        items: HashMap<K, T>,
    }

    impl<K: Eq + Hash + Clone + Display + 'static, T: Clone + 'static> Sealed<K, T> for Vec<(K, T)> {
        fn into_input(self, at: &'static Location<'static>) -> Input<K, T> {
            let mut keys = Vec::with_capacity(self.len());
            let mut items = HashMap::with_capacity(self.len());
            for (key, value) in self {
                if items.insert(key.clone(), value).is_some() {
                    rt(|rt| rt.diag(&codes::DUPLICATE_KEY, format!("duplicate row key `{key}`; the entire snapshot is unavailable"), at));
                    // Keep the computation and its dependencies alive: a valid later snapshot
                    // recovers, but no previously published row stays actionable in between.
                    keys.clear();
                    items.clear();
                    break;
                }
                keys.push(key);
            }
            Input::Snapshot(Rc::new(Snapshot { keys, items }))
        }
    }

    impl<K: Eq + Hash + Clone + Display + 'static, T: Clone + 'static> IntoRows<K, T> for Vec<(K, T)> {}
    impl<K: 'static, T: 'static> Sealed<K, T> for ReadList<K, T> {
        fn into_input(self, _: &'static Location<'static>) -> Input<K, T> {
            Input::Incremental(self)
        }
    }
    impl<K: 'static, T: 'static> IntoRows<K, T> for ReadList<K, T> {}

    impl<K, T> Clone for Input<K, T> {
        fn clone(&self) -> Self {
            match self {
                Self::Incremental(list) => Self::Incremental(list.clone()),
                Self::Snapshot(snapshot) => Self::Snapshot(snapshot.clone()),
            }
        }
    }
    impl<K, T> PartialEq for Input<K, T> {
        fn eq(&self, other: &Self) -> bool {
            match (self, other) {
                (Self::Incremental(a), Self::Incremental(b)) => a == b,
                (Self::Snapshot(a), Self::Snapshot(b)) => Rc::ptr_eq(a, b),
                _ => false,
            }
        }
    }
    impl<K: Eq + Hash + Clone + 'static, T: Clone + 'static> Input<K, T> {
        pub(super) fn track(&self) {
            if let Self::Incremental(list) = self {
                list.track();
            }
        }
        pub(super) fn subscribe(&self) -> Option<Cursor> {
            match self {
                Self::Incremental(list) => Some(list.subscribe()),
                Self::Snapshot(_) => None,
            }
        }
        pub(super) fn take(&self, cursor: &Option<Cursor>) -> Changes<Change<K>> {
            match (self, cursor) {
                (Self::Incremental(list), Some(cursor)) => list.take(cursor),
                _ => Changes::Delta(Vec::new()),
            }
        }
        pub(super) fn snapshot(&self) -> Vec<K> {
            match self {
                Self::Incremental(list) => list.snapshot(),
                Self::Snapshot(snapshot) => snapshot.keys.clone(),
            }
        }
        pub(super) fn read(&self, key: &K) -> Option<T> {
            match self {
                Self::Incremental(list) => list.item(key).map(|item| item.get()),
                Self::Snapshot(snapshot) => snapshot.items.get(key).cloned(),
            }
        }
    }
}
use source::Input;

/// Both renderers own exactly the same key -> scope/node mapping and live row input.
/// Virtualization only decides which keys reside and how their nodes are placed.
type RowView<K, T> = dyn Fn(&K, ReadSignal<T>) -> View;

struct Rows<K: 'static, T: 'static> {
    entries: HashMap<K, (ScopeId, NodeId)>,
    input: ReadSignal<Input<K, T>>,
    owner: ScopeId,
    base: String,
    at: &'static Location<'static>,
    row: Box<RowView<K, T>>,
}

impl<K: Eq + Hash + Clone + Display + 'static, T: Clone + 'static> Rows<K, T> {
    fn build(&self, key: &K, offscreen: bool) -> (ScopeId, NodeId) {
        let base = if offscreen { format!("{}/offscreen", self.base) } else { self.base.clone() };
        let input = self.input;
        rt(|rt| {
            let scope = rt.create_scope(Some(self.owner), &format!("{base}/row:{key}"));
            let node = rt.with_frame(Ctx::Build, None, Some(Some(scope)), || {
                let k = key.clone();
                let at = self.at;
                // Resolve the current source and key on every invalidated read, including a
                // read immediately after a model write in the same handler. No UI flush.
                let item = memo_with(
                    move || {
                        input.with(|input| input.read(&k)).unwrap_or_else(|| {
                            crate::reactive::graph::rt(|rt| {
                                rt.diag(&codes::LIST_ITEM_UNAVAILABLE, format!("row `{k}` is absent from its current input"), at)
                            });
                            panic!("[{}] row `{k}` is unavailable", codes::LIST_ITEM_UNAVAILABLE);
                        })
                    },
                    |_, _| false,
                );
                match (self.row)(key, item) {
                    View::El(e) => build_el(*e),
                    _ => {
                        rt.diag(&codes::EACH_ROW_NOT_ELEMENT, "a list row must be one element".into(), self.at);
                        build_el(col())
                    }
                }
            });
            crate::app::window().kernel.borrow_mut().set_row_key(node, Some(key.to_string().into()));
            (scope, node)
        })
    }

    fn ensure(&mut self, key: &K) -> (ScopeId, NodeId) {
        if let Some(row) = self.entries.get(key) {
            return *row;
        }
        let row = self.build(key, false);
        self.entries.insert(key.clone(), row);
        row
    }

    fn remove(&mut self, key: &K, exit: bool) {
        if let Some((scope, node)) = self.entries.remove(key) {
            if exit {
                crate::app::window().kernel.borrow_mut().begin_exit(node);
            }
            rt(|rt| rt.dispose_scope(scope));
        }
    }

    fn reconcile(&mut self, keys: &[K], exit: bool) -> bool {
        let wanted: HashSet<_> = keys.iter().collect();
        let removed: Vec<_> = self.entries.keys().filter(|key| !wanted.contains(key)).cloned().collect();
        let mut changed = !removed.is_empty();
        for key in removed {
            self.remove(&key, exit);
        }
        for key in keys {
            changed |= !self.entries.contains_key(key);
            self.ensure(key);
        }
        changed
    }
}

/// Places `nodes` as consecutive kernel children starting at `base_index`.
fn place(parent: NodeId, base_index: usize, nodes: impl Iterator<Item = NodeId>) {
    let u = crate::app::window();
    let mut kn = u.kernel.borrow_mut();
    for (i, node) in nodes.enumerate() {
        let want = base_index + i;
        if kn.parent(node).is_some() {
            if kn.children(parent).get(want) != Some(&node) {
                kn.move_child(node, want);
            }
        } else {
            kn.insert_child(parent, node, want);
        }
    }
}

/// A region's current read-only list and position in its change log.
type Current<K, T> = Rc<RefCell<Option<(Input<K, T>, Option<Cursor>)>>>;

/// Renders `list()`. While it stays the same stage, its log is applied row by row: one row
/// created per insert, one disposed per remove, one moved per move. Switching to another stage
/// reconciles by key once. A row must be one element; it receives its item's own signal.
#[track_caller]
pub fn each<K, T, S: IntoRows<K, T> + 'static>(list: impl Fn() -> S + 'static, row: impl Fn(&K, ReadSignal<T>) -> View + 'static) -> View
where
    K: Eq + Hash + Clone + Display + 'static,
    T: Clone + 'static,
{
    let at = Location::caller();
    let made = super::building();
    let row = move |k: &K, t: ReadSignal<T>| super::as_part_of(made, || row(k, t));
    View::Dyn(Mount(Box::new(move |parent: &Parent, target: &Slots| {
        let count: Rc<Cell<usize>> = Rc::default();
        target.borrow_mut().push(Slot::Count(count.clone()));
        let (owner, base) = rt(|rt| {
            let owner = rt.current_scope().expect("each outside a scope");
            (owner, rt.child_name(owner, "each"))
        });
        let parent = parent.clone();
        let input = memo(move || list().into_input(at));
        let rows = Rc::new(RefCell::new(Rows { entries: HashMap::new(), input, owner, base, at, row: Box::new(row) }));
        let current: Current<K, T> = Rc::default();
        crate::reactive::graph::structural_binding(at, move || {
            let next = input.get();
            next.track();
            rt(|rt| {
                rt.with_frame(Ctx::Observe, None, Some(Some(owner)), || {
                    let u = crate::app::window();
                    let same = current.borrow().as_ref().is_some_and(|(s, _)| *s == next);
                    if same {
                        let changes = {
                            let cur = current.borrow();
                            let (s, c) = cur.as_ref().unwrap();
                            s.take(c)
                        };
                        if let Changes::Delta(changes) = changes {
                            // A key inserted and removed before propagation has no current value
                            // to build. Reconcile the final snapshot, still honoring retirements.
                            let removed: HashSet<_> = changes
                                .iter()
                                .filter_map(|change| match change {
                                    Change::Remove { key, .. } => Some(key),
                                    _ => None,
                                })
                                .collect();
                            if changes.iter().any(|change| matches!(change, Change::Insert { key, .. } if removed.contains(key))) {
                                for change in &changes {
                                    if let Change::Remove { key, .. } = change {
                                        rows.borrow_mut().remove(key, true);
                                    }
                                }
                                let keys = next.snapshot();
                                rows.borrow_mut().reconcile(&keys, true);
                                place(parent.node, count_offset(&parent, &count), keys.iter().map(|key| rows.borrow().entries[key].1));
                                count.set(keys.len());
                                return;
                            }
                            for ch in changes {
                                let base_index = count_offset(&parent, &count);
                                match ch {
                                    Change::Insert { key, index } => {
                                        let (_, node) = rows.borrow_mut().ensure(&key);
                                        u.kernel.borrow_mut().insert_child(parent.node, node, base_index + index);
                                        count.set(count.get() + 1);
                                    }
                                    Change::Remove { key, .. } => {
                                        rows.borrow_mut().remove(&key, true);
                                        count.set(count.get() - 1);
                                    }
                                    Change::Move { key, to, .. } => {
                                        if let Some((_, node)) = rows.borrow().entries.get(&key) {
                                            u.kernel.borrow_mut().move_child(*node, base_index + to);
                                        }
                                    }
                                }
                            }
                            return;
                        }
                    }
                    let cursor = next.subscribe();
                    let keys = next.snapshot();
                    rows.borrow_mut().reconcile(&keys, true);
                    place(parent.node, count_offset(&parent, &count), keys.iter().map(|key| rows.borrow().entries[key].1));
                    count.set(keys.len());
                    *current.borrow_mut() = Some((next, cursor));
                })
            });
        });
    })))
}

// ---- virtualization ----

/// Rows kept beyond each edge of the visible area.
/// A reading position belongs to a business key, not a mutable numeric index.
#[derive(Clone)]
struct RowAnchor<K> {
    key: K,
    offset: f64,
}

impl<K: Eq + Hash + Clone> RowAnchor<K> {
    fn at(rows: &Seq<K>, y: f64) -> Option<Self> {
        if y <= 0.0 {
            return None;
        }
        let index = rows.index_at_weight(y).min(rows.len().saturating_sub(1));
        Some(Self { key: rows.nth(index)?.clone(), offset: y - rows.prefix_weight(index) })
    }

    fn position(&self, rows: &Seq<K>) -> Option<f64> {
        let index = rows.index_of(&self.key)?;
        Some((rows.prefix_weight(index) + self.offset).max(0.0))
    }

    fn removing(&mut self, key: &K, rows: &Seq<K>) {
        if &self.key == key
            && let Some(index) = rows.index_of(key)
            && let Some(next) = rows.nth(index + 1).or_else(|| index.checked_sub(1).and_then(|i| rows.nth(i)))
        {
            self.key = next.clone();
        }
    }

    fn surviving(&mut self, old: &Seq<K>, next: &Seq<K>) {
        if !next.contains(&self.key)
            && let Some(index) = old.index_of(&self.key)
            && let Some(key) = (index + 1..old.len()).chain((0..index).rev()).filter_map(|i| old.nth(i)).find(|key| next.contains(key))
        {
            self.key = key.clone();
        }
    }
}

/// Value invalidation reuses collection observers without waking structural readers or
/// retaining a reactive computation for every offscreen row. Removed keys leave this set.
struct ItemChanges<K> {
    keys: Rc<RefCell<HashSet<K>>>,
    revision: Signal<u64>,
}

impl<K: Eq + Hash + Clone, T> Observer<K, T> for ItemChanges<K> {
    fn inserted(&self, _: usize, _: &K, _: &T) {}
    fn removed(&self, _: usize, key: &K, _: &T) {
        self.keys.borrow_mut().remove(key);
    }
    fn moved(&self, _: &K, _: usize, _: usize, _: &T) {}
    fn updated(&self, key: &K, _: &T, _: &T) {
        if self.keys.borrow_mut().insert(key.clone()) {
            self.revision.set_internal(untrack(|| self.revision.get()).wrapping_add(1));
        }
    }
}

const OVERSCAN: usize = 5;

/// Like [`each`], for long lists: only rows near the visible area exist. Put it directly in a
/// scroll container (`v_flex().overflow_y_scroll().flex_1()`); it follows that container's
/// scroll offset and height.
///
/// Row heights start from `estimate` and are replaced by each row's measured height once it
/// has been laid out; the first visible row stays put while rows above it are measured
/// (scroll anchoring). Rows are kept in a weighted order-statistic tree, so offset → row and
/// row → offset are O(log n). Ordinary incremental edits and scrolling update only affected
/// and resident rows. A new snapshot or changed layout constraints can reset O(n) cached
/// weights, without mounting the offscreen rows. The
/// edge spacers are `{parent id}-top` / `-bottom`. Focus and pointer capture temporarily keep
/// their rows resident; hidden gap spacers preserve their positions without mounting the
/// intervening rows.
#[track_caller]
pub fn virtual_each<K, T, S: IntoRows<K, T> + 'static>(
    list: impl Fn() -> S + 'static,
    estimate: impl Fn(&T) -> f32 + 'static,
    row: impl Fn(&K, ReadSignal<T>) -> View + 'static,
) -> View
where
    K: Eq + Hash + Clone + Display + 'static,
    T: Clone + 'static,
{
    let at = Location::caller();
    let made = super::building();
    let row = move |k: &K, t: ReadSignal<T>| super::as_part_of(made, || row(k, t));
    View::Dyn(Mount(Box::new(move |parent: &Parent, target: &Slots| {
        let count: Rc<Cell<usize>> = Rc::default();
        target.borrow_mut().push(Slot::Count(count.clone()));
        let (owner, base) = rt(|rt| {
            let owner = rt.current_scope().expect("virtual_each outside a scope");
            (owner, rt.child_name(owner, "each"))
        });
        let parent = parent.clone();
        let container = parent.node;
        let u = crate::app::window();
        if !u.kernel.borrow().scrollable(container).1 {
            rt(|rt| {
                rt.diag(&codes::VIRTUAL_LIST_NOT_SCROLLABLE, "virtual_each must sit directly in a scroll container: give its parent `overflow_y_scroll()` and a bounded height (e.g. `flex_1()` in a `size_full()` column)".into(),
                    at,
                )
            });
        }
        let prefix = u.kernel.borrow().test_id(container).map(str::to_owned);
        let spacer = {
            let (u, prefix) = (u.clone(), prefix.clone());
            move |side: &str| {
                let mut k = u.kernel.borrow_mut();
                let n = k.create(crate::kernel::Kind::Box);
                k.set_hidden(n, true);
                // Spacers stand for rows: they must not shrink in the scroll container's column.
                k.set_style(n, crate::kernel::StyleProp::Shrink(0.0));
                if let Some(p) = &prefix {
                    k.set_test_id(n, Some(&format!("{p}-{side}")));
                }
                n
            }
        };
        let (top, bottom) = (spacer("top"), spacer("bottom"));
        let gaps: Rc<RefCell<Vec<NodeId>>> = Rc::default();
        let scroll = u.scroll_state(container);
        let revision = Signal::scoped(0u64);
        let row_gap = Rc::new(Cell::new(u.kernel.borrow().row_gap(container) as f64));
        let list_top: Rc<Cell<f32>> = Rc::default();
        let mirror: Rc<RefCell<Seq<K>>> = Rc::default();
        let input = memo(move || list().into_input(at));
        let rows = Rc::new(RefCell::new(Rows { entries: HashMap::new(), input, owner, base, at, row: Box::new(row) }));
        let measured: Rc<RefCell<HashSet<K>>> = Rc::default();
        let estimates: Rc<RefCell<HashMap<K, f64>>> = Rc::default();
        let pending_anchor: Rc<RefCell<Option<RowAnchor<K>>>> = Rc::default();
        let anchor_request = Rc::new(Cell::new(0));
        let dirty_items: Rc<RefCell<HashSet<K>>> = Rc::default();
        let item_observer = Rc::new(RefCell::new(None));
        let observer_cleanup = item_observer.clone();
        rt(|rt| {
            rt.on_cleanup(Box::new(move || {
                observer_cleanup.borrow_mut().take();
            }))
        });

        let current: Current<K, T> = Rc::default();
        // Keys by the name paths write them in (every row built, and every key listed).
        let names: Rc<RefCell<HashMap<String, K>>> = Rc::default();

        // How a selection through the list orders rows and reads the ones not mounted.
        {
            let (mirror, names, rows) = (mirror.clone(), names.clone(), rows.clone());
            let index_of = {
                let (mirror, names) = (mirror.clone(), names.clone());
                Rc::new(move |name: &str| names.borrow().get(name).and_then(|k| mirror.borrow().index_of(k)))
            };
            let keys = {
                let (mirror, names) = (mirror.clone(), names.clone());
                Rc::new(move |start: usize, end: usize| {
                    let m = mirror.borrow();
                    let mut names = names.borrow_mut();
                    (start..end.min(m.len()))
                        .filter_map(|i| m.nth(i))
                        .map(|k| {
                            let name = k.to_string();
                            names.entry(name.clone()).or_insert_with(|| k.clone());
                            Rc::<str>::from(name)
                        })
                        .collect::<Vec<_>>()
                })
            };
            let build = Rc::new(move |name: &str| -> Option<(NodeId, Box<dyn FnOnce()>)> {
                let k = names.borrow().get(name).cloned()?;
                input.with(|input| input.read(&k))?;
                let (scope, node) = rows.borrow().build(&k, true);
                Some((node, Box::new(move || rt(|rt| rt.dispose_scope(scope)))))
            });
            u.lists.borrow_mut().insert(container, crate::app::ListOrder { index_of, keys, build: Some(build) });
            let u = u.clone();
            rt(|rt| {
                rt.on_cleanup(Box::new(move || {
                    u.lists.borrow_mut().remove(&container);
                }))
            });
        }

        // After each layout: where the list starts in the container, and the rows' real heights.
        {
            let (u, list_top, mirror, rows, measured, row_gap, estimates) =
                (u.clone(), list_top.clone(), mirror.clone(), rows.clone(), measured.clone(), row_gap.clone(), estimates.clone());
            let pending_anchor = pending_anchor.clone();
            let anchor_request = anchor_request.clone();
            let constraints = RefCell::new(None);
            u.clone().after_layout(Rc::new(move || {
                let mut k = u.kernel.borrow_mut();
                if !k.is_alive(container) || !k.is_alive(top) {
                    return false;
                }
                let (sx, sy) = k.scroll_offset(container);
                let cy = k.content_origin_y(container);
                let start = k.rect(top).1 - cy + sy;
                let old_start = list_top.get();
                let mut changed = false;
                if (start - old_start).abs() > 0.5 {
                    list_top.set(start);
                    changed = true;
                }
                let mut m = mirror.borrow_mut();
                let y = (sy - old_start).max(0.0) as f64;
                let gap = k.row_gap(container) as f64;
                let gap_delta = gap - row_gap.replace(gap);
                changed |= gap_delta.abs() > 0.01;
                let signature = k.virtual_constraints(container);
                let invalid = constraints.borrow().as_ref().is_some_and(|previous| previous != &signature);
                *constraints.borrow_mut() = Some(signature);
                let pending = pending_anchor.borrow_mut().take().filter(|_| anchor_request.get() == k.scroll_request(container));
                let mut anchor = pending.clone().or_else(|| RowAnchor::at(&m, y));
                let rows = rows.borrow();
                let rows = &rows.entries;
                let mut measured = measured.borrow_mut();
                // Prefer an already displayed row when estimates are still converging.
                if pending.is_none()
                    && y > 0.0
                    && let Some(index) = rows
                        .keys()
                        .filter(|key| measured.contains(*key))
                        .filter_map(|key| m.index_of(key))
                        .filter(|&i| m.prefix_weight(i + 1) > y && m.prefix_weight(i) < y + k.client_size(container).1 as f64)
                        .min()
                {
                    anchor = Some(RowAnchor { key: m.nth(index).unwrap().clone(), offset: y - m.prefix_weight(index) });
                }
                if invalid || gap_delta.abs() > 0.01 {
                    for key in m.to_vec() {
                        let weight = if invalid { estimates.borrow()[&key] + gap } else { m.weight_of(&key).unwrap() + gap_delta };
                        m.set_weight(&key, weight);
                    }
                    if invalid {
                        measured.clear();
                    }
                    changed = true;
                }
                for (key, (_, node)) in rows {
                    if k.is_alive(*node) {
                        let weight = k.rect(*node).3 as f64 + gap;
                        if m.weight_of(key).is_some_and(|old| (old - weight).abs() > 0.5) {
                            m.set_weight(key, weight);
                            changed = true;
                        }
                    }
                }
                // Only a measured row can clamp a saved intra-row position. An estimate may
                // be much shorter than the paragraph the reader is halfway through.
                if let Some(anchor) = &mut anchor
                    && let Some((_, node)) = rows.get(&anchor.key)
                    && k.is_alive(*node)
                {
                    anchor.offset = anchor.offset.min((k.rect(*node).3 as f64 + gap - 1.0).max(0.0));
                }
                if (changed || pending.is_some())
                    && !k.is_following_tail(container)
                    && let Some(to) = anchor.as_ref().and_then(|anchor| anchor.position(&m))
                    && (to + start as f64 - sy as f64).abs() > 0.5
                {
                    k.scroll_to_unclamped(container, sx, (to as f32 + start).max(0.0));
                    changed = true;
                }
                measured.extend(rows.iter().filter(|(_, (_, node))| k.is_alive(*node)).map(|(key, _)| key.clone()));
                if changed {
                    revision.update(|r| *r += 1);
                }
                changed
            }));
        }
        {
            let (u, gaps) = (u.clone(), gaps.clone());
            rt(|rt| {
                rt.on_cleanup(Box::new(move || {
                    let mut k = u.kernel.borrow_mut();
                    for n in [top, bottom].into_iter().chain(gaps.borrow_mut().drain(..)) {
                        if k.is_alive(n) {
                            k.destroy(n);
                        }
                    }
                }))
            });
        }
        crate::reactive::graph::structural_binding(at, move || {
            let next = input.get();
            next.track();
            let state = scroll.get();
            revision.get();
            u.interaction_revision.get();
            let mut y = (u.kernel.borrow().scroll_offset(container).1 - list_top.get()).max(0.0) as f64;
            let height = state.client.1 as f64;
            rt(|rt| {
                rt.with_frame(Ctx::Observe, None, Some(Some(owner)), || {
                    if anchor_request.get() != u.kernel.borrow().scroll_request(container) {
                        pending_anchor.borrow_mut().take();
                    }
                    let mut anchor = pending_anchor.borrow().clone().or_else(|| RowAnchor::at(&mirror.borrow(), y));
                    let weight = |key: &K| {
                        let height = untrack(|| next.read(key)).map(|item| estimate(&item).max(1.0) as f64).unwrap_or(1.0);
                        estimates.borrow_mut().insert(key.clone(), height);
                        height + row_gap.get()
                    };
                    // 1. Keep the weighted mirror in step with the list.
                    let same = current.borrow().as_ref().is_some_and(|(s, _)| *s == next);
                    let changes = if same {
                        let cur = current.borrow();
                        let (source, cursor) = cur.as_ref().unwrap();
                        Some(source.take(cursor))
                    } else {
                        None
                    };
                    let mut geometry_changed;
                    if let Some(Changes::Delta(changes)) = changes {
                        geometry_changed = !changes.is_empty();
                        let mut m = mirror.borrow_mut();
                        for ch in changes {
                            match ch {
                                Change::Insert { key, index } => {
                                    let w = weight(&key);
                                    m.insert_weighted(index, key, w);
                                }
                                Change::Remove { key, .. } => {
                                    if let Some(anchor) = &mut anchor {
                                        anchor.removing(&key, &m);
                                    }
                                    m.remove(&key);
                                    estimates.borrow_mut().remove(&key);
                                    rows.borrow_mut().remove(&key, false);
                                    measured.borrow_mut().remove(&key);
                                    names.borrow_mut().remove(&key.to_string());
                                }
                                Change::Move { key, to, .. } => {
                                    let w = m.weight_of(&key).unwrap_or(0.0);
                                    m.remove(&key);
                                    m.insert_weighted(to, key, w);
                                }
                            }
                        }
                    } else {
                        geometry_changed = true;
                        item_observer.borrow_mut().take();
                        dirty_items.borrow_mut().clear();
                        if let Input::Incremental(list) = &next {
                            *item_observer.borrow_mut() =
                                Some(list.observe_items(Rc::new(ItemChanges { keys: dirty_items.clone(), revision })));
                        }
                        let cursor = next.subscribe();
                        estimates.borrow_mut().clear();
                        let mut m = Seq::new();
                        for k in next.snapshot() {
                            let w = weight(&k);
                            m.insert_weighted(m.len(), k, w);
                        }
                        // Final membership wins over any interaction. Snapshot inputs and
                        // bounded-log recovery must retire absent rows before selecting pins.
                        let removed: Vec<_> = rows.borrow().entries.keys().filter(|key| !m.contains(key)).cloned().collect();
                        for key in removed {
                            rows.borrow_mut().remove(&key, false);
                        }
                        measured.borrow_mut().retain(|key| m.contains(key));
                        names.borrow_mut().retain(|_, key| m.index_of(key).is_some());
                        if let Some(anchor) = &mut anchor {
                            anchor.surviving(&mirror.borrow(), &m);
                        }
                        *mirror.borrow_mut() = m;
                        *current.borrow_mut() = Some((next.clone(), cursor));
                    }
                    {
                        let mut m = mirror.borrow_mut();
                        for key in dirty_items.borrow_mut().drain() {
                            if m.contains(&key) {
                                geometry_changed = true;
                                m.set_weight(&key, weight(&key));
                                measured.borrow_mut().remove(&key);
                            }
                        }
                        if !u.kernel.borrow().is_following_tail(container)
                            && let Some(to) = anchor.as_ref().and_then(|anchor| anchor.position(&m))
                            && (to - y).abs() > 0.5
                        {
                            let mut kernel = u.kernel.borrow_mut();
                            let x = kernel.scroll_offset(container).0;
                            kernel.scroll_to_unclamped(container, x, (to as f32 + list_top.get()).max(0.0));
                            y = to;
                        }
                    }
                    if geometry_changed && !u.kernel.borrow().is_following_tail(container) {
                        *pending_anchor.borrow_mut() = anchor;
                        anchor_request.set(u.kernel.borrow().scroll_request(container));
                    }
                    // 2. The window: rows intersecting [y, y + height), plus overscan.
                    let m = mirror.borrow();
                    let len = m.len();
                    let first = m.index_at_weight(y).min(len);
                    let last = m.index_at_weight(y + height).min(len.saturating_sub(1));
                    let start = first.saturating_sub(OVERSCAN);
                    let end = if len == 0 { 0 } else { (last + 1 + OVERSCAN).min(len) };
                    let mut indices: Vec<_> = (start..end).collect();
                    if let Some(anchor) = pending_anchor.borrow().as_ref()
                        && let Some(index) = m.index_of(&anchor.key)
                    {
                        indices.push(index);
                    }
                    let interactions = u.interaction_nodes();
                    if !interactions.is_empty() {
                        let kernel = u.kernel.borrow();
                        indices.extend(rows.borrow().entries.iter().filter_map(|(key, (_, node))| {
                            interactions
                                .iter()
                                .any(|&target| kernel.is_in_interaction_scope(target, *node))
                                .then(|| m.index_of(key))
                                .flatten()
                        }));
                    }
                    indices.sort_unstable();
                    indices.dedup();
                    let want: Vec<K> = indices.iter().filter_map(|&i| m.nth(i).cloned()).collect();
                    let first = indices.first().copied().unwrap_or(0);
                    let last = indices.last().map_or(0, |&i| i + 1);
                    let (top_h, bottom_h) = (m.prefix_weight(first), m.total_weight() - m.prefix_weight(last));
                    let gap_heights: Vec<_> = indices
                        .windows(2)
                        .filter(|pair| pair[1] > pair[0] + 1)
                        .map(|pair| m.prefix_weight(pair[1]) - m.prefix_weight(pair[0] + 1))
                        .collect();
                    drop(m);
                    // 3. Reconcile the window's rows by key: rows coming into and leaving the
                    // window neither enter nor exit (they are not added or removed).
                    crate::app::window().kernel.borrow_mut().set_quiet(true);
                    let changed = rows.borrow_mut().reconcile(&want, false);
                    for key in &want {
                        names.borrow_mut().entry(key.to_string()).or_insert_with(|| key.clone());
                    }
                    measured.borrow_mut().retain(|key| rows.borrow().entries.contains_key(key));
                    if changed {
                        crate::app::window().kernel.borrow_mut().set_instant(true);
                    }
                    {
                        let mut kn = u.kernel.borrow_mut();
                        let height = |h: f64| crate::kernel::StyleProp::Height(crate::kernel::Length::Px(h as f32));
                        kn.set_style(top, height(top_h));
                        kn.set_style(bottom, height(bottom_h));
                        // Weights include one logical row gap. A hidden spacer replaces that
                        // missing extent; cancel only the extra physical flex gap it introduces.
                        let margin = crate::kernel::Length::Px(-row_gap.get() as f32);
                        kn.set_style(
                            top,
                            crate::kernel::StyleProp::MarginBottom(if want.is_empty() { crate::kernel::Length::Px(0.0) } else { margin }),
                        );
                        kn.set_style(bottom, crate::kernel::StyleProp::MarginTop(margin));
                    }
                    {
                        let mut gaps = gaps.borrow_mut();
                        while gaps.len() < gap_heights.len() {
                            let name = format!("gap-{}", gaps.len());
                            gaps.push(spacer(&name));
                        }
                        let mut kernel = u.kernel.borrow_mut();
                        for node in gaps.drain(gap_heights.len()..) {
                            kernel.destroy(node);
                        }
                        for (&node, &height) in gaps.iter().zip(&gap_heights) {
                            kernel.set_style(node, crate::kernel::StyleProp::Height(crate::kernel::Length::Px(height as f32)));
                            kernel
                                .set_style(node, crate::kernel::StyleProp::MarginBottom(crate::kernel::Length::Px(-row_gap.get() as f32)));
                        }
                    }
                    let mut nodes = vec![top];
                    let mut gap = 0;
                    for (position, key) in want.iter().enumerate() {
                        if position > 0 && indices[position] > indices[position - 1] + 1 {
                            nodes.push(gaps.borrow()[gap]);
                            gap += 1;
                        }
                        nodes.push(rows.borrow().entries[key].1);
                    }
                    nodes.push(bottom);
                    count.set(nodes.len());
                    place(parent.node, count_offset(&parent, &count), nodes.into_iter());
                    crate::app::window().kernel.borrow_mut().set_quiet(false);
                })
            });
        });
    })))
}

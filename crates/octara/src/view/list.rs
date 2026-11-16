//! Keyed rows from ordinary snapshots or incremental collections, with shared row ownership.

use super::{Mount, Parent, Slot, Slots, View, build_el, col, count_offset};
use crate::diagnostics as codes;
use crate::kernel::NodeId;
use crate::reactive::graph::{Ctx, ReadSignal, ScopeId, Signal, memo, memo_with, rt, untrack};
use crate::reactive::list::{Change, Cursor, ReadList};
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
        pub(super) fn take(&self, cursor: &Option<Cursor>) -> Vec<Change<K>> {
            match (self, cursor) {
                (Self::Incremental(list), Some(cursor)) => list.take(cursor),
                _ => Vec::new(),
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
const OVERSCAN: usize = 5;

/// Like [`each`], for long lists: only rows near the visible area exist. Put it directly in a
/// scroll container (`v_flex().overflow_y_scroll().flex_1()`); it follows that container's
/// scroll offset and height.
///
/// Row heights start from `estimate` and are replaced by each row's measured height once it
/// has been laid out; the first visible row stays put while rows above it are measured
/// (scroll anchoring). Rows are kept in a weighted order-statistic tree, so offset → row and
/// row → offset are O(log n) and an update costs O(visible · log n) whatever the length. The
/// two spacers are `{parent id}-top` / `-bottom`.
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
        let spacer = |side: &str| {
            let mut k = u.kernel.borrow_mut();
            let n = k.create(crate::kernel::Kind::Box);
            k.set_hidden(n, true);
            // Spacers stand for rows: they must not shrink in the scroll container's column.
            k.set_style(n, crate::kernel::StyleProp::Shrink(0.0));
            if let Some(p) = &prefix {
                k.set_test_id(n, Some(&format!("{p}-{side}")));
            }
            n
        };
        let (top, bottom) = (spacer("top"), spacer("bottom"));
        let scroll = u.scroll_state(container);
        let revision = Signal::scoped(0u64);
        let list_top: Rc<Cell<f32>> = Rc::default();
        let mirror: Rc<RefCell<Seq<K>>> = Rc::default();
        let input = memo(move || list().into_input(at));
        let rows = Rc::new(RefCell::new(Rows { entries: HashMap::new(), input, owner, base, at, row: Box::new(row) }));
        let measured: Rc<RefCell<HashSet<K>>> = Rc::default();
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
        let hook = {
            let (u, list_top, mirror, rows, measured) = (u.clone(), list_top.clone(), mirror.clone(), rows.clone(), measured.clone());
            u.clone().after_layout(Rc::new(move || {
                let mut k = u.kernel.borrow_mut();
                if !k.is_alive(container) || !k.is_alive(top) {
                    return false;
                }
                let (sx, sy) = k.scroll_offset(container);
                let cy = k.content_origin_y(container);
                let start = k.rect(top).1 - cy + sy;
                let mut changed = false;
                if (start - list_top.get()).abs() > 0.5 {
                    list_top.set(start);
                    changed = true;
                }
                let mut m = mirror.borrow_mut();
                let y = (sy - start).max(0.0) as f64;
                // Rows whose measured height differs from their weight.
                let rows = rows.borrow();
                let rows = &rows.entries;
                let mut measured = measured.borrow_mut();
                let updates: Vec<(K, f64)> = rows
                    .iter()
                    .filter(|(_, (_, node))| k.is_alive(*node))
                    .filter_map(|(key, (_, node))| {
                        let h = k.rect(*node).3 as f64;
                        m.weight_of(key).filter(|w| (w - h).abs() > 0.5).map(|_| (key.clone(), h))
                    })
                    .collect();
                if !updates.is_empty() {
                    // Keep the first previously laid-out row in view, even if its own height
                    // changed. A new estimated row must not displace a row already being read.
                    let mut anchor = m.index_at_weight(y).min(m.len().saturating_sub(1));
                    let seen = rows
                        .keys()
                        .filter(|key| measured.contains(*key))
                        .filter_map(|key| m.index_of(key))
                        .filter(|&i| m.prefix_weight(i + 1) > y && m.prefix_weight(i) < y + k.client_size(container).1 as f64)
                        .min();
                    if let Some(i) = seen {
                        anchor = i;
                    }
                    let on_screen = m.prefix_weight(anchor) - y;
                    for (key, h) in &updates {
                        m.set_weight(key, *h);
                    }
                    let to = m.prefix_weight(anchor) - on_screen;
                    // Following its tail, the container keeps its end in place instead (after the
                    // next layout).
                    if (to - y).abs() > 0.5 && !k.is_following_tail(container) {
                        k.scroll_to_unclamped(container, sx, (to as f32 + start).max(0.0));
                    }
                    changed = true;
                }
                measured.extend(rows.iter().filter(|(_, (_, node))| k.is_alive(*node)).map(|(key, _)| key.clone()));
                if changed {
                    revision.update(|r| *r += 1);
                }
                changed
            }))
        };
        {
            let u = u.clone();
            rt(|rt| {
                rt.on_cleanup(Box::new(move || {
                    u.remove_after_layout(hook);
                    let mut k = u.kernel.borrow_mut();
                    for n in [top, bottom] {
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
            let y = (state.offset.1 - list_top.get()).max(0.0) as f64;
            let height = state.client.1 as f64;
            rt(|rt| {
                rt.with_frame(Ctx::Observe, None, Some(Some(owner)), || {
                    let weight = |k: &K| untrack(|| next.read(k)).map(|item| estimate(&item) as f64).unwrap_or(0.0);
                    // 1. Keep the weighted mirror in step with the list.
                    let same = current.borrow().as_ref().is_some_and(|(s, _)| *s == next);
                    if same {
                        let changes = {
                            let cur = current.borrow();
                            let (s, c) = cur.as_ref().unwrap();
                            s.take(c)
                        };
                        let mut m = mirror.borrow_mut();
                        for ch in changes {
                            match ch {
                                Change::Insert { key, index } => {
                                    let w = weight(&key);
                                    m.insert_weighted(index, key, w);
                                }
                                Change::Remove { key, .. } => {
                                    m.remove(&key);
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
                        let cursor = next.subscribe();
                        let mut m = Seq::new();
                        for k in next.snapshot() {
                            let w = weight(&k);
                            m.insert_weighted(m.len(), k, w);
                        }
                        names.borrow_mut().retain(|_, key| m.index_of(key).is_some());
                        *mirror.borrow_mut() = m;
                        *current.borrow_mut() = Some((next.clone(), cursor));
                    }
                    // 2. The window: rows intersecting [y, y + height), plus overscan.
                    let m = mirror.borrow();
                    let len = m.len();
                    let first = m.index_at_weight(y).min(len);
                    let last = m.index_at_weight(y + height).min(len.saturating_sub(1));
                    let start = first.saturating_sub(OVERSCAN);
                    let end = if len == 0 { 0 } else { (last + 1 + OVERSCAN).min(len) };
                    let want: Vec<K> = (start..end).filter_map(|i| m.nth(i).cloned()).collect();
                    let (top_h, bottom_h) = (m.prefix_weight(start), m.total_weight() - m.prefix_weight(end));
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
                        let u = crate::app::window();
                        let mut kn = u.kernel.borrow_mut();
                        let height = |h: f64| crate::kernel::StyleProp::Height(crate::kernel::Length::Px(h as f32));
                        kn.set_style(top, height(top_h));
                        kn.set_style(bottom, height(bottom_h));
                    }
                    let nodes =
                        std::iter::once(top).chain(want.iter().map(|key| rows.borrow().entries[key].1)).chain(std::iter::once(bottom));
                    place(parent.node, count_offset(&parent, &count), nodes);
                    count.set(want.len() + 2);
                    crate::app::window().kernel.borrow_mut().set_quiet(false);
                })
            });
        });
    })))
}

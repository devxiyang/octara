//! Rendering collections: [`each`] and [`virtual_each`] apply a stage's change log row by row.

use super::{Mount, Parent, Slot, Slots, View, build_el, col, count_offset};
use crate::diagnostics as codes;
use crate::kernel::NodeId;
use crate::reactive::graph::{Ctx, ReadSignal, ScopeId, Signal, rt};
use crate::reactive::list::{Change, Cursor, Source};
use crate::reactive::seq::Seq;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt::Display;
use std::hash::Hash;
use std::panic::Location;
use std::rc::Rc;

fn build_row<K: Display, T: Clone + 'static>(
    rt: &crate::reactive::graph::Runtime,
    owner: ScopeId,
    base: &str,
    at: &'static Location<'static>,
    k: &K,
    item: Option<ReadSignal<T>>,
    row: &dyn Fn(&K, ReadSignal<T>) -> View,
) -> (ScopeId, NodeId) {
    let s = rt.create_scope(Some(owner), &format!("{base}/row:{k}"));
    let node = rt.with_frame(Ctx::Build, None, Some(Some(s)), || match item.map(|i| row(k, i)).unwrap_or(View::Empty) {
        View::El(e) => build_el(*e),
        _ => {
            rt.diag(&codes::EACH_ROW_NOT_ELEMENT, "a list row must be one element".into(), at);
            build_el(col())
        }
    });
    (s, node)
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

/// The source a list region follows and its read position.
type Current<K, T> = Rc<RefCell<Option<(Source<K, T>, Cursor)>>>;

/// Renders `source()`. While it stays the same stage, its log is applied row by row: one row
/// created per insert, one disposed per remove, one moved per move. Switching to another stage
/// reconciles by key once. A row must be one element; it receives its item's own signal.
#[track_caller]
pub fn each<K, T>(source: impl Fn() -> Source<K, T> + 'static, row: impl Fn(&K, ReadSignal<T>) -> View + 'static) -> View
where
    K: Eq + Hash + Clone + Display + 'static,
    T: Clone + 'static,
{
    let at = Location::caller();
    View::Dyn(Mount(Box::new(move |parent: &Parent, target: &Slots| {
        let count: Rc<Cell<usize>> = Rc::default();
        target.borrow_mut().push(Slot::Count(count.clone()));
        let (owner, base) = rt(|rt| {
            let owner = rt.current_scope().expect("each outside a scope");
            (owner, rt.child_name(owner, "each"))
        });
        let parent = parent.clone();
        let rows: Rc<RefCell<HashMap<K, (ScopeId, NodeId)>>> = Rc::default();
        let current: Current<K, T> = Rc::default();
        crate::reactive::graph::binding(at, move || {
            let src = source();
            src.track();
            rt(|rt| {
                rt.with_frame(Ctx::Untracked, None, Some(Some(owner)), || {
                    let u = crate::app::window();
                    let same = current.borrow().as_ref().is_some_and(|(s, _)| *s == src);
                    if same {
                        let changes = {
                            let cur = current.borrow();
                            let (s, c) = cur.as_ref().unwrap();
                            s.take(c)
                        };
                        for ch in changes {
                            let base_index = count_offset(&parent, &count);
                            match ch {
                                Change::Insert { key, index } => {
                                    let (scope, node) = build_row(rt, owner, &base, at, &key, src.item(&key), &row);
                                    u.kernel.borrow_mut().insert_child(parent.node, node, base_index + index);
                                    rows.borrow_mut().insert(key, (scope, node));
                                    count.set(count.get() + 1);
                                }
                                Change::Remove { key, .. } => {
                                    let r = rows.borrow_mut().remove(&key);
                                    if let Some((scope, node)) = r {
                                        u.kernel.borrow_mut().begin_exit(node);
                                        rt.dispose_scope(scope);
                                        count.set(count.get() - 1);
                                    }
                                }
                                Change::Move { key, to, .. } => {
                                    if let Some((_, node)) = rows.borrow().get(&key) {
                                        u.kernel.borrow_mut().move_child(*node, base_index + to);
                                    }
                                }
                            }
                        }
                        return;
                    }
                    let cursor = src.subscribe();
                    let mut old = std::mem::take(&mut *rows.borrow_mut());
                    let order: Vec<(K, (ScopeId, NodeId))> = src
                        .snapshot()
                        .into_iter()
                        .map(|k| {
                            let e = old.remove(&k).unwrap_or_else(|| build_row(rt, owner, &base, at, &k, src.item(&k), &row));
                            (k, e)
                        })
                        .collect();
                    for (_, (scope, node)) in old {
                        u.kernel.borrow_mut().begin_exit(node);
                        rt.dispose_scope(scope);
                    }
                    place(parent.node, count_offset(&parent, &count), order.iter().map(|(_, (_, n))| *n));
                    count.set(order.len());
                    *rows.borrow_mut() = order.into_iter().collect();
                    *current.borrow_mut() = Some((src, cursor));
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
pub fn virtual_each<K, T>(
    source: impl Fn() -> Source<K, T> + 'static,
    estimate: impl Fn(&T) -> f32 + 'static,
    row: impl Fn(&K, ReadSignal<T>) -> View + 'static,
) -> View
where
    K: Eq + Hash + Clone + Display + 'static,
    T: Clone + 'static,
{
    let at = Location::caller();
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
        let rows: Rc<RefCell<HashMap<K, (ScopeId, NodeId)>>> = Rc::default();
        let current: Current<K, T> = Rc::default();

        // After each layout: where the list starts in the container, and the rows' real heights.
        let hook = {
            let (u, list_top, mirror, rows) = (u.clone(), list_top.clone(), mirror.clone(), rows.clone());
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
                let updates: Vec<(K, f64)> = rows
                    .iter()
                    .filter(|(_, (_, node))| k.is_alive(*node))
                    .filter_map(|(key, (_, node))| {
                        let h = k.rect(*node).3 as f64;
                        m.weight_of(key).filter(|w| (w - h).abs() > 0.5).map(|_| (key.clone(), h))
                    })
                    .collect();
                if !updates.is_empty() {
                    // Scroll anchoring: keep in place a row the reader has already seen at its
                    // real height (the first measured row still in view), else the top row.
                    let mut anchor = m.index_at_weight(y).min(m.len().saturating_sub(1));
                    let mut seen: Vec<usize> = rows
                        .keys()
                        .filter(|key| !updates.iter().any(|(u, _)| u == *key))
                        .filter_map(|key| m.index_of(key))
                        .filter(|&i| m.prefix_weight(i + 1) > y)
                        .collect();
                    seen.sort_unstable();
                    if let Some(&i) = seen.first() {
                        anchor = i;
                    }
                    let on_screen = m.prefix_weight(anchor) - y;
                    for (key, h) in &updates {
                        m.set_weight(key, *h);
                    }
                    let to = m.prefix_weight(anchor) - on_screen;
                    if (to - y).abs() > 0.5 {
                        k.scroll_to_unclamped(container, sx, (to as f32 + start).max(0.0));
                    }
                    changed = true;
                }
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
        crate::reactive::graph::binding(at, move || {
            let src = source();
            src.track();
            let state = scroll.get();
            revision.get();
            let y = (state.offset.1 - list_top.get()).max(0.0) as f64;
            let height = state.client.1 as f64;
            rt(|rt| {
                rt.with_frame(Ctx::Untracked, None, Some(Some(owner)), || {
                    let weight = |k: &K| src.item(k).map(|i| i.with_untracked(|t| estimate(t)) as f64).unwrap_or(0.0);
                    // 1. Keep the weighted mirror in step with the source.
                    let same = current.borrow().as_ref().is_some_and(|(s, _)| *s == src);
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
                                }
                                Change::Move { key, to, .. } => {
                                    let w = m.weight_of(&key).unwrap_or(0.0);
                                    m.remove(&key);
                                    m.insert_weighted(to, key, w);
                                }
                            }
                        }
                    } else {
                        let cursor = src.subscribe();
                        let mut m = Seq::new();
                        for k in src.snapshot() {
                            let w = weight(&k);
                            m.insert_weighted(m.len(), k, w);
                        }
                        *mirror.borrow_mut() = m;
                        *current.borrow_mut() = Some((src.clone(), cursor));
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
                    let mut old = std::mem::take(&mut *rows.borrow_mut());
                    let mut built = false;
                    let order: Vec<(K, (ScopeId, NodeId))> = want
                        .into_iter()
                        .map(|k| {
                            let e = old.remove(&k).unwrap_or_else(|| {
                                built = true;
                                build_row(rt, owner, &base, at, &k, src.item(&k), &row)
                            });
                            (k, e)
                        })
                        .collect();
                    // The window moved: rows shift in the list's own coordinates without moving on
                    // screen, so nothing here is layout motion.
                    if built || !old.is_empty() {
                        crate::app::window().kernel.borrow_mut().set_instant(true);
                    }
                    for (_, (scope, _)) in old {
                        rt.dispose_scope(scope);
                    }
                    {
                        let u = crate::app::window();
                        let mut kn = u.kernel.borrow_mut();
                        let height = |h: f64| crate::kernel::StyleProp::Height(crate::kernel::Length::Px(h as f32));
                        kn.set_style(top, height(top_h));
                        kn.set_style(bottom, height(bottom_h));
                    }
                    let nodes = std::iter::once(top).chain(order.iter().map(|(_, (_, n))| *n)).chain(std::iter::once(bottom));
                    place(parent.node, count_offset(&parent, &count), nodes);
                    count.set(order.len() + 2);
                    *rows.borrow_mut() = order.into_iter().collect();
                    crate::app::window().kernel.borrow_mut().set_quiet(false);
                })
            });
        });
    })))
}

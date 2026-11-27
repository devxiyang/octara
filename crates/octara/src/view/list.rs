//! Keyed row identity and demand-driven residency for long scrollable lists.

use super::style::Styled;
use super::{El, NodeHandle, Target, View, control::next_region, el, el::build_el, node_handle};
use octara_model::geometry::{LayoutBox, Size, Vec2};
use octara_runtime::{
    BuildCx, MountCx, ReadCx, ScopeId, Signal,
    graph::{
        delta::{Changes, Cursor},
        list::{
            IntoReadList, ListDelta, ReadList, RowSignal,
            seq::{RowSize, SumTree},
        },
    },
    window::CopyRow,
};
use octara_tree::{NodeChange, NodeFlags, NodeId, NodeRef, Retirement, RowKey, tables::AuthoredStyle};
use rustc_hash::{FxHashMap, FxHashSet};
use smallvec::smallvec;
use std::{cell::RefCell, fmt::Display, hash::Hash, rc::Rc};

struct MountedRow {
    scope: ScopeId,
    node: NodeHandle,
    identity: RowKey,
}

impl MountedRow {
    fn retire(self, cx: &mut MountCx<'_>, retirement: Retirement) {
        cx.retire(self.node.get(), retirement);
        cx.retire_scope(self.scope);
    }
}

fn row_identity(region: u64, key: &impl Display, used: &mut FxHashSet<RowKey>) -> RowKey {
    let label = key.to_string();
    let primary = RowKey::new(region, label.clone());
    if used.insert(primary.clone()) {
        primary
    } else {
        let distinct = RowKey::new(next_region(), label);
        used.insert(distinct.clone());
        distinct
    }
}

fn build_row<K, T, P>(
    cx: &mut MountCx<'_>,
    target: &Target,
    identity: RowKey,
    key: &K,
    list: ReadList<K, T>,
    render: &mut impl FnMut(&mut BuildCx<'_>, &K, RowSignal<K, T>) -> El,
    observe: impl FnOnce(&mut MountCx<'_>, NodeRef) -> P,
) -> (MountedRow, P)
where
    K: Clone + Eq + Hash + 'static,
    T: 'static,
{
    let mount = |cx: &mut MountCx<'_>| {
        let element = cx.build(|cx| render(cx, key, list.row(key.clone())));
        let node = build_el(cx, element, target.component, Some(identity.clone()), None);
        let feedback = observe(cx, node);
        (node, feedback)
    };
    let name = identity.to_string();
    let (scope, (node, feedback)) = cx.with_named_scope(&name, mount);
    (MountedRow { scope, node: node_handle(cx, node), identity }, feedback)
}

/// Mounts one retained element per business key. Moves keep the existing row and child scope.
/// Row properties can subscribe to `RowSignal` to update independently of the row order.
#[track_caller]
pub fn each<K, T>(list: impl IntoReadList<K, T>, mut render: impl FnMut(&mut BuildCx<'_>, &K, RowSignal<K, T>) -> El + 'static) -> View
where
    K: Clone + Eq + Hash + Display + 'static,
    T: 'static,
{
    let list = list.into_read_list();
    View::region(move |cx, target| {
        let region = next_region();
        let mut entries = FxHashMap::<K, MountedRow>::default();
        cx.with_scope(|cx| {
            cx.structure(
                move |r| r.keys(list),
                move |cx, keys| {
                    let keys_set: FxHashSet<_> = keys.iter().collect();
                    let removed: Vec<_> = entries.keys().filter(|key| !keys_set.contains(key)).cloned().collect();
                    for key in removed {
                        entries.remove(&key).expect("mounted key").retire(cx, Retirement::Destroy);
                    }
                    let mut identities = entries.values().map(|row| row.identity.clone()).collect();
                    for key in keys {
                        if !entries.contains_key(key) {
                            let identity = row_identity(region, key, &mut identities);
                            let (row, ()) = build_row(cx, &target, identity, key, list, &mut render, |_, _| ());
                            entries.insert(key.clone(), row);
                        }
                    }
                    target.set_nodes(cx, keys.iter().map(|key| &entries[key].node));
                },
            )
        });
    })
}

/// Starting row height and the number of extra rows mounted above and below the viewport.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VirtualOptions {
    pub estimated_row_height: f32,
    pub overscan: usize,
    /// Preserve the first visible row across insertions, moves, and height changes.
    pub maintain_scroll_anchor: bool,
}

impl Default for VirtualOptions {
    fn default() -> Self {
        Self { estimated_row_height: 32.0, overscan: 4, maintain_scroll_anchor: false }
    }
}

struct Resident {
    row: MountedRow,
    geometry: Signal<Option<LayoutBox>>,
    pinned: Signal<bool>,
}

struct Spacer {
    node: NodeHandle,
    height: f64,
}

struct VirtualRows<K> {
    order: SumTree<K>,
    cursor: Option<Cursor>,
    rows: FxHashMap<K, Resident>,
    gaps: FxHashMap<K, Spacer>,
    trailing: Spacer,
    leading: Spacer,
    top: Signal<Option<LayoutBox>>,
    container: Signal<Option<LayoutBox>>,
    scroll: Signal<Vec2>,
    anchor: Signal<Option<NodeId>>,
    viewport: octara_runtime::ReadSignal<Size>,
    origin: f64,
    region: u64,
}

#[derive(PartialEq)]
struct Plan<K> {
    leading: f64,
    trailing: f64,
    rows: Vec<(K, f64)>,
}

impl<K> VirtualRows<K>
where
    K: Clone + Eq + Hash + 'static,
{
    fn topology<T: 'static>(&mut self, read: &mut ReadCx<'_>, list: ReadList<K, T>, estimate: f64) {
        let estimated = RowSize { height: estimate, width: 0.0 };
        if let Some(cursor) = self.cursor.as_mut()
            && let Changes::Delta(changes) = read.list_topology_changes(list, cursor)
        {
            let changed: FxHashSet<_> = changes
                .into_iter()
                .filter_map(|delta| match delta {
                    ListDelta::Insert(key) | ListDelta::Remove(key) | ListDelta::Move { key, .. } => Some(key),
                    ListDelta::Update(_) | ListDelta::Reset => None,
                })
                .collect();
            let mut upserts = Vec::new();
            for key in changed {
                let size = self.order.size_of(&key).unwrap_or(estimated);
                self.order.remove(&key);
                if let Some(rank) = read.untrack(|read| read.rank(list, &key)) {
                    upserts.push((rank, key, size));
                }
            }
            upserts.sort_unstable_by_key(|&(rank, ..)| rank);
            for (rank, key, size) in upserts {
                self.order.insert(rank, key, size);
            }
            return;
        }
        self.cursor = Some(read.list_topology_cursor(list));
        let mut order = SumTree::default();
        for (rank, key) in read.keys(list).into_iter().enumerate() {
            let size = self.order.size_of(&key).unwrap_or(estimated);
            order.insert(rank, key, size);
        }
        self.order = order;
    }

    fn copy_between<T: 'static>(
        &self,
        read: &mut ReadCx<'_>,
        list: ReadList<K, T>,
        first: NodeId,
        last: NodeId,
        copy: Option<&RowText<K, T>>,
    ) -> Option<Result<Vec<CopyRow>, ()>> {
        let rank = |node| {
            self.rows.iter().find_map(|(key, resident)| {
                matches!(resident.row.node.get(), NodeRef::Existing { id, .. } if id == node).then(|| self.order.rank(key)).flatten()
            })
        };
        let (start, end) = (rank(first)?, rank(last)?);
        Some(
            (start + 1..end)
                .map(|rank| {
                    let key = self.order.key_at(rank).expect("selected row rank");
                    if let Some(resident) = self.rows.get(key)
                        && let NodeRef::Existing { id, .. } = resident.row.node.get()
                    {
                        return Ok(CopyRow::Mounted(id));
                    }
                    let copy = copy.ok_or(())?;
                    read.with_row(list, key, |row| row.map(|row| CopyRow::Unmounted(copy(key, row))).ok_or(()))
                })
                .collect(),
        )
    }

    fn plan<T: 'static>(&mut self, read: &mut ReadCx<'_>, list: ReadList<K, T>, options: VirtualOptions) -> Plan<K> {
        let estimate = f64::from(options.estimated_row_height);
        self.topology(read, list, estimate);
        let offset = f64::from(read.get(self.scroll).y);
        let container = read.get(self.container);
        let viewport = read.get(self.viewport);
        let top = read.get(self.top);
        self.origin = top.map_or(0.0, |top| f64::from(top.rect.y));
        let visible_height = container.map_or(viewport.height, |bounds| {
            (bounds.rect.h - bounds.padding.top - bounds.padding.bottom - bounds.border.top - bounds.border.bottom).max(0.0)
        });
        let anchor = read.get(self.anchor);
        let mut anchored = None;
        let mut pinned = Vec::new();
        for (key, resident) in &self.rows {
            let is_pinned = read.get(resident.pinned);
            let is_anchor = matches!(resident.row.node.get(), NodeRef::Existing { id, .. } if Some(id) == anchor);
            let rank = (is_pinned || is_anchor).then(|| self.order.rank(key)).flatten();
            if let Some(rank) = rank.filter(|_| is_pinned) {
                pinned.push(rank);
            }
            if let Some(geometry) = read.get(resident.geometry)
                && geometry.rect.h.is_finite()
            {
                let width = if geometry.rect.w.is_finite() { geometry.rect.w.max(0.0) } else { 0.0 };
                self.order.set_size(key, RowSize { height: f64::from(geometry.rect.h.max(1.0)), width });
                if let Some(rank) = rank.filter(|_| is_anchor) {
                    anchored = Some((rank, f64::from(geometry.rect.y)));
                }
            }
        }
        // Predict where the next spacer plan puts the retained anchor. Geometry already
        // includes any compensation applied to the measured layout during this turn.
        let compensation = anchored.map_or(0.0, |(rank, measured_y)| {
            pinned.push(rank);
            self.origin + self.order.offset(rank) - measured_y
        });
        let from = (offset + compensation - self.origin).max(0.0);
        let end = from + f64::from(visible_height);
        let summary = self.order.summary();
        let first = self.order.rank_at(from).min(summary.count.saturating_sub(1));
        let last = self.order.rank_at(end).saturating_add(1);
        let visible = first.saturating_sub(options.overscan)..last.saturating_add(options.overscan).min(summary.count);
        pinned.extend(visible);
        pinned.sort_unstable();
        pinned.dedup();
        let leading = self.order.offset(pinned.first().copied().unwrap_or(0));
        let mut previous_end = leading;
        let mut rows = Vec::with_capacity(pinned.len());
        for rank in pinned {
            let key = self.order.key_at(rank).expect("resident rank");
            let top = self.order.offset(rank);
            rows.push((key.clone(), (top - previous_end).max(0.0)));
            previous_end = self.order.offset(rank + 1);
        }
        Plan { leading, trailing: (summary.total_height - previous_end).max(0.0), rows }
    }
}

impl Spacer {
    fn build(cx: &mut MountCx<'_>, height: f64, anchor: bool) -> Self {
        let mut flags = NodeFlags::HIDDEN | NodeFlags::SELECT_NONE;
        flags.set(NodeFlags::VIRTUAL_SCROLL_ANCHOR, anchor);
        let node = build_el(cx, el().h(height).shrink(0.0).flags(flags), None, None, None);
        Self { node: node_handle(cx, node), height }
    }

    fn resize(&mut self, cx: &mut MountCx<'_>, height: f64) {
        if self.height == height {
            return;
        }
        self.height = height;
        let style = AuthoredStyle {
            base: smallvec![octara_model::style::StyleProp::Height(height.into()), octara_model::style::StyleProp::Shrink(0.0)],
            ..Default::default()
        };
        cx.change(self.node.get(), NodeChange::Style(Box::new(style)));
    }
}

type RowText<K, T> = dyn Fn(&K, &T) -> String;

/// Mounts a windowed list directly inside a vertical scroll container. Put spacing inside each
/// row. Offscreen focused or captured rows remain mounted, with spacers preserving their offsets.
#[track_caller]
pub fn virtual_each<K, T>(
    list: impl IntoReadList<K, T>,
    options: VirtualOptions,
    render: impl FnMut(&mut BuildCx<'_>, &K, RowSignal<K, T>) -> El + 'static,
) -> View
where
    K: Clone + Eq + Hash + Display + 'static,
    T: 'static,
{
    virtual_rows(list, options, render, None)
}

/// Mounts a virtual list that can copy a selection across unmounted rows.
/// The callback supplies the plain text of a whole unmounted row in selection order.
#[track_caller]
pub fn virtual_each_with_copy<K, T>(
    list: impl IntoReadList<K, T>,
    options: VirtualOptions,
    render: impl FnMut(&mut BuildCx<'_>, &K, RowSignal<K, T>) -> El + 'static,
    copy_text: impl Fn(&K, &T) -> String + 'static,
) -> View
where
    K: Clone + Eq + Hash + Display + 'static,
    T: 'static,
{
    virtual_rows(list, options, render, Some(Box::new(copy_text)))
}

#[track_caller]
fn virtual_rows<K, T>(
    list: impl IntoReadList<K, T>,
    mut options: VirtualOptions,
    mut render: impl FnMut(&mut BuildCx<'_>, &K, RowSignal<K, T>) -> El + 'static,
    copy_text: Option<Box<RowText<K, T>>>,
) -> View
where
    K: Clone + Eq + Hash + Display + 'static,
    T: 'static,
{
    if !options.estimated_row_height.is_finite() || options.estimated_row_height <= 0.0 {
        options.estimated_row_height = VirtualOptions::default().estimated_row_height;
    }
    let list = list.into_read_list();
    View::region(move |cx, target| {
        cx.with_scope(|cx| {
            let parent = target.siblings.parent.clone();
            let (top, container, scroll, anchor, viewport) = cx.build(|cx| {
                (cx.signal(None::<LayoutBox>), cx.signal(None::<LayoutBox>), cx.signal(Vec2::default()), cx.signal(None), cx.viewport())
            });
            let leading = Spacer::build(cx, 0.0, options.maintain_scroll_anchor);
            let trailing = Spacer::build(cx, 0.0, false);
            cx.observe_geometry(leading.node.get(), top);
            cx.observe_geometry(parent.get(), container);
            cx.observe_scroll(parent.get(), scroll);
            cx.observe_scroll_anchor(parent.get(), anchor);
            let rows = Rc::new(RefCell::new(VirtualRows::<K> {
                order: SumTree::default(),
                cursor: None,
                rows: FxHashMap::default(),
                gaps: FxHashMap::default(),
                trailing,
                leading,
                top,
                container,
                scroll,
                anchor,
                viewport,
                origin: 0.0,
                region: next_region(),
            }));
            let copy = rows.clone();
            cx.on_copy_rows(parent.get(), move |read, first, last| {
                copy.borrow().copy_between(read, list, first, last, copy_text.as_deref())
            });
            let reveal = rows.clone();
            cx.on_reveal(parent.get(), move |w, label| {
                let rows = reveal.borrow();
                let index =
                    (0..rows.order.summary().count).find(|&index| rows.order.key_at(index).is_some_and(|key| key.to_string() == label));
                if let Some(index) = index
                    && let NodeRef::Existing { id, .. } = parent.get()
                {
                    let y = rows.origin + rows.order.offset(index);
                    let x = w.get(scroll).x;
                    w.scroll_to(id, Vec2::new(x, y as f32));
                }
            });
            let source = rows.clone();
            cx.structure(
                move |r| source.borrow_mut().plan(r, list, options),
                move |cx, plan| {
                    let mut rows = rows.borrow_mut();
                    apply_plan(cx, &target, &mut rows, list, plan, &mut render);
                },
            );
        });
    })
}

fn apply_plan<K, T>(
    cx: &mut MountCx<'_>,
    target: &Target,
    state: &mut VirtualRows<K>,
    list: ReadList<K, T>,
    plan: &Plan<K>,
    render: &mut impl FnMut(&mut BuildCx<'_>, &K, RowSignal<K, T>) -> El,
) where
    K: Clone + Eq + Hash + Display + 'static,
    T: 'static,
{
    let keep: FxHashMap<_, _> = plan.rows.iter().map(|(key, gap)| (key, *gap)).collect();
    let retired: Vec<_> = state.rows.keys().filter(|key| !keep.contains_key(key)).cloned().collect();
    for key in retired {
        state.rows.remove(&key).expect("resident key").row.retire(cx, Retirement::Destroy);
    }
    state.gaps.retain(|key, gap| {
        let keep = keep.get(key).is_some_and(|height| *height > 0.0);
        if !keep {
            cx.retire(gap.node.get(), Retirement::Destroy);
        }
        keep
    });
    state.leading.resize(cx, plan.leading);
    state.trailing.resize(cx, plan.trailing);
    let mut nodes = Vec::with_capacity(plan.rows.len() + state.gaps.len() + 2);
    nodes.push(state.leading.node.clone());
    let mut added_rows = false;
    let mut identities = state.rows.values().map(|resident| resident.row.identity.clone()).collect();
    for (key, height) in &plan.rows {
        if *height > 0.0 {
            let gap = state.gaps.entry(key.clone()).or_insert_with(|| Spacer::build(cx, *height, false));
            gap.resize(cx, *height);
            nodes.push(gap.node.clone());
        }
        if !state.rows.contains_key(key) {
            let identity = row_identity(state.region, key, &mut identities);
            let mut fixed_height = |cx: &mut BuildCx<'_>, key: &K, item| render(cx, key, item).shrink(0.0);
            let (row, (geometry, pinned)) = build_row(cx, target, identity, key, list, &mut fixed_height, |cx, node| {
                let (geometry, pinned) = cx.build(|cx| (cx.signal(None::<LayoutBox>), cx.signal(false)));
                cx.observe_geometry(node, geometry);
                cx.observe_pinned(node, pinned);
                (geometry, pinned)
            });
            state.rows.insert(key.clone(), Resident { row, geometry, pinned });
            added_rows = true;
        }
        nodes.push(state.rows[key].row.node.clone());
    }
    nodes.push(state.trailing.node.clone());
    target.set_nodes(cx, &nodes);
    if added_rows {
        cx.refresh_current_structure();
    }
}

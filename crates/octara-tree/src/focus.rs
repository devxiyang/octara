//! Focus traversal, roving focus groups, and layer focus history.

use octara_model::identity::NodeId;
use octara_model::style::State;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{NodeFlags, Tree, TreeDelta, overlay::FocusTrapFeatures};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrow {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Axis {
    Horizontal,
    Vertical,
    Both,
}

/// A container with one Tab stop and directional navigation between its members.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FocusGroup {
    axis: Axis,
    wraps: bool,
    activates: bool,
}

impl FocusGroup {
    const fn new(axis: Axis) -> Self {
        Self { axis, wraps: false, activates: false }
    }

    pub const fn horizontal() -> Self {
        Self::new(Axis::Horizontal)
    }

    pub const fn vertical() -> Self {
        Self::new(Axis::Vertical)
    }

    pub const fn both() -> Self {
        Self::new(Axis::Both)
    }

    pub const fn is_vertical(self) -> bool {
        matches!(self.axis, Axis::Vertical)
    }

    pub const fn wrap(mut self) -> Self {
        self.wraps = true;
        self
    }

    /// Activate a directional navigation target after its new focus has settled.
    pub const fn activate_on_navigation(mut self) -> Self {
        self.activates = true;
        self
    }
}

impl Tree {
    fn is_focus_preferred(&self, node: NodeId) -> bool {
        self.semantics(node).is_some_and(|slot| slot.focus_preferred)
            || self.node(node).is_some_and(|core| core.flags.contains(NodeFlags::FOCUS_PREFERRED))
    }

    /// Whether a node can receive direct focus under the active layer policies.
    pub fn is_focusable(&self, node: NodeId) -> bool {
        self.can_set_focus(Some(node))
    }

    pub fn can_set_focus(&self, node: Option<NodeId>) -> bool {
        let modal = self.front_modal();
        let lock = self.focus_layer(modal, FocusTrapFeatures::FOCUS_LOCK);
        node.map_or(lock.is_none(), |node| {
            self.focusable_in(node, modal) && lock.is_none_or(|lock| self.is_in_interaction_scope(node, lock))
        })
    }

    fn focus_layer(&self, modal: Option<NodeId>, feature: FocusTrapFeatures) -> Option<NodeId> {
        self.front_layer(modal, |_, layer| layer.focus_trap.contains(feature))
    }

    pub fn is_layer_focus_container(&self, node: NodeId) -> bool {
        self.overlay(node).and_then(|slot| slot.layer).is_some_and(|layer| {
            layer.focus_trap.intersects(FocusTrapFeatures::INITIAL_FOCUS | FocusTrapFeatures::AUTO_FOCUS | FocusTrapFeatures::TAB_LOCK)
        })
    }

    pub(crate) fn is_in_layer_content(&self, node: NodeId) -> bool {
        self.overlay_nodes()
            .any(|layer| self.overlay(layer).is_some_and(|slot| slot.layer.is_some()) && self.is_logically_owned_by(node, layer))
    }

    pub(crate) fn focusable_in(&self, node: NodeId, modal: Option<NodeId>) -> bool {
        (self.node(node).is_some_and(|core| core.flags.contains(NodeFlags::FOCUSABLE)) || self.is_layer_focus_container(node))
            && self.interactive_in(node, modal)
    }

    /// The nearest focus group enclosing this node.
    pub fn focus_group_of(&self, node: NodeId) -> Option<NodeId> {
        self.ancestor_matching(self.parent(node)?, |id| self.semantics(id).is_some_and(|slot| slot.focus_group.is_some()))
    }

    fn ordered_focusable(&self, scope: Option<NodeId>) -> Vec<NodeId> {
        let mut following: FxHashMap<NodeId, Vec<NodeId>> = FxHashMap::default();
        let mut relocated = FxHashSet::default();
        for (layer, _) in self.shown_layers() {
            if let Some(owner) = self.anchored_layer_owner(layer)
                && self.is_shown(owner)
                && !self.is_within(owner, layer)
            {
                following.entry(owner).or_default().push(layer);
                relocated.insert(layer);
            }
        }

        let mut ordered = Vec::new();
        let mut visited = FxHashSet::default();
        let mut pending = vec![self.root()];
        while let Some(id) = pending.pop() {
            if !self.is_locally_shown(id) || !visited.insert(id) {
                continue;
            }
            if self.node(id).is_some_and(|core| core.flags.contains(NodeFlags::FOCUSABLE)) && self.focusable_in(id, scope) {
                ordered.push(id);
            }
            if let Some(panels) = following.get(&id) {
                pending.extend(panels.iter().rev().copied());
            }
            pending.extend(self.children(id).iter().rev().filter(|c| !relocated.contains(c)).copied());
        }
        ordered
    }

    /// Positive indices precede natural order. Anchored panels follow their trigger and
    /// focus groups choose a single stop. Negative indices remain available to direct focus.
    pub fn focus_order(&self, scope: Option<NodeId>, focused: Option<NodeId>) -> Vec<NodeId> {
        let candidates = self.ordered_focusable(scope);
        let group_of = |id| self.focus_group_of(id).filter(|&group| scope.is_none_or(|scope| self.is_in_modal_scope(group, scope)));
        let mut groups: FxHashMap<NodeId, Vec<NodeId>> = FxHashMap::default();
        for &node in &candidates {
            if let Some(group) = group_of(node) {
                groups.entry(group).or_default().push(node);
            }
        }

        let tab_index = |node| self.semantics(node).map_or(0, |slot| slot.tab_index);
        let mut stops = Vec::new();
        for node in candidates.into_iter().filter(|&node| tab_index(node) >= 0) {
            let Some(group) = group_of(node) else {
                stops.push(node);
                continue;
            };
            let Some(members) = groups.remove(&group) else { continue };
            let is_stop = |id| tab_index(id) >= 0 && members.contains(&id);
            let stop = focused
                .filter(|id| members.contains(id))
                .or_else(|| members.iter().copied().find(|&id| tab_index(id) >= 0 && self.is_focus_preferred(id)))
                .or_else(|| self.semantics(group).and_then(|slot| slot.group_current).filter(|&id| is_stop(id)))
                .unwrap_or(node);
            stops.push(stop);
        }
        stops.sort_by_key(|&node| {
            let index = tab_index(node);
            (index <= 0, index.max(0))
        });
        stops
    }

    /// Whether logical portal order or a layer focus policy should handle browser Tab.
    pub fn manages_tab(&self) -> bool {
        self.shown_layers().any(|(node, policy)| {
            policy.inert_outside
                || policy.focus_trap.intersects(FocusTrapFeatures::TAB_LOCK | FocusTrapFeatures::FOCUS_LOCK)
                || self.anchored_layer_owner(node).is_some()
        })
    }

    /// The next Tab stop. A layer wraps traversal only when it has a Tab lock.
    pub fn tab_target(&self, forward: bool) -> Option<NodeId> {
        let focused = self.focus();
        let modal = self.front_modal();
        let tab_lock = self.focus_layer(modal, FocusTrapFeatures::TAB_LOCK);
        let focus_lock = self.focus_layer(modal, FocusTrapFeatures::FOCUS_LOCK);
        let order: Vec<_> = self
            .focus_order(tab_lock.or(focus_lock).or(modal), focused)
            .into_iter()
            .filter(|&node| {
                modal.is_none_or(|modal| self.is_in_modal_scope(node, modal))
                    && focus_lock.is_none_or(|lock| self.is_in_interaction_scope(node, lock))
            })
            .collect();
        if order.is_empty() {
            return tab_lock.filter(|&node| self.is_focusable(node));
        }
        let wraps = tab_lock.is_some() || !self.manages_tab();
        let position = focused.and_then(|focus| order.iter().position(|&id| id == focus));
        match (position, forward) {
            (Some(position), true) => order.get(position + 1).or_else(|| order.first().filter(|_| wraps)).copied(),
            (Some(position), false) if position > 0 => order.get(position - 1).copied(),
            (Some(_), false) if !wraps => None,
            (None, true) => order.first().copied(),
            (_, false) => order.last().copied(),
        }
    }

    /// Directional navigation stays in the same focus group and modal scope.
    pub fn group_step(&self, focused: NodeId, arrow: Arrow) -> Option<NodeId> {
        let group = self.focus_group_of(focused)?;
        let policy = self.semantics(group)?.focus_group?;
        let members: Vec<_> = self.group_descendants(group).filter(|&node| self.is_focusable(node)).collect();
        let at = members.iter().position(|&id| id == focused)?;
        let step = |forward| match forward {
            true => members.get(at + 1).or_else(|| policy.wraps.then(|| &members[0])),
            false => at.checked_sub(1).and_then(|at| members.get(at)).or_else(|| members.last().filter(|_| policy.wraps)),
        };
        match arrow {
            Arrow::Home => members.first(),
            Arrow::End => members.last(),
            Arrow::Left if policy.axis != Axis::Vertical => step(false),
            Arrow::Right if policy.axis != Axis::Vertical => step(true),
            Arrow::Up if policy.axis != Axis::Horizontal => step(false),
            Arrow::Down if policy.axis != Axis::Horizontal => step(true),
            _ => None,
        }
        .copied()
    }

    fn group_descendants(&self, group: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let mut pending: Vec<_> = self.children(group).iter().rev().copied().collect();
        std::iter::from_fn(move || {
            while let Some(node) = pending.pop() {
                if !self.is_locally_shown(node) || self.nodes[node].flags.contains(NodeFlags::INERT) {
                    continue;
                }
                if self.semantics(node).is_none_or(|slot| slot.focus_group.is_none()) {
                    pending.extend(self.children(node).iter().rev().copied());
                }
                return Some(node);
            }
            None
        })
    }

    /// Keeps the browser's roving Tab stop synchronized with live group members.
    pub(crate) fn reconcile_group_stops(&mut self, delta: &mut TreeDelta) {
        let groups: FxHashSet<_> = delta
            .semantics_dirty
            .iter()
            .chain(&delta.topology_dirty)
            .flat_map(|&node| {
                let own = self.semantics(node).and_then(|slot| slot.focus_group).map(|_| node);
                own.into_iter().chain(self.focus_group_of(node))
            })
            .collect();
        for group in groups {
            let current = self.tables.semantics[group].group_current;
            let next = self.focus().filter(|&node| self.focus_group_of(node) == Some(group)).or_else(|| {
                // Keep a group's representative while its container is hidden or a layer temporarily owns Tab.
                let mut first = None;
                let mut previous = None;
                for node in self.group_descendants(group).filter(|&node| {
                    self.nodes[node].flags.contains(NodeFlags::FOCUSABLE)
                        && !self.nodes[node].state.contains(State::DISABLED)
                        && self.semantics(node).is_none_or(|slot| slot.tab_index >= 0)
                }) {
                    if self.is_focus_preferred(node) {
                        return Some(node);
                    }
                    first.get_or_insert(node);
                    if Some(node) == current {
                        previous = Some(node);
                    }
                }
                previous.or(first)
            });
            if current != next {
                self.tables.semantics[group].group_current = next;
                self.semantics_changed(group, delta);
                for node in current.into_iter().chain(next) {
                    if self.is_live(node) {
                        self.semantics_changed(node, delta);
                    }
                }
            }
        }
    }

    /// The group that requests activation after a directional focus move.
    pub fn group_activation(&self, from: NodeId, to: NodeId) -> Option<NodeId> {
        let group = self.focus_group_of(from)?;
        let policy = self.semantics(group)?.focus_group?;
        let already_preferred = from == to && self.is_focus_preferred(to);
        (policy.activates && !already_preferred && self.focus_group_of(to) == Some(group)).then_some(group)
    }

    /// The layer's first autofocus control, first Tab stop, or container.
    pub fn initial_focus(&self, layer: NodeId) -> Option<NodeId> {
        let eligible = |id| self.is_logically_owned_by(id, layer) && self.is_focusable(id);
        self.ordered_focusable(Some(layer))
            .into_iter()
            .find(|&id| eligible(id) && self.node(id).is_some_and(|core| core.flags.contains(NodeFlags::AUTOFOCUS)))
            .or_else(|| self.focus_order(Some(layer), None).into_iter().find(|&id| eligible(id)))
            .or_else(|| self.is_focusable(layer).then_some(layer))
    }
}

struct LayerFocus {
    node: NodeId,
    features: FocusTrapFeatures,
    openers: Vec<NodeId>,
    had_focus: bool,
}

#[derive(Default)]
pub(crate) struct FocusRestore {
    layers: Vec<LayerFocus>,
}

impl FocusRestore {
    pub(crate) fn remember(&mut self, tree: &Tree, focused: Option<NodeId>) {
        for entry in &mut self.layers {
            entry.had_focus = focused.is_some_and(|node| tree.is_logically_owned_by(node, entry.node));
        }
    }

    /// Compute a focus change from the resulting authored tree while preserving opener history.
    pub(crate) fn reconcile(&mut self, tree: &Tree) -> Option<Option<NodeId>> {
        let focused = tree.focus();
        let shown: Vec<_> = tree.shown_layers().collect();
        let modal = shown.iter().rev().find(|(_, policy)| policy.inert_outside).map(|(id, _)| *id);
        let mut old = std::mem::take(&mut self.layers);
        let previous_openers = old.iter().rev().find(|entry| entry.had_focus).map_or_else(Vec::new, |entry| entry.openers.clone());
        let mut initial = None;
        for (node, policy) in shown {
            let entry = if let Some(index) = old.iter().position(|entry| entry.node == node) {
                LayerFocus { features: policy.focus_trap, ..old.remove(index) }
            } else {
                if policy.focus_trap.contains(FocusTrapFeatures::INITIAL_FOCUS)
                    && modal.is_none_or(|modal| tree.is_in_modal_scope(node, modal))
                {
                    initial = Some(node);
                }
                LayerFocus {
                    node,
                    features: policy.focus_trap,
                    openers: focused
                        .filter(|&focus| !tree.is_logically_owned_by(focus, node))
                        .into_iter()
                        .chain(previous_openers.iter().copied())
                        .collect(),
                    had_focus: false,
                }
            };
            self.layers.push(entry);
        }
        let next = initial
            .and_then(|node| tree.initial_focus(node))
            .or_else(|| {
                old.iter()
                    .rev()
                    .filter(|entry| entry.had_focus && entry.features.contains(FocusTrapFeatures::RESTORE_FOCUS))
                    .flat_map(|entry| entry.openers.iter().copied())
                    .find(|&node| tree.is_focusable(node))
            })
            .or_else(|| focused.filter(|&node| tree.focusable_in(node, modal)))
            .or_else(|| {
                self.layers
                    .iter()
                    .rev()
                    .filter(|entry| entry.had_focus && entry.features.contains(FocusTrapFeatures::AUTO_FOCUS))
                    .map(|entry| entry.node)
                    .find(|&node| tree.is_focusable(node))
            });
        (next != focused).then_some(next)
    }
}

//! Overlay order and logical ownership shared by focus, pointer, and accessibility.

use crate::{HandlerKind, NodeFlags, Tree};
use octara_model::{
    identity::NodeId,
    style::{Display, State},
};

pub use octara_model::overlay::{FocusTrapFeatures, LayerGroupId, LayerPolicy};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverlayMount {
    /// The authored control that owns the overlay.
    pub owner: NodeId,
    /// The registration order used when mounting overlay siblings.
    pub order: u64,
}

impl Tree {
    /// True when an attached node, its physical ancestors, and its portal owners are displayed.
    pub fn is_shown(&self, node: NodeId) -> bool {
        let mut at = Some(node);
        while let Some(id) = at {
            if !self.is_locally_shown(id) || !self.overlay_target_shown(id) {
                return false;
            }
            if id == self.root() {
                return true;
            }
            at = self.physical_parent(id);
        }
        false
    }

    pub(crate) fn is_locally_shown(&self, node: NodeId) -> bool {
        self.is_live(node) && self.style(node).is_none_or(|slot| slot.layout.display != Display::None)
    }

    pub(crate) fn overlay_target_shown(&self, node: NodeId) -> bool {
        self.overlay(node).is_none_or(|slot| {
            let target_shown =
                |target| self.logical_ancestors(target).take_while(|&id| self.is_locally_shown(id)).last() == Some(self.root());
            slot.mount.is_none_or(|mount| target_shown(mount.owner))
                && slot.target.and_then(|(target, _)| target.node()).is_none_or(target_shown)
        })
    }

    /// Physical containment, as used by the layout tree.
    pub fn is_within(&self, node: NodeId, ancestor: NodeId) -> bool {
        self.node(ancestor).is_some() && self.ancestor_matching(node, |id| id == ancestor).is_some()
    }

    pub(crate) fn ancestor_matching(&self, node: NodeId, predicate: impl Fn(NodeId) -> bool) -> Option<NodeId> {
        let mut at = Some(node);
        while let Some(id) = at.filter(|&id| self.node(id).is_some()) {
            if predicate(id) {
                return Some(id);
            }
            at = self.physical_parent(id);
        }
        None
    }

    pub(crate) fn logical_ancestors(&self, node: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        std::iter::successors(self.node(node).map(|_| node), |&id| self.logical_parent(id))
    }

    /// Whether a node is contained by an authored branch, including its nested portals.
    pub fn is_logically_owned_by(&self, node: NodeId, owner: NodeId) -> bool {
        self.logical_ancestors(node).any(|id| id == owner)
    }

    pub fn is_in_interaction_scope(&self, node: NodeId, owner: NodeId) -> bool {
        self.is_logically_owned_by(node, owner)
            || self.layer_trigger(owner).is_some_and(|trigger| self.is_logically_owned_by(node, trigger))
            || self.layer_group(owner).is_some_and(|group| {
                self.logical_ancestors(node).any(|id| self.layer_group(id) == Some(group))
                    || self.overlay_nodes().any(|id| {
                        self.layer_group(id) == Some(group)
                            && self.is_shown(id)
                            && self.layer_trigger(id).is_some_and(|trigger| self.is_logically_owned_by(node, trigger))
                    })
            })
    }

    pub fn layer_group(&self, node: NodeId) -> Option<LayerGroupId> {
        let slot = self.overlay(node)?;
        slot.layer.and_then(|layer| layer.group).or(slot.group)
    }

    pub fn is_in_modal_scope(&self, node: NodeId, modal: NodeId) -> bool {
        self.is_in_interaction_scope(node, modal)
    }

    /// Shown modal layers in back-to-front stacking order.
    pub fn modal_layers(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.shown_layers().filter_map(|(node, layer)| layer.inert_outside.then_some(node))
    }

    pub fn front_modal(&self) -> Option<NodeId> {
        let mut shown = self
            .overlay_nodes()
            .filter(|&node| self.overlay(node).and_then(|slot| slot.layer).is_some_and(|layer| layer.inert_outside) && self.is_shown(node));
        let first = shown.next()?;
        let Some(second) = shown.next() else { return Some(first) };
        [first, second].into_iter().chain(shown).max_by_key(|&node| self.shown_path(node))
    }

    pub(crate) fn shown_layers(&self) -> impl Iterator<Item = (NodeId, LayerPolicy)> + '_ {
        let mut layers: Vec<_> = self
            .overlay_nodes()
            .filter_map(|id| {
                let layer = self.overlay(id)?.layer?;
                Some((self.shown_path(id)?, id, layer))
            })
            .collect();
        layers.sort_unstable_by(|(a, _, _), (b, _, _)| a.cmp(b));
        layers.into_iter().map(|(_, id, layer)| (id, layer))
    }

    fn shown_path(&self, mut node: NodeId) -> Option<Vec<((bool, i32), usize)>> {
        let mut path = Vec::new();
        loop {
            if !self.is_locally_shown(node) || !self.overlay_target_shown(node) {
                return None;
            }
            if node == self.root() {
                path.reverse();
                return Some(path);
            }
            let parent = self.physical_parent(node)?;
            let ordinal = self.children(parent).iter().position(|&child| child == node)?;
            path.push((self.stacking_key(node), ordinal));
            node = parent;
        }
    }

    pub(crate) fn front_layer(&self, modal: Option<NodeId>, eligible: impl Fn(NodeId, LayerPolicy) -> bool) -> Option<NodeId> {
        self.shown_layers()
            .filter(|&(node, policy)| modal.is_none_or(|modal| self.is_in_modal_scope(node, modal)) && eligible(node, policy))
            .last()
            .map(|(node, _)| node)
    }

    /// The highest eligible layer inside the modal barrier can consume an outside gesture.
    pub(crate) fn outside_dismissal(&self, hit: Option<NodeId>, modal: Option<NodeId>) -> Option<NodeId> {
        let top =
            self.front_layer(modal, |node, policy| policy.dismiss_on_outside_press && self.handler(node, HandlerKind::Dismiss).is_some())?;
        hit.is_none_or(|hit| !self.is_in_interaction_scope(hit, top)).then_some(top)
    }

    fn layer_portal(&self, layer: NodeId) -> Option<&crate::tables::OverlaySlot> {
        if self.overlay(layer)?.layer?.inert_outside {
            return None;
        }
        let mut at = layer;
        loop {
            if let Some(slot) = self.overlay(at) {
                if at != layer && slot.layer.is_some() {
                    return None;
                }
                if slot.mount.is_some() {
                    return Some(slot);
                }
            }
            at = self.physical_parent(at)?;
        }
    }

    fn layer_trigger(&self, layer: NodeId) -> Option<NodeId> {
        let portal = self.layer_portal(layer)?;
        portal.mount.map(|mount| mount.owner)
    }

    /// The authored owner of a nonmodal layer relocated for Tab traversal.
    pub(crate) fn anchored_layer_owner(&self, layer: NodeId) -> Option<NodeId> {
        self.layer_portal(layer)?.target?;
        self.layer_trigger(layer)
    }

    /// Whether a direct user interaction may target this node in the active modal scope.
    pub fn is_interactive(&self, node: NodeId) -> bool {
        self.interactive_in(node, self.front_modal())
    }

    /// Whether the node or an authored ancestor blocks interaction and accessibility.
    pub fn is_inert(&self, node: NodeId) -> bool {
        self.logical_ancestors(node).any(|id| self.nodes[id].flags.contains(NodeFlags::INERT))
    }

    /// Whether the node or an authored ancestor is disabled.
    pub fn is_disabled(&self, node: NodeId) -> bool {
        self.logical_ancestors(node).any(|id| self.nodes[id].state.contains(State::DISABLED))
    }

    pub(crate) fn interactive_in(&self, node: NodeId, modal: Option<NodeId>) -> bool {
        self.is_shown(node)
            && !self.logical_ancestors(node).any(|id| {
                let core = &self.nodes[id];
                core.flags.contains(NodeFlags::INERT) || core.state.contains(State::DISABLED)
            })
            && modal.is_none_or(|modal| self.is_in_modal_scope(node, modal))
    }
}

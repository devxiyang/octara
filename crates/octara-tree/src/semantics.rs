//! Accessibility scope shared by native and Web projections.

use octara_model::identity::NodeId;
use rustc_hash::FxHashSet;

use crate::{NodeFlags, Tree};

/// A snapshot of accessible nodes and the physical ancestors needed to reach them.
/// During a modal, ancestors outside the active scope carry topology only.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SemanticScope {
    pub modal: Option<NodeId>,
    members: FxHashSet<NodeId>,
    ancestors: FxHashSet<NodeId>,
}

impl SemanticScope {
    /// Whether the node may expose its own accessibility properties.
    pub fn exposes(&self, node: NodeId) -> bool {
        self.members.contains(&node)
    }

    /// Whether the projection needs the node for its semantics or for topology.
    pub fn includes(&self, node: NodeId) -> bool {
        self.exposes(node) || self.ancestors.contains(&node)
    }

    /// Iterate exposed nodes and their required physical ancestors once each.
    pub fn included(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.members.iter().copied().chain(self.ancestors.iter().copied().filter(|&node| !self.exposes(node)))
    }
}

impl Tree {
    /// Whether accessibility can traverse this node, before considering its ancestors or a modal.
    pub fn is_locally_accessible(&self, node: NodeId) -> bool {
        self.is_locally_shown(node)
            && self.overlay_target_shown(node)
            && self.node(node).is_some_and(|core| !core.flags.intersects(NodeFlags::HIDDEN | NodeFlags::INERT))
            && (node != self.overlay_root() || !self.children(node).is_empty())
    }

    /// Whether a node is accessible when the window has no active modal.
    pub fn is_accessible_without_modal(&self, mut node: NodeId) -> bool {
        while self.is_locally_accessible(node) {
            if node == self.root() {
                return true;
            }
            let Some(parent) = self.parent(node) else { break };
            node = parent;
        }
        false
    }

    pub fn semantic_scope(&self) -> SemanticScope {
        self.semantic_scope_with_modal(self.front_modal())
    }

    /// Build a scope with a previously resolved front modal.
    pub fn semantic_scope_with_modal(&self, modal: Option<NodeId>) -> SemanticScope {
        let mut scope = SemanticScope { modal, ..SemanticScope::default() };
        let mut pending = vec![self.root()];
        while let Some(node) = pending.pop() {
            if !self.is_locally_accessible(node) {
                continue;
            }
            if modal.is_none_or(|modal| self.is_in_modal_scope(node, modal)) {
                scope.members.insert(node);
            }
            pending.extend(self.children(node));
        }

        scope.ancestors.insert(self.root());
        for &node in &scope.members {
            let mut parent = self.parent(node);
            while let Some(id) = parent {
                if scope.members.contains(&id) || !scope.ancestors.insert(id) {
                    break;
                }
                parent = self.parent(id);
            }
        }
        scope
    }
}

//! Visual exit participation and logical retirement of mounted subtrees.

use crate::{ExitTicket, NodeFlags, NodeId, RetiredNode, Retirement, Tree, TreeDelta};
use octara_model::motion::Curve;
use octara_model::style::State;
use rustc_hash::{FxHashMap, FxHashSet};

impl Tree {
    pub fn pending_exit(&self, node: NodeId) -> Option<ExitTicket> {
        self.pending_exits.contains_key(&node).then_some(ExitTicket { tree: self.id(), node })
    }

    pub fn accepts_exit(&self, ticket: ExitTicket) -> bool {
        self.pending_exit(ticket.node) == Some(ticket)
    }

    pub(crate) fn finish_exit(&mut self, ticket: ExitTicket, delta: &mut TreeDelta) {
        if !self.accepts_exit(ticket) {
            return;
        }
        let root = self.pending_exits.remove(&ticket.node).expect("validated exit ticket");
        let count = self.exit_roots.get_mut(&root).expect("exit barrier root");
        *count -= 1;
        if *count == 0 {
            self.destroy_subtree(root, delta);
        }
    }

    pub(crate) fn retire_subtrees(&mut self, order: Vec<NodeId>, requests: &FxHashMap<NodeId, Retirement>, delta: &mut TreeDelta) {
        for &node in &order {
            if requests[&node] == Retirement::Destroy {
                self.destroy_subtree(node, delta);
            }
        }
        for node in order {
            if requests[&node] != Retirement::Destroy && self.node(node).is_some_and(|core| !core.flags.contains(NodeFlags::EXITING)) {
                self.begin_exit(node, requests, delta);
            }
        }
    }

    fn begin_exit(&mut self, root: NodeId, requests: &FxHashMap<NodeId, Retirement>, delta: &mut TreeDelta) {
        let nodes = self.retirement_subtree(root);
        let mode = requests[&root];
        if mode == Retirement::VisibleExit
            && !nodes.iter().any(|&node| {
                self.pending_exits.contains_key(&node) || (self.exit_participant(node, requests) && self.has_exit_motion(node))
            })
        {
            self.destroy_subtree(root, delta);
            return;
        }
        let mut count = 0;
        for node in nodes {
            self.exit_roots.remove(&node);
            if let Some(barrier) = self.pending_exits.get_mut(&node) {
                *barrier = root;
                count += 1;
            }
            if self.nodes[node].flags.contains(NodeFlags::EXITING) {
                continue;
            }

            let parent = self.physical_parent(node);
            if self.exit_participant(node, requests) {
                self.pending_exits.insert(node, root);
                count += 1;
                let ticket = ExitTicket { tree: self.id(), node };
                let visible = requests.get(&node).copied().unwrap_or(mode) == Retirement::VisibleExit;
                delta.retired.push(RetiredNode::Exiting { ticket, visible });
            }

            let core = &mut self.nodes[node];
            core.flags.remove(NodeFlags::ATTACHED);
            core.flags.insert(NodeFlags::EXITING);
            core.state.remove(State::PRESSED | State::HOVER | State::DRAG_OVER | State::DRAGGING | State::DROP_HOVER);
            self.index_names(node, false);
            self.semantics_changed(node, delta);
            self.paint_changed(node, delta);
            if let Some(parent) = parent {
                self.remove_row_index(parent, node);
                delta.topology_dirty.insert(parent);
                self.layout_changed(parent, delta);
            }
        }
        self.exit_roots.insert(root, count);
    }

    fn exit_participant(&self, node: NodeId, requests: &FxHashMap<NodeId, Retirement>) -> bool {
        requests.contains_key(&node)
            || self.overlay(node).is_some_and(|slot| slot.mount.is_some())
            || (self.style(node).is_some_and(|style| style.opacity_motion_authored) && self.has_exit_motion(node))
    }

    fn has_exit_motion(&self, node: NodeId) -> bool {
        let duration = match self.visual(node).motion.opacity {
            Curve::Instant => return false,
            Curve::Fade(duration) => duration,
            Curve::Spring(spring) => spring.duration,
        };
        duration.is_finite() && duration > 0.0
    }

    pub(crate) fn cancel_replaced_overlay_exits(&mut self, delta: &mut TreeDelta) {
        if self.exit_roots.is_empty() {
            return;
        }
        let mounts: FxHashSet<_> = delta
            .overlay_dirty
            .iter()
            .filter(|&node| self.is_live(node))
            .filter_map(|node| self.overlay(node)?.mount.map(|mount| (mount.owner, mount.order)))
            .collect();
        if mounts.is_empty() {
            return;
        }
        let mut stale: Vec<_> = self
            .overlay_index
            .iter()
            .copied()
            .filter(|&node| {
                self.nodes[node].flags.contains(NodeFlags::EXITING)
                    && self.overlay(node).and_then(|slot| slot.mount).is_some_and(|mount| mounts.contains(&(mount.owner, mount.order)))
            })
            .collect();
        stale.sort_unstable();
        for node in stale {
            self.destroy_subtree(node, delta);
        }
    }

    fn retirement_subtree(&self, root: NodeId) -> Vec<NodeId> {
        if self.node(root).is_none() || self.overlay_index.is_empty() {
            return self.subtree(root).collect();
        }
        let mut nodes = Vec::new();
        let mut seen = FxHashSet::default();
        let mut pending = vec![root];
        while let Some(node) = pending.pop() {
            if seen.insert(node) {
                nodes.push(node);
                let mut owned = smallvec::SmallVec::<[NodeId; 2]>::from_slice(self.logical_portals(node));
                owned.sort_unstable();
                pending.extend(owned.into_iter().rev());
                pending.extend(self.children(node).iter().rev().copied());
            }
        }
        nodes
    }

    fn destroy_subtree(&mut self, root: NodeId, delta: &mut TreeDelta) {
        let nodes = self.retirement_subtree(root);
        let members: FxHashSet<_> = nodes.iter().copied().collect();
        let mut settled = Vec::new();
        // Exiting portals keep their ownership until the visual exit settles.
        // Remove their edges before clearing any of the retained allocations.
        for &node in &nodes {
            self.unindex_portal(node);
        }
        for &node in &nodes {
            if self.physical_parent(node).is_none_or(|parent| !members.contains(&parent)) {
                self.detach(node, delta);
            }
            self.exit_roots.remove(&node);
            if let Some(barrier) = self.pending_exits.remove(&node)
                && !members.contains(&barrier)
            {
                let count = self.exit_roots.get_mut(&barrier).expect("exit barrier root");
                *count -= 1;
                if *count == 0 {
                    settled.push(barrier);
                }
            }
        }
        for node in nodes.into_iter().rev() {
            self.clear_node(node, delta);
        }
        for barrier in settled {
            self.destroy_subtree(barrier, delta);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tables::AuthoredStyle;
    use crate::transaction::OverlaySpec;
    use crate::{NodeChange, NodeContent, NodeSpec, RuntimeOp, TreeId};
    use octara_model::style::StyleProp;

    fn tree() -> Tree {
        Tree::new(TreeId::new([21, 45], 1), Default::default())
    }

    fn fade() -> NodeChange {
        NodeChange::Style(Box::new(AuthoredStyle {
            base: [StyleProp::OpacityMotion(Curve::Fade(0.2))].into_iter().collect(),
            ..Default::default()
        }))
    }

    #[test]
    fn parent_waits_for_a_child_whose_exit_started_in_an_earlier_transaction() {
        let mut tree = tree();
        let mut mount = tree.transaction();
        let parent = mount.create(NodeSpec::new(NodeContent::Box));
        let child = mount.create(NodeSpec::new(NodeContent::Box));
        let grandchild = mount.create(NodeSpec::new(NodeContent::Box));
        mount.change(parent, fade());
        mount.change(child, fade());
        mount.place(tree.node_ref(tree.root()), 0, parent);
        mount.place(parent, 0, child);
        mount.place(child, 0, grandchild);
        mount.change(grandchild, NodeChange::TestId(Some("grandchild".into())));
        let (receipt, _) = tree.apply(mount.finish()).unwrap();
        let [parent, child, grandchild] = [parent, child, grandchild].map(|node| receipt.node(node).unwrap());

        let mut close = tree.transaction();
        close.retire(tree.node_ref(child), Retirement::VisibleExit);
        tree.apply(close.finish()).unwrap();
        let child_exit = tree.pending_exit(child).unwrap();
        assert!(tree.test_ids("grandchild").is_empty());
        assert!(!tree.contains_target(tree.guard(grandchild)));

        let mut close = tree.transaction();
        close.retire(tree.node_ref(parent), Retirement::VisibleExit);
        let (_, delta) = tree.apply(close.finish()).unwrap();
        let parent_exit = tree.pending_exit(parent).unwrap();
        assert_eq!(delta.retired, [RetiredNode::Exiting { ticket: parent_exit, visible: true }]);
        assert_eq!(tree.pending_exit(child), Some(child_exit));
        assert!(tree.validate_integrity().is_ok());

        let wrong_tree = ExitTicket { tree: tree.id().next_generation().unwrap(), ..child_exit };
        assert!(tree.apply_runtime(RuntimeOp::FinishExit(wrong_tree)).is_empty());
        assert!(tree.apply_runtime(RuntimeOp::FinishExit(parent_exit)).is_empty());
        assert!(tree.apply_runtime(RuntimeOp::FinishExit(parent_exit)).is_empty());
        assert!(tree.node(parent).is_some());
        assert!(tree.node(grandchild).is_some());
        let complete = tree.apply_runtime(RuntimeOp::FinishExit(child_exit));
        assert_eq!(complete.retired, [grandchild, child, parent].map(RetiredNode::Destroyed));
        assert!(tree.validate_integrity().is_ok());
    }

    #[test]
    fn reopening_a_portal_cancels_the_old_barrier_and_its_nested_portal_tickets() {
        let mut tree = tree();
        let mut mount = tree.transaction();
        let owner = mount.create(NodeSpec::new(NodeContent::Box));
        let parent = mount.create(NodeSpec::new(NodeContent::Box));
        let child = mount.create(NodeSpec::new(NodeContent::Box));
        let layer = |owner, order| NodeChange::Overlay(OverlaySpec { mount: Some((owner, order)), ..Default::default() });
        mount.change(parent, fade());
        mount.change(child, fade());
        let overlays = tree.node_ref(tree.overlay_root());
        mount.place(tree.node_ref(tree.root()), 0, owner);
        mount.place(overlays, 0, parent);
        mount.change(parent, layer(owner.into(), 17));
        mount.place(overlays, 1, child);
        mount.change(child, layer(parent.into(), 18));
        let (receipt, _) = tree.apply(mount.finish()).unwrap();
        let [owner, parent, child] = [owner, parent, child].map(|node| receipt.node(node).unwrap());

        let mut close = tree.transaction();
        close.retire(tree.node_ref(parent), Retirement::VisibleExit);
        tree.apply(close.finish()).unwrap();
        let parent_exit = tree.pending_exit(parent).unwrap();
        let child_exit = tree.pending_exit(child).unwrap();
        assert!(!tree.contains_target(tree.guard(child)));
        assert!(tree.apply_runtime(RuntimeOp::FinishExit(child_exit)).is_empty());

        let mut reopen = tree.transaction();
        let replacement = reopen.create(NodeSpec::new(NodeContent::Box));
        reopen.place(overlays, 0, replacement);
        reopen.change(replacement, layer(tree.node_ref(owner), 17));
        let (receipt, delta) = tree.apply(reopen.finish()).unwrap();
        let replacement = receipt.node(replacement).unwrap();
        assert!(delta.retired.contains(&RetiredNode::Destroyed(parent)));
        assert!(delta.retired.contains(&RetiredNode::Destroyed(child)));
        assert!(tree.apply_runtime(RuntimeOp::FinishExit(parent_exit)).is_empty());
        assert!(tree.apply_runtime(RuntimeOp::FinishExit(child_exit)).is_empty());
        assert!(tree.contains_target(tree.guard(replacement)));
        assert!(tree.validate_integrity().is_ok());
    }
}

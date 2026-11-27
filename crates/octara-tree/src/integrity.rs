//! Retained-tree consistency checks used by deterministic simulation.

use crate::{
    NodeFlags, NodeId, RowKey, Tree,
    tables::{HandlerId, HandlerKind},
};
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::Arc;

impl Tree {
    /// Checks topology and both identity indices against the retained nodes.
    pub fn validate_integrity(&self) -> Result<(), &'static str> {
        if !self.is_live(self.root)
            || self.physical_parent(self.root).is_some()
            || self.physical_parent(self.overlay_root) != Some(self.root)
        {
            return Err("invalid tree root or overlay root");
        }
        let mut incoming = FxHashSet::default();
        for (parent, _) in self.nodes() {
            for &child in self.children(parent) {
                if self.physical_parent(child) != Some(parent) || !incoming.insert(child) {
                    return Err("child is missing, duplicated, or has the wrong parent");
                }
            }
        }

        let mut rows = FxHashMap::<(NodeId, RowKey), NodeId>::default();
        let mut portals = FxHashMap::<NodeId, FxHashSet<NodeId>>::default();
        let mut ids = FxHashMap::<_, FxHashSet<NodeId>>::default();
        let mut test_ids = FxHashMap::<Arc<str>, FxHashSet<NodeId>>::default();
        let mut roots = Vec::new();
        for (node, core) in self.nodes() {
            if self.tables.presence(node) != core.present {
                return Err("node presence mask does not match its cold tables");
            }
            let mount = self.overlay(node).and_then(|slot| slot.mount);
            if core.flags.contains(NodeFlags::HAS_OVERLAY_MOUNT) != mount.is_some() {
                return Err("portal flag does not match its overlay mount");
            }
            if let Some(mount) = mount {
                if self.node(mount.owner).is_none() {
                    return Err("portal owner is missing");
                }
                portals.entry(mount.owner).or_default().insert(node);
            }
            if incoming.contains(&node) != core.parent.is_some() {
                return Err("parent has no corresponding child");
            }
            if core.parent.is_none() {
                roots.push(node);
            }
            let exiting = core.flags.contains(NodeFlags::EXITING);
            let attached = node == self.root || (!exiting && core.parent.is_some_and(|parent| self.is_live(parent)));
            if self.is_live(node) != attached || (exiting && core.flags.contains(NodeFlags::ATTACHED)) {
                return Err("attachment does not match the topology");
            }
            let Some(provenance) = self.provenance(node) else { continue };
            if attached && let Some(id) = &provenance.id {
                ids.entry(id.clone()).or_default().insert(node);
            }
            if attached && let Some(test_id) = &provenance.test_id {
                test_ids.entry(test_id.clone()).or_default().insert(node);
            }
            if !exiting
                && let Some((parent, key)) = core.parent.zip(provenance.row_key.as_ref())
                && rows.insert((parent, key.clone()), node).is_some()
            {
                return Err("two children claim the same row key");
            }
        }
        let reachable = roots.into_iter().map(|root| self.subtree(root).count()).sum::<usize>();
        if reachable != self.nodes.len() {
            return Err("retained topology contains a cycle");
        }
        if portals.len() != self.logical_portals.len()
            || self.logical_portals.iter().any(|(owner, nodes)| {
                portals
                    .get(&owner)
                    .is_none_or(|expected| nodes.len() != expected.len() || &nodes.iter().copied().collect::<FxHashSet<_>>() != expected)
            })
        {
            return Err("logical portal index does not match retained portal mounts");
        }
        if self.nodes().any(|(node, core)| self.subtree_style_bloom(node) != core.style_bloom) {
            return Err("style sensitivity does not match the subtree");
        }
        for (index, expected) in [(&self.test_id_index, test_ids), (&self.id_index, ids)] {
            if index.len() != expected.len()
                || index.iter().any(|(key, nodes)| {
                    expected.get(key).is_none_or(|expected| {
                        expected.len() != nodes.len()
                            || nodes.iter().collect::<FxHashSet<_>>().len() != nodes.len()
                            || nodes.iter().any(|node| !expected.contains(node))
                    })
                })
            {
                return Err("name index does not match attached nodes");
            }
        }
        if rows != self.row_children {
            return Err("row index does not match retained children");
        }
        let mut exits = FxHashMap::<NodeId, usize>::default();
        for (&node, &root) in &self.pending_exits {
            if !self.nodes.get(node).is_some_and(|core| core.flags.contains(NodeFlags::EXITING)) {
                return Err("visual exit has no exiting allocation");
            }
            *exits.entry(root).or_default() += 1;
        }
        if exits != self.exit_roots
            || exits.keys().any(|&root| !self.nodes.get(root).is_some_and(|core| core.flags.contains(NodeFlags::EXITING)))
        {
            return Err("exit barrier does not match its pending visual exits");
        }
        Ok(())
    }

    pub fn handlers(&self, node: NodeId) -> impl Iterator<Item = (HandlerKind, HandlerId)> + '_ {
        self.handler_slot(node).into_iter().flat_map(|handlers| handlers.callbacks.iter().copied())
    }
}

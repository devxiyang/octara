//! Retained projection tables with separate cycle undo and per-round diff journals.

use octara_tree::{DirtyBitmap, NodeId};
use slotmap::SecondaryMap;

/// Each slot has one active generation, with all displaced values retained for rollback.
pub(crate) struct DenseProjected<T> {
    entries: SecondaryMap<NodeId, T>,
    occupied: DirtyBitmap,
    undo_log: Vec<(NodeId, Option<T>)>,
    undo_recorded: DirtyBitmap,
    stage_log: Vec<(NodeId, Option<T>)>,
    stage_recorded: DirtyBitmap,
    recording: bool,
    recording_stage: bool,
}

impl<T> Default for DenseProjected<T> {
    fn default() -> Self {
        Self {
            entries: SecondaryMap::new(),
            occupied: DirtyBitmap::default(),
            undo_log: Vec::new(),
            undo_recorded: DirtyBitmap::default(),
            stage_log: Vec::new(),
            stage_recorded: DirtyBitmap::default(),
            recording: false,
            recording_stage: false,
        }
    }
}

impl<T> DenseProjected<T> {
    pub(crate) fn begin(&mut self) {
        self.recording = true;
    }

    pub(crate) fn begin_stage(&mut self) {
        self.begin();
        self.recording_stage = true;
        self.stage_log.clear();
        self.stage_recorded.clear();
    }

    pub(crate) fn get(&self, node: NodeId) -> Option<&T> {
        self.entries.get(node)
    }

    pub(crate) fn contains_key(&self, node: NodeId) -> bool {
        self.entries.contains_key(node)
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (NodeId, &T)> {
        self.entries.iter()
    }

    pub(crate) fn keys(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.entries.keys()
    }

    pub(crate) fn undo_keys(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.undo_log.iter().map(|&(node, _)| node)
    }

    pub(crate) fn stage_changes(&self) -> impl Iterator<Item = (NodeId, Option<&T>)> {
        self.stage_log.iter().map(|(node, old)| (*node, old.as_ref()))
    }

    fn insert_unlogged(&mut self, node: NodeId, value: T) -> Option<T> {
        self.occupied.insert(node);
        self.entries.insert(node, value)
    }

    fn remove_unlogged(&mut self, node: NodeId) -> Option<T> {
        let value = self.entries.remove(node)?;
        self.occupied.remove(node);
        Some(value)
    }

    pub(crate) fn commit(&mut self) {
        self.undo_log.clear();
        self.undo_recorded.clear();
        self.stage_log.clear();
        self.stage_recorded.clear();
        self.recording = false;
        self.recording_stage = false;
    }

    pub(crate) fn rollback(&mut self) {
        while let Some((node, old)) = self.undo_log.pop() {
            match old {
                Some(value) => self.insert_unlogged(node, value),
                None => self.remove_unlogged(node),
            };
        }
        self.commit();
    }
}

impl<T: Clone> DenseProjected<T> {
    pub(crate) fn insert(&mut self, node: NodeId, value: T) {
        if let Some(previous) = self.occupied.key_at_slot(node)
            && previous != node
        {
            if self.entries.entry(node).is_none() {
                return;
            }
            self.remove(previous);
        }
        let old = self.insert_unlogged(node, value);
        self.record(node, old);
    }

    pub(crate) fn remove(&mut self, node: NodeId) -> bool {
        let Some(old) = self.remove_unlogged(node) else { return false };
        self.record(node, Some(old));
        true
    }

    pub(crate) fn get_mut(&mut self, node: NodeId) -> Option<&mut T> {
        if (self.recording && !self.undo_recorded.contains(&node)) || (self.recording_stage && !self.stage_recorded.contains(&node)) {
            let old = self.entries.get(node)?.clone();
            self.record(node, Some(old));
        }
        self.entries.get_mut(node)
    }

    fn record(&mut self, node: NodeId, old: Option<T>) {
        let undo = self.recording && self.undo_recorded.insert(node);
        let stage = self.recording_stage && self.stage_recorded.insert(node);
        match (undo, stage) {
            (true, true) => {
                self.stage_log.push((node, old.clone()));
                self.undo_log.push((node, old));
            }
            (true, false) => self.undo_log.push((node, old)),
            (false, true) => self.stage_log.push((node, old)),
            (false, false) => {}
        }
    }
}

impl<T: Clone + PartialEq> DenseProjected<T> {
    pub(crate) fn set(&mut self, node: NodeId, value: Option<T>) -> bool {
        if self.entries.get(node) == value.as_ref() {
            return false;
        }
        match value {
            Some(value) => {
                self.insert(node, value);
                self.entries.contains_key(node)
            }
            None => self.remove(node),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slotmap::SlotMap;

    #[test]
    fn replacing_a_live_generation_preserves_its_value_for_rollback() {
        let mut nodes = SlotMap::<NodeId, ()>::with_key();
        let first = nodes.insert(());
        let mut table = DenseProjected::default();
        table.insert(first, 10);
        nodes.remove(first);
        let second = nodes.insert(());
        table.begin_stage();
        table.insert(second, 20);
        assert!(!table.set(first, Some(30)), "a stale key must not replace a newer generation");
        assert_eq!(table.stage_changes().collect::<Vec<_>>(), [(first, Some(&10)), (second, None)]);
        table.rollback();
        assert_eq!(table.get(first), Some(&10));
        assert!(!table.contains_key(second));

        table.begin();
        table.insert(second, 40);
        table.commit();
        assert_eq!(table.get(second), Some(&40));
        assert!(!table.contains_key(first));
    }

    #[test]
    fn journal_restores_the_cycle_start_across_rounds_and_slot_reuse() {
        let mut nodes = SlotMap::<NodeId, ()>::with_key();
        let a = nodes.insert(());
        let mut table = DenseProjected::default();
        table.insert(a, 10);
        table.begin_stage();
        assert_eq!(table.get_mut(a), Some(&mut 10));
        table.insert(a, 20);
        assert_eq!(table.stage_changes().collect::<Vec<_>>(), [(a, Some(&10))]);
        table.begin_stage();
        nodes.remove(a);
        table.remove(a);
        let b = nodes.insert(());
        table.insert(b, 30);
        assert_eq!(table.stage_changes().collect::<Vec<_>>(), [(a, Some(&20)), (b, None)]);
        table.rollback();
        assert_eq!(table.iter().collect::<Vec<_>>(), [(a, &10)]);
        assert!(table.stage_changes().next().is_none());
    }
}

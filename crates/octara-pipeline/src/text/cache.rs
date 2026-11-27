use octara_tree::NodeId;
use rustc_hash::FxHashMap;
use std::ops::Deref;

/// Records the first change to each node during a projection cycle.
pub(super) struct Cache<V> {
    entries: FxHashMap<NodeId, V>,
    previous: Option<FxHashMap<NodeId, Option<V>>>,
}

impl<V> Default for Cache<V> {
    fn default() -> Self {
        Self { entries: FxHashMap::default(), previous: None }
    }
}

impl<V> Cache<V> {
    pub(super) fn begin(&mut self) {
        self.previous.get_or_insert_with(FxHashMap::default);
    }

    pub(super) fn commit(&mut self) {
        self.previous = None;
    }

    pub(super) fn rollback(&mut self) {
        for (node, value) in self.previous.take().into_iter().flatten() {
            match value {
                Some(value) => self.entries.insert(node, value),
                None => self.entries.remove(&node),
            };
        }
    }

    pub(super) fn insert(&mut self, node: NodeId, value: V) {
        let old = self.entries.insert(node, value);
        if let Some(previous) = &mut self.previous {
            previous.entry(node).or_insert(old);
        }
    }

    pub(super) fn remove(&mut self, node: &NodeId) -> bool {
        let Some(old) = self.entries.remove(node) else { return false };
        if let Some(previous) = &mut self.previous {
            previous.entry(*node).or_insert(Some(old));
        }
        true
    }

    pub(super) fn get_mut(&mut self, node: &NodeId) -> Option<&mut V>
    where
        V: Clone,
    {
        let value = self.entries.get_mut(node)?;
        if let Some(previous) = &mut self.previous {
            previous.entry(*node).or_insert_with(|| Some(value.clone()));
        }
        Some(value)
    }
}

impl<V> Deref for Cache<V> {
    type Target = FxHashMap<NodeId, V>;

    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}

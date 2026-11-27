use std::collections::BTreeSet;

use crate::graph::FlushTier;

use super::{ReactiveHot, ReactiveId, ReactiveWorld, SCHEDULED};

const TIERS: usize = 4;
const SHALLOW_DEPTHS: u32 = u64::BITS;

#[derive(Default)]
pub(super) struct Pending {
    buckets: [Vec<Vec<u32>>; TIERS],
    nonempty: [u64; TIERS],
    unordered: [u64; TIERS],
    deep: [BTreeSet<(u32, u32)>; TIERS],
}

impl Pending {
    pub(super) fn is_empty(&self) -> bool {
        self.nonempty == [0; TIERS] && self.deep.iter().all(BTreeSet::is_empty)
    }

    fn insert(&mut self, tier: usize, index: u32, node: &mut ReactiveHot) {
        let depth = node.depth;
        if depth >= SHALLOW_DEPTHS {
            self.deep[tier].insert((depth, index));
            return;
        }
        let buckets = &mut self.buckets[tier];
        let bit = 1 << depth;
        let depth = depth as usize;
        if buckets.len() <= depth {
            buckets.resize_with(depth + 1, Vec::new);
        }
        let bucket = &mut buckets[depth];
        if bucket.last().is_some_and(|&last| last < index) {
            self.unordered[tier] |= bit;
        }
        node.queue_index = bucket.len() as u32;
        bucket.push(index);
        self.nonempty[tier] |= bit;
    }

    fn remove(&mut self, tier: usize, index: u32, nodes: &mut [ReactiveHot]) {
        let node = &nodes[index as usize];
        let depth = node.depth;
        if depth >= SHALLOW_DEPTHS {
            self.deep[tier].remove(&(depth, index));
            return;
        }
        let bit = 1 << depth;
        let bucket = &mut self.buckets[tier][depth as usize];
        let position = node.queue_index as usize;
        debug_assert_eq!(bucket[position], index);
        bucket.swap_remove(position);
        if let Some(&moved) = bucket.get(position) {
            nodes[moved as usize].queue_index = position as u32;
            if position + 1 < bucket.len() {
                self.unordered[tier] |= bit;
            }
        }
        if bucket.is_empty() {
            self.nonempty[tier] &= !bit;
            self.unordered[tier] &= !bit;
        }
    }

    fn pop(&mut self, tier: usize, nodes: &mut [ReactiveHot]) -> Option<u32> {
        let mask = self.nonempty[tier];
        if mask == 0 {
            return self.deep[tier].pop_first().map(|(_, index)| index);
        }
        let depth = mask.trailing_zeros();
        let bit = 1 << depth;
        let bucket = &mut self.buckets[tier][depth as usize];
        if self.unordered[tier] & bit != 0 {
            // Preserve the flush order of equal-depth nodes even when their sources
            // schedule them in a different order. Sort only when this bucket is read.
            bucket.sort_unstable_by(|a, b| b.cmp(a));
            for (position, &index) in bucket.iter().enumerate() {
                nodes[index as usize].queue_index = position as u32;
            }
            self.unordered[tier] &= !bit;
        }
        let index = bucket.pop().expect("nonempty reactive depth bucket");
        if bucket.is_empty() {
            self.nonempty[tier] &= !bit;
        }
        Some(index)
    }
}

impl ReactiveWorld {
    /// Removes the next observer in depth order, breaking ties by node slot.
    /// Its owner should evaluate or retire it.
    pub fn next_observer(&mut self, tier: FlushTier) -> Option<ReactiveId> {
        let index = self.pending.pop(tier as usize, &mut self.hot)?;
        let node = &mut self.hot[index as usize];
        debug_assert!(node.has_flag(SCHEDULED));
        node.flags &= !SCHEDULED;
        Some(node.id(self.runtime, index))
    }

    pub(crate) fn has_pending_observers(&self) -> bool {
        !self.pending.is_empty()
    }

    pub(crate) fn schedule(&mut self, id: ReactiveId) {
        let index = id.node();
        let node = &mut self.hot[index as usize];
        if !node.has_flag(SCHEDULED)
            && !node.computing()
            && let Some(tier) = node.kind.tier()
        {
            self.pending.insert(tier as usize, index, node);
            node.flags |= SCHEDULED;
        }
    }

    pub(crate) fn unschedule(&mut self, id: ReactiveId) {
        let index = id.node();
        let node = &mut self.hot[index as usize];
        if node.has_flag(SCHEDULED) {
            let tier = node.kind.tier().expect("scheduled reactive node has a flush tier");
            node.flags &= !SCHEDULED;
            self.pending.remove(tier as usize, index, &mut self.hot);
        }
    }
}

//! Hierarchical dirty sets keyed by slot index.

use crate::NodeId;
use rustc_hash::FxHashMap;
use slotmap::{Key, KeyData};

const FIRST_GENERATION: u32 = 1;
const WORD_BITS: usize = u64::BITS as usize;

/// A slot-indexed dirty set. Generations preserve identity across merged deltas
/// even when a transaction reuses a slot before the next projection.
#[derive(Clone, Debug, Default)]
pub struct DirtyBitmap {
    l0: Vec<u64>,
    l1: Vec<u64>,
    generations: FxHashMap<usize, u32>,
    reused: Vec<NodeId>,
}

impl DirtyBitmap {
    pub fn insert(&mut self, node: NodeId) -> bool {
        if node.is_null() {
            return false;
        }
        let (slot, generation) = parts(node);
        let word = slot / WORD_BITS;
        self.grow(word + 1);
        let bit = 1 << (slot % WORD_BITS);
        if self.l0[word] & bit != 0 {
            return self.insert_reused(node, slot, generation);
        }
        self.l0[word] |= bit;
        self.l1[word / WORD_BITS] |= 1 << (word % WORD_BITS);
        if generation != FIRST_GENERATION {
            self.generations.insert(slot, generation);
        }
        true
    }

    fn insert_reused(&mut self, node: NodeId, slot: usize, generation: u32) -> bool {
        if self.generation(slot) == generation || self.reused.contains(&node) {
            return false;
        }
        self.reused.push(node);
        true
    }

    fn grow(&mut self, words: usize) {
        if self.l0.len() < words {
            self.l0.resize(words, 0);
            self.l1.resize(words.div_ceil(WORD_BITS), 0);
        }
    }

    fn generation(&self, slot: usize) -> u32 {
        self.generations.get(&slot).copied().unwrap_or(FIRST_GENERATION)
    }

    fn has_slot(&self, slot: usize) -> bool {
        self.l0.get(slot / WORD_BITS).is_some_and(|word| word & (1 << (slot % WORD_BITS)) != 0)
    }

    pub fn contains(&self, node: &NodeId) -> bool {
        let (slot, generation) = parts(*node);
        self.has_slot(slot) && (self.generation(slot) == generation || self.reused.contains(node))
    }

    pub fn is_empty(&self) -> bool {
        self.l1.iter().all(|&word| word == 0)
    }

    pub fn clear(&mut self) {
        for (group, summary) in self.l1.iter_mut().enumerate() {
            for word in bits(std::mem::take(summary)) {
                self.l0[group * WORD_BITS + word] = 0;
            }
        }
        self.generations.clear();
        self.reused.clear();
    }

    pub fn is_disjoint(&self, other: &Self) -> bool {
        self.l1.iter().zip(&other.l1).enumerate().all(|(group, (left, right))| {
            bits(left & right).all(|offset| {
                let word = group * WORD_BITS + offset;
                let common = self.l0[word] & other.l0[word];
                if self.generations.is_empty() && other.generations.is_empty() {
                    return common == 0;
                }
                bits(common).all(|bit| {
                    let slot = word * WORD_BITS + bit;
                    self.generation(slot) != other.generation(slot)
                })
            })
        }) && !self.reused.iter().any(|node| other.contains(node))
            && !other.reused.iter().any(|node| self.contains(node))
    }

    pub fn union_with(&mut self, other: &Self) {
        if other.is_empty() {
            return;
        }
        // Fresh slots all have version 1. Only reused slots need a generation lookup.
        for &slot in self.generations.keys() {
            let first = key(slot, FIRST_GENERATION);
            if other.has_slot(slot) && !other.generations.contains_key(&slot) && !self.reused.contains(&first) {
                self.reused.push(first);
            }
        }
        for (&slot, &generation) in &other.generations {
            if self.has_slot(slot) {
                self.insert_reused(key(slot, generation), slot, generation);
            } else {
                self.generations.insert(slot, generation);
            }
        }
        self.grow(other.l0.len());
        for (group, &summary) in other.l1.iter().enumerate() {
            self.l1[group] |= summary;
            for offset in bits(summary) {
                let word = group * WORD_BITS + offset;
                self.l0[word] |= other.l0[word];
            }
        }
        self.extend(other.reused.iter().copied());
    }

    pub fn iter(&self) -> impl Iterator<Item = NodeId> + Clone + '_ {
        self.l1
            .iter()
            .enumerate()
            .flat_map(|(group, &summary)| bits(summary).map(move |offset| group * WORD_BITS + offset))
            .flat_map(|word| bits(self.l0[word]).map(move |bit| word * WORD_BITS + bit))
            .map(|slot| key(slot, self.generation(slot)))
            .chain(self.reused.iter().copied())
    }
}

impl Extend<NodeId> for DirtyBitmap {
    fn extend<T: IntoIterator<Item = NodeId>>(&mut self, nodes: T) {
        for node in nodes {
            self.insert(node);
        }
    }
}

impl FromIterator<NodeId> for DirtyBitmap {
    fn from_iter<T: IntoIterator<Item = NodeId>>(nodes: T) -> Self {
        let mut result = Self::default();
        result.extend(nodes);
        result
    }
}

fn bits(mut word: u64) -> impl Iterator<Item = usize> + Clone {
    std::iter::from_fn(move || {
        if word == 0 {
            return None;
        }
        let bit = word.trailing_zeros() as usize;
        word &= word - 1;
        Some(bit)
    })
}

// SlotMap 1.1 encodes the index in the low half and the generation in the high half.
fn parts(node: NodeId) -> (usize, u32) {
    let bits = node.data().as_ffi();
    (bits as u32 as usize, (bits >> 32) as u32)
}

fn key(slot: usize, generation: u32) -> NodeId {
    KeyData::from_ffi(((generation as u64) << 32) | slot as u64).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn union_spans_summary_words_and_preserves_reused_keys() {
        let old = key(6, 1);
        let new = key(6, 3);
        let distant = key(65 * 4096, 1);
        let mut left: DirtyBitmap = [old, key(4095, 1)].into_iter().collect();
        let right: DirtyBitmap = [new, distant].into_iter().collect();
        assert!(left.is_disjoint(&right));
        left.union_with(&right);
        left.union_with(&right);
        assert_eq!(left.iter().collect::<Vec<_>>(), [old, key(4095, 1), distant, new]);
        assert!(!left.is_disjoint(&right));
        left.clear();
        assert!(left.is_empty());
        assert!(left.iter().next().is_none());
        left.union_with(&right);
        assert_eq!(left.iter().collect::<Vec<_>>(), [new, distant]);
    }
}

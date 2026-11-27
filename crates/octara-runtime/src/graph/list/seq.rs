//! `Seq<K>`: an ordered sequence of unique keys with O(log n) position queries.
//!
//! An implicit treap (ordered by position, balanced by random priorities) with subtree sizes and
//! parent pointers, plus a key -> node index. Supports insert at a position, remove by key,
//! `index_of(key)`, `nth(i)` and `count_while` (rank of the first element failing a monotone
//! predicate), all expected O(log n). Each element also carries a weight with subtree sums,
//! so `prefix_weight(i)` and `index_at_weight(y)` are expected O(log n). The virtual list's
//! [SumTree] uses these sums for heights and tracks maximum widths by subtree.
//!
//! Ordered subsets compare fixed-size order tags in constant time. After an edit, repair visits
//! the changed treap paths and subtrees whose path labels moved. Under the random-priority model,
//! expected relabeling is logarithmic: the probability that an edited key is the highest-priority
//! key in an interval of length d is 1/d.
//!
//! Very deep subtrees use dense gap ordinals. Depth is bounded by the sum of the independent
//! left and right record counts. For m candidates, the record count R has E[2^R] = m + 1. By a union
//! bound the probability of any depth-128 node is at most n³ / 2^128. With u32 slots, even an
//! arena-wide relabel at that depth contributes at most one extra node of expected work.

use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use std::hash::Hash;
use std::sync::OnceLock;

const NIL: u32 = u32::MAX;
const ROOT_ORDER: u128 = 1 << (u128::BITS - 1);

/// A dyadic midpoint and an ordinal in the gap immediately following it.
/// Only subtrees below depth 127 need ordinals. The entire arena fits in u32,
/// so subset searches always compare two fixed-size keys.
#[derive(Clone, Copy, Default, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct OrderKey(u128, u32);

#[derive(Clone, Copy, Default)]
struct OrderSlot {
    midpoint: u128,
    ordinal: u32,
    dirty: bool,
}

struct Node<K> {
    key: K,
    prio: u64,
    l: u32,
    r: u32,
    p: u32,
    size: u32,
    weight: f64,
    sum: f64,
}

struct Width {
    row: f32,
    max: f32,
}

pub struct Seq<K> {
    nodes: Vec<Option<Node<K>>>,
    free: Vec<u32>,
    root: u32,
    index: FxHashMap<K, u32>,
    seed: u64,
    order: OnceLock<Vec<OrderSlot>>,
    widths: Option<Vec<Width>>,
}

impl<K: Eq + Hash> Default for Seq<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Eq + Hash> Seq<K> {
    pub fn new() -> Self {
        Seq {
            nodes: Vec::new(),
            free: Vec::new(),
            root: NIL,
            index: FxHashMap::default(),
            seed: 0x9E37_79B9_7F4A_7C15,
            order: OnceLock::new(),
            widths: None,
        }
    }

    pub fn from_keys(keys: impl IntoIterator<Item = K>) -> Self
    where
        K: Clone,
    {
        let mut s = Self::new();
        for k in keys {
            s.push(k);
        }
        s
    }

    fn n(&self, t: u32) -> &Node<K> {
        self.nodes[t as usize].as_ref().unwrap()
    }
    fn nm(&mut self, t: u32) -> &mut Node<K> {
        self.nodes[t as usize].as_mut().unwrap()
    }
    fn size(&self, t: u32) -> u32 {
        if t == NIL { 0 } else { self.n(t).size }
    }
    fn sum(&self, t: u32) -> f64 {
        if t == NIL { 0.0 } else { self.n(t).sum }
    }
    fn pull(&mut self, t: u32) {
        let (l, r) = (self.n(t).l, self.n(t).r);
        let size = 1 + self.size(l) + self.size(r);
        let sum = self.n(t).weight + self.sum(l) + self.sum(r);
        let node = self.nm(t);
        node.size = size;
        node.sum = sum;
        if let Some(widths) = &mut self.widths {
            let max = |child| if child == NIL { 0.0 } else { widths[child as usize].max };
            let width = &widths[t as usize];
            widths[t as usize].max = width.row.max(max(l)).max(max(r));
        }
    }
    fn pull_topology(&mut self, t: u32) {
        self.pull(t);
        let (l, r) = (self.n(t).l, self.n(t).r);
        if l != NIL {
            self.nm(l).p = t;
        }
        if r != NIL {
            self.nm(r).p = t;
        }
        if let Some(order) = self.order.get_mut() {
            if order.len() < self.nodes.len() {
                order.resize(self.nodes.len(), OrderSlot::default());
            }
            order[t as usize].dirty = true;
        }
    }

    fn rand(&mut self) -> u64 {
        // xorshift64*
        self.seed ^= self.seed >> 12;
        self.seed ^= self.seed << 25;
        self.seed ^= self.seed >> 27;
        self.seed.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn merge(&mut self, a: u32, b: u32) -> u32 {
        if a == NIL {
            return b;
        }
        if b == NIL {
            return a;
        }
        if self.n(a).prio > self.n(b).prio {
            let ar = self.n(a).r;
            let m = self.merge(ar, b);
            self.nm(a).r = m;
            self.pull_topology(a);
            a
        } else {
            let bl = self.n(b).l;
            let m = self.merge(a, bl);
            self.nm(b).l = m;
            self.pull_topology(b);
            b
        }
    }

    /// Splits `t` into its first `k` elements and the rest.
    fn split(&mut self, t: u32, k: u32) -> (u32, u32) {
        if t == NIL {
            return (NIL, NIL);
        }
        let l = self.n(t).l;
        let ls = self.size(l);
        if ls >= k {
            let (a, b) = self.split(l, k);
            self.nm(t).l = b;
            self.pull_topology(t);
            (a, t)
        } else {
            let r = self.n(t).r;
            let (a, b) = self.split(r, k - ls - 1);
            self.nm(t).r = a;
            self.pull_topology(t);
            (t, b)
        }
    }

    fn set_root(&mut self, t: u32) {
        self.root = t;
        if t != NIL {
            self.nm(t).p = NIL;
        }
        if let Some(order) = self.order.get_mut() {
            Self::repair_order(&self.nodes, t, order);
        }
    }

    fn repair_order(nodes: &[Option<Node<K>>], root: u32, order: &mut Vec<OrderSlot>) {
        order.resize(nodes.len(), OrderSlot::default());
        let mut pending = SmallVec::<[(u32, u128); 16]>::new();
        if root != NIL {
            pending.push((root, ROOT_ORDER));
        }
        while let Some((id, midpoint)) = pending.pop() {
            let entry = &mut order[id as usize];
            if entry.midpoint == midpoint && entry.ordinal == 0 && !entry.dirty {
                continue;
            }
            *entry = OrderSlot { midpoint, ordinal: 0, dirty: false };
            let node = nodes[id as usize].as_ref().expect("live treap node");
            let half = midpoint.isolate_lowest_one() >> 1;
            if half == 0 {
                // Reserve both adjacent gaps for these descendants.
                Self::renumber_gap(nodes, order, node.l, midpoint - 1);
                Self::renumber_gap(nodes, order, node.r, midpoint);
            } else {
                pending.extend([(node.l, midpoint - half), (node.r, midpoint + half)].into_iter().filter(|(child, _)| *child != NIL));
            }
        }
    }

    fn renumber_gap(nodes: &[Option<Node<K>>], order: &mut [OrderSlot], mut node: u32, midpoint: u128) {
        let mut parents = SmallVec::<[u32; 16]>::new();
        let mut ordinal = 0;
        loop {
            if node != NIL {
                parents.push(node);
                node = nodes[node as usize].as_ref().expect("live treap node").l;
            } else if let Some(parent) = parents.pop() {
                ordinal += 1;
                order[parent as usize] = OrderSlot { midpoint, ordinal, dirty: false };
                node = nodes[parent as usize].as_ref().expect("live treap node").r;
            } else {
                break;
            }
        }
    }

    /// Cached in-order key for searches over a subset of this sequence.
    pub(super) fn order_key(&self, key: &K) -> OrderKey {
        let slot = self.order_slots()[self.index[key] as usize];
        OrderKey(slot.midpoint, slot.ordinal)
    }

    /// Build the comparison cache during an ordered consumer's initial projection.
    pub(super) fn prepare_order(&self) {
        self.order_slots();
    }

    fn order_slots(&self) -> &[OrderSlot] {
        self.order.get_or_init(|| {
            let mut labels = Vec::new();
            Self::repair_order(&self.nodes, self.root, &mut labels);
            labels
        })
    }

    pub fn len(&self) -> usize {
        self.size(self.root) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.root == NIL
    }

    pub fn contains(&self, k: &K) -> bool {
        self.index.contains_key(k)
    }

    /// Inserts `k` (must be absent) at position `i` (clamped to the end), with weight 1.
    pub fn insert(&mut self, i: usize, k: K)
    where
        K: Clone,
    {
        self.insert_weighted(i, k, 1.0);
    }

    pub fn insert_weighted(&mut self, i: usize, k: K, weight: f64)
    where
        K: Clone,
    {
        self.insert_sized(i, k, weight, 0.0);
    }

    fn insert_sized(&mut self, i: usize, k: K, weight: f64, width: f32)
    where
        K: Clone,
    {
        assert!(!self.index.contains_key(&k), "Seq: duplicate key");
        let i = i.min(self.len()) as u32;
        let prio = self.rand();
        let node = Node { key: k.clone(), prio, l: NIL, r: NIL, p: NIL, size: 1, weight, sum: weight };
        let t = match self.free.pop() {
            Some(t) => {
                self.nodes[t as usize] = Some(node);
                t
            }
            None => {
                let slot = u32::try_from(self.nodes.len()).ok().filter(|&slot| slot != NIL).expect("Seq: node slots exhausted");
                self.nodes.push(Some(node));
                slot
            }
        };
        if let Some(widths) = &mut self.widths {
            let width = Width { row: width, max: width };
            if t as usize == widths.len() {
                widths.push(width);
            } else {
                widths[t as usize] = width;
            }
        }
        self.index.insert(k, t);
        let (a, b) = self.split(self.root, i);
        let ab = self.merge(a, t);
        let r = self.merge(ab, b);
        self.set_root(r);
    }

    /// Moves an existing key to its final index, preserving its weight.
    pub fn move_to(&mut self, key: &K, to: usize) -> bool {
        let Some(from) = self.index_of(key) else { return false };
        let to = to.min(self.len() - 1);
        if from == to {
            return false;
        }
        let (before, rest) = self.split(self.root, from as u32);
        let (row, after) = self.split(rest, 1);
        let priority = self.rand();
        self.nm(row).prio = priority;
        let remaining = self.merge(before, after);
        let (before, after) = self.split(remaining, to as u32);
        let prefix = self.merge(before, row);
        let root = self.merge(prefix, after);
        self.set_root(root);
        true
    }

    pub fn push(&mut self, k: K)
    where
        K: Clone,
    {
        let n = self.len();
        self.insert(n, k);
    }

    /// Removes `k`, returning its former position.
    pub fn remove(&mut self, k: &K) -> Option<usize> {
        let t = self.index.remove(k)?;
        let i = self.position(t);
        let (a, bc) = self.split(self.root, i as u32);
        let (_b, c) = self.split(bc, 1);
        let r = self.merge(a, c);
        self.set_root(r);
        self.nodes[t as usize] = None;
        self.free.push(t);
        Some(i)
    }

    fn position(&self, mut t: u32) -> usize {
        let mut i = self.size(self.n(t).l);
        loop {
            let p = self.n(t).p;
            if p == NIL {
                break;
            }
            if self.n(p).r == t {
                i += self.size(self.n(p).l) + 1;
            }
            t = p;
        }
        i as usize
    }

    /// Changes `k`'s weight, O(log n).
    pub fn set_weight(&mut self, k: &K, weight: f64) {
        self.set_size(k, weight, 0.0);
    }

    fn set_size(&mut self, k: &K, weight: f64, width: f32) {
        let Some(&t) = self.index.get(k) else { return };
        let mut changed = self.n(t).weight.to_bits() != weight.to_bits();
        if let Some(widths) = &mut self.widths {
            changed |= widths[t as usize].row.to_bits() != width.to_bits();
            widths[t as usize].row = width;
        }
        if !changed {
            return;
        }
        self.nm(t).weight = weight;
        let mut x = t;
        while x != NIL {
            self.pull(x);
            x = self.n(x).p;
        }
    }

    pub fn weight_of(&self, k: &K) -> Option<f64> {
        self.index.get(k).map(|t| self.n(*t).weight)
    }

    pub fn total_weight(&self) -> f64 {
        self.sum(self.root)
    }

    /// Sum of the weights of the first `i` elements.
    pub fn prefix_weight(&self, mut i: usize) -> f64 {
        let (mut t, mut acc) = (self.root, 0.0);
        while t != NIL && i > 0 {
            let ls = self.size(self.n(t).l) as usize;
            if i <= ls {
                t = self.n(t).l;
            } else {
                acc += self.sum(self.n(t).l) + self.n(t).weight;
                i -= ls + 1;
                t = self.n(t).r;
            }
        }
        acc
    }

    /// Index of the element covering offset `y` (the first whose cumulative end exceeds `y`);
    /// `len()` when `y` is past the end.
    pub fn index_at_weight(&self, mut y: f64) -> usize {
        let (mut t, mut idx) = (self.root, 0usize);
        while t != NIL {
            let l = self.n(t).l;
            if y < self.sum(l) {
                t = l;
                continue;
            }
            y -= self.sum(l);
            if y < self.n(t).weight {
                return idx + self.size(l) as usize;
            }
            y -= self.n(t).weight;
            idx += self.size(l) as usize + 1;
            t = self.n(t).r;
        }
        idx
    }

    pub fn index_of(&self, k: &K) -> Option<usize> {
        self.index.get(k).map(|t| self.position(*t))
    }

    pub fn nth(&self, mut i: usize) -> Option<&K> {
        let mut t = self.root;
        while t != NIL {
            let ls = self.size(self.n(t).l) as usize;
            if i < ls {
                t = self.n(t).l;
            } else if i == ls {
                return Some(&self.n(t).key);
            } else {
                i -= ls + 1;
                t = self.n(t).r;
            }
        }
        None
    }

    pub fn last(&self) -> Option<&K> {
        self.len().checked_sub(1).and_then(|i| self.nth(i))
    }

    /// Number of leading elements for which `pred` holds; `pred` must be true on a prefix.
    pub fn count_while(&self, mut pred: impl FnMut(&K) -> bool) -> usize {
        let (mut t, mut count) = (self.root, 0usize);
        while t != NIL {
            if pred(&self.n(t).key) {
                count += self.size(self.n(t).l) as usize + 1;
                t = self.n(t).r;
            } else {
                t = self.n(t).l;
            }
        }
        count
    }

    pub fn to_vec(&self) -> Vec<K>
    where
        K: Clone,
    {
        let mut out = Vec::with_capacity(self.len());
        let mut stack = Vec::new();
        let mut t = self.root;
        while t != NIL || !stack.is_empty() {
            while t != NIL {
                stack.push(t);
                t = self.n(t).l;
            }
            let x = stack.pop().unwrap();
            out.push(self.n(x).key.clone());
            t = self.n(x).r;
        }
        out
    }
}

/// A row's finite, nonnegative dimensions.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RowSize {
    pub height: f64,
    pub width: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RowSummary {
    pub count: usize,
    pub total_height: f64,
    pub max_width: f32,
}

/// Retains virtual row measurements in list order. Insert, remove, resize, rank,
/// prefix height, and offset lookup take expected O(log n).
pub struct SumTree<K>(Seq<K>);

impl<K: Eq + Hash> Default for SumTree<K> {
    fn default() -> Self {
        Self(Seq { widths: Some(Vec::new()), ..Seq::new() })
    }
}

impl<K: Eq + Hash> SumTree<K> {
    pub fn insert(&mut self, rank: usize, key: K, size: RowSize)
    where
        K: Clone,
    {
        self.0.insert_sized(rank, key, size.height, size.width);
    }

    pub fn remove(&mut self, key: &K) {
        self.0.remove(key);
    }

    pub fn set_size(&mut self, key: &K, size: RowSize) {
        self.0.set_size(key, size.height, size.width);
    }

    pub fn size_of(&self, key: &K) -> Option<RowSize> {
        let slot = *self.0.index.get(key)?;
        Some(RowSize { height: self.0.n(slot).weight, width: self.0.widths.as_ref().expect("row widths")[slot as usize].row })
    }

    pub fn summary(&self) -> RowSummary {
        let root = self.0.root as usize;
        RowSummary {
            count: self.0.len(),
            total_height: self.0.total_weight(),
            max_width: self.0.widths.as_ref().expect("row widths").get(root).map_or(0.0, |width| width.max),
        }
    }

    pub fn rank(&self, key: &K) -> Option<usize> {
        self.0.index_of(key)
    }

    pub fn key_at(&self, rank: usize) -> Option<&K> {
        self.0.nth(rank)
    }

    pub fn offset(&self, rank: usize) -> f64 {
        self.0.prefix_weight(rank)
    }

    pub fn rank_at(&self, offset: f64) -> usize {
        self.0.index_at_weight(offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sum_tree_keeps_dimensions_after_resizes_and_topology_edits() {
        let mut tree = SumTree::default();
        for (key, (height, width)) in [(10.0, 60.0), (30.0, 20.0), (20.0, 40.0)].into_iter().enumerate() {
            tree.insert(key, key, RowSize { height, width });
        }
        assert_eq!(tree.summary(), RowSummary { count: 3, total_height: 60.0, max_width: 60.0 });
        assert_eq!(tree.rank_at(9.0), 0);
        assert_eq!(tree.rank_at(10.0), 1);
        assert_eq!(tree.rank_at(40.0), 2);
        assert_eq!(tree.rank_at(60.0), 3);

        tree.set_size(&0, RowSize { height: 5.0, width: 10.0 });
        assert_eq!(tree.summary(), RowSummary { count: 3, total_height: 55.0, max_width: 40.0 });
        assert_eq!(tree.offset(2), 35.0);
        assert_eq!(tree.rank_at(35.0), 2);
        tree.set_size(&1, RowSize { height: 30.0, width: 95.0 });
        assert_eq!(tree.summary().max_width, 95.0);

        let last = tree.size_of(&2).unwrap();
        tree.remove(&2);
        tree.insert(0, 2, last);
        tree.remove(&1);
        assert_eq!(tree.summary(), RowSummary { count: 2, total_height: 25.0, max_width: 40.0 });
        assert_eq!(tree.rank(&0), Some(1));
        assert_eq!(tree.key_at(0), Some(&2));
        assert_eq!(tree.offset(1), 20.0);
        assert_eq!(tree.rank_at(20.0), 1);
        tree.remove(&0);
        tree.remove(&2);
        assert_eq!(tree.summary(), RowSummary::default());
        assert_eq!(tree.rank_at(0.0), 0);
    }

    #[test]
    #[ignore = "microbenchmark, run in release mode with --ignored --nocapture"]
    fn sum_tree_100k_row_update_and_offset() {
        use std::hint::black_box;
        use std::time::{Duration, Instant};

        const ROWS: usize = 100_000;
        const SAMPLES: usize = 10_000;
        let initial = |rank: usize| RowSize { height: 12.0 + (rank % 80) as f64, width: 20.0 + (rank % 420) as f32 };
        let mut tree = SumTree::default();
        let mut seq = Seq::new();
        let mut linear = Vec::with_capacity(ROWS);
        for rank in 0..ROWS {
            let size = initial(rank);
            tree.insert(rank, rank, size);
            seq.insert_weighted(rank, rank, size.height);
            linear.push(size);
        }
        let operations: Vec<_> = (0..SAMPLES)
            .map(|sample| {
                let rank = sample * 79_919 % ROWS;
                let size = initial(rank + 1);
                (rank, size, sample * 37_777 % (ROWS + 1))
            })
            .collect();
        fn time(operations: &[(usize, RowSize, usize)], mut operation: impl FnMut(usize, RowSize, usize) -> f64) -> Duration {
            let start = Instant::now();
            for &(key, size, rank) in black_box(operations) {
                black_box(operation(key, size, rank));
            }
            start.elapsed() / operations.len() as u32
        }
        let sum_tree = time(&operations, |key, size, rank| {
            tree.set_size(&key, size);
            tree.offset(rank)
        });
        let height_only = time(&operations, |key, size, rank| {
            seq.set_weight(&key, size.height);
            seq.prefix_weight(rank)
        });
        let scan = time(&operations, |key, size, rank| {
            linear[key] = size;
            linear[..rank].iter().map(|row| row.height).sum()
        });
        eprintln!("100k rows, per update and offset: SumTree {sum_tree:?}, height-only Seq {height_only:?}, linear scan {scan:?}");
        assert_eq!(tree.offset(ROWS), seq.total_weight());
        assert_eq!(tree.offset(ROWS), linear.iter().map(|row| row.height).sum::<f64>());
        assert!(sum_tree < Duration::from_micros(5), "run with --release and verify the 5 µs target: {sum_tree:?}");
    }

    #[test]
    fn deep_order_keys_use_fixed_size_gap_ordinals() {
        // A deliberately skewed tree exceeds the midpoint precision.
        let mut seq = Seq::new();
        for key in (0..512).rev() {
            seq.push(key);
            seq.nm(seq.root).prio = 0;
            if seq.n(seq.root).l != NIL {
                // Subsequent insertions have higher priority than the current root.
                assert_eq!(seq.n(seq.root).key, key);
            }
        }
        let keys = seq.to_vec();
        assert!(seq.order_key(&511).1 > 0);
        for pair in keys.windows(2) {
            assert!(seq.order_key(&pair[0]) < seq.order_key(&pair[1]));
        }

        // An incremental update inside a gap retains exact order too.
        seq.move_to(&511, 128);
        assert_eq!(seq.index_of(&511), Some(128));
        for pair in seq.to_vec().windows(2) {
            assert!(seq.order_key(&pair[0]) < seq.order_key(&pair[1]));
        }

        // The same encoding handles a gap on the right.
        let mut seq = Seq::new();
        for key in 0..512u32 {
            seq.insert(0, key);
            seq.nm(seq.root).prio = 0;
        }
        for node in seq.nodes.iter_mut().flatten() {
            node.prio = u64::from(node.key) + 1;
        }
        assert!(seq.order_key(&0).1 > 0);
        let OrderKey(midpoint, ordinal) = seq.order_key(&46);
        seq.remove(&47);
        assert!(seq.order_key(&46) == OrderKey(midpoint, ordinal - 1));
        for pair in seq.to_vec().windows(2) {
            assert!(seq.order_key(&pair[0]) < seq.order_key(&pair[1]));
        }
    }

    /// Differential test against a `Vec` reference on random operations.
    #[test]
    fn weights_match_reference() {
        let mut seq = Seq::new();
        let mut reference: Vec<(u32, f64)> = Vec::new();
        let mut x: u64 = 7;
        let mut rnd = |m: usize| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x % m.max(1) as u64) as usize
        };
        for k in 0..3_000u32 {
            let i = rnd(reference.len() + 1);
            let w = 1.0 + rnd(40) as f64;
            seq.insert_weighted(i, k, w);
            reference.insert(i, (k, w));
            if k % 3 == 0 {
                let j = rnd(reference.len());
                let w = 1.0 + rnd(40) as f64;
                seq.set_weight(&reference[j].0, w);
                reference[j].1 = w;
            }
            if k % 50 == 0 {
                let i = rnd(reference.len() + 1);
                let want: f64 = reference[..i].iter().map(|e| e.1).sum();
                assert_eq!(seq.prefix_weight(i), want);
                let y = rnd(want as usize + 1) as f64;
                let mut acc = 0.0;
                let expect = reference.iter().position(|e| {
                    acc += e.1;
                    acc > y
                });
                assert_eq!(seq.index_at_weight(y), expect.unwrap_or(reference.len()));
            }
        }
        assert_eq!(seq.total_weight(), reference.iter().map(|e| e.1).sum::<f64>());
    }

    #[test]
    fn matches_vec_reference() {
        let mut seq = Seq::new();
        let mut reference: Vec<u32> = Vec::new();
        let mut x: u64 = 42;
        let mut next = 0u32;
        let mut rnd = |m: usize| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x % m.max(1) as u64) as usize
        };
        for step in 0..20_000 {
            match rnd(5) {
                0 | 1 => {
                    let i = rnd(reference.len() + 1);
                    seq.insert(i, next);
                    reference.insert(i, next);
                    next += 1;
                }
                2 if !reference.is_empty() => {
                    let k = reference[rnd(reference.len())];
                    let i = seq.remove(&k).unwrap();
                    assert_eq!(reference.remove(i), k);
                }
                3 if !reference.is_empty() => {
                    let old = rnd(reference.len());
                    let to = rnd(reference.len());
                    let key = reference.remove(old);
                    reference.insert(to, key);
                    assert_eq!(seq.move_to(&key, to), old != to);
                }
                _ if !reference.is_empty() => {
                    let i = rnd(reference.len());
                    let j = rnd(reference.len());
                    assert_eq!(seq.order_key(&reference[i]) < seq.order_key(&reference[j]), i < j);
                    assert_eq!(seq.nth(i), Some(&reference[i]));
                    assert_eq!(seq.index_of(&reference[i]), Some(i));
                    let pivot = reference[i];
                    let pos = reference.iter().position(|k| *k == pivot).unwrap();
                    assert_eq!(seq.count_while(|k| reference[..pos].contains(k)), pos);
                }
                _ => {}
            }
            assert_eq!(seq.len(), reference.len());
            if step % 997 == 0 {
                assert_eq!(seq.to_vec(), reference);
                for pair in reference.windows(2) {
                    assert!(seq.order_key(&pair[0]) < seq.order_key(&pair[1]));
                }
            }
        }
        assert_eq!(seq.to_vec(), reference);
    }
}

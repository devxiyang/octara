//! `Seq<K>`: an ordered sequence of unique keys with O(log n) position queries.
//!
//! An implicit treap (ordered by position, balanced by random priorities) with subtree sizes and
//! parent pointers, plus a key -> node index. Supports insert at a position, remove by key,
//! `index_of(key)`, `nth(i)` and `count_while` (rank of the first element failing a monotone
//! predicate), all expected O(log n). Each element also carries a weight (a row height for
//! virtualization) with subtree sums, so `prefix_weight(i)` and `index_at_weight(y)` are O(log n):
//! the "sum tree" GPUI and Rune use for long lists. The reference implementation is a `Vec`; the
//! tests compare the two on random operations.

use std::collections::HashMap;
use std::hash::Hash;

const NIL: u32 = u32::MAX;

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

pub struct Seq<K> {
    nodes: Vec<Option<Node<K>>>,
    free: Vec<u32>,
    root: u32,
    index: HashMap<K, u32>,
    seed: u64,
}

impl<K: Eq + Hash + Clone> Default for Seq<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Eq + Hash + Clone> Seq<K> {
    pub fn new() -> Self {
        Seq { nodes: Vec::new(), free: Vec::new(), root: NIL, index: HashMap::new(), seed: 0x9E37_79B9_7F4A_7C15 }
    }

    pub fn from_keys(keys: impl IntoIterator<Item = K>) -> Self {
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
        self.nm(t).size = size;
        self.nm(t).sum = sum;
        if l != NIL {
            self.nm(l).p = t;
        }
        if r != NIL {
            self.nm(r).p = t;
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
            self.pull(a);
            a
        } else {
            let bl = self.n(b).l;
            let m = self.merge(a, bl);
            self.nm(b).l = m;
            self.pull(b);
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
            self.pull(t);
            (a, t)
        } else {
            let r = self.n(t).r;
            let (a, b) = self.split(r, k - ls - 1);
            self.nm(t).r = a;
            self.pull(t);
            (t, b)
        }
    }

    fn set_root(&mut self, t: u32) {
        self.root = t;
        if t != NIL {
            self.nm(t).p = NIL;
        }
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
    pub fn insert(&mut self, i: usize, k: K) {
        self.insert_weighted(i, k, 1.0);
    }

    pub fn insert_weighted(&mut self, i: usize, k: K, weight: f64) {
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
                self.nodes.push(Some(node));
                self.nodes.len() as u32 - 1
            }
        };
        self.index.insert(k, t);
        let (a, b) = self.split(self.root, i);
        let ab = self.merge(a, t);
        let r = self.merge(ab, b);
        self.set_root(r);
    }

    pub fn push(&mut self, k: K) {
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
        let Some(&t) = self.index.get(k) else { return };
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

    pub fn to_vec(&self) -> Vec<K> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
            match rnd(4) {
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
                _ if !reference.is_empty() => {
                    let i = rnd(reference.len());
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
            }
        }
        assert_eq!(seq.to_vec(), reference);
    }
}

//! Incremental collections (design §3; DBSP / ILC): a `List` and composable views over it.
//!
//! Every collection is a *stage*: an ordered set of keys with an item per key, a bounded log of
//! structural changes (`Insert` / `Remove` / `Move`, each with the index at that moment) and
//! observers that receive value-level events (`inserted`, `removed`, `updated(old, new)`,
//! `moved`). A view is an observer of its upstream stage and a stage itself, so views compose:
//! `list.filter(..).sort_by(..)`, `list.group_by(..)` with per-group members.
//!
//! Each stage keeps its order in a `Seq` (order-statistic treap), so every event costs
//! O(log n) (O(log² n) where a rank among members is needed). The UI layer renders a stage by
//! applying its log (`Source::subscribe` / `Source::take`); a `Move` moves a row.

use crate::diagnostics as codes;
use crate::reactive::graph::{Ctx, ReadSignal, Signal, rt, state};
use crate::reactive::seq::Seq;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::fmt::Display;
use std::hash::Hash;
use std::panic::Location;
use std::rc::{Rc, Weak};

// ---------------------------------------------------------------------------------------------
// Changes, logs, observers
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Change<K> {
    Insert {
        key: K,
        index: usize,
    },
    Remove {
        key: K,
        index: usize,
    },
    /// `to` is the index after removing the key from `from`.
    Move {
        key: K,
        from: usize,
        to: usize,
    },
}

/// A reader's position in a change log.
pub(crate) type Cursor = Rc<Cell<u64>>;

/// The item signals of a flattened grouping, by flat key.
type FlatSignals<G, K, T> = HashMap<FlatKey<G, K>, Signal<FlatItem<G, T>>>;

/// A change log trimmed to its slowest live reader. Shared by collections (`Change<K>`) and
/// text (`TextChange`).
pub(crate) struct Log<E> {
    first: u64,
    entries: VecDeque<E>,
    cursors: Vec<Weak<Cell<u64>>>,
}

impl<E: Clone> Log<E> {
    pub(crate) fn new() -> Self {
        Log { first: 0, entries: VecDeque::new(), cursors: Vec::new() }
    }
    fn end(&self) -> u64 {
        self.first + self.entries.len() as u64
    }
    pub(crate) fn subscribe(&mut self) -> Cursor {
        let c = Rc::new(Cell::new(self.end()));
        self.cursors.push(Rc::downgrade(&c));
        c
    }
    pub(crate) fn push(&mut self, c: E) {
        self.entries.push_back(c);
        self.trim();
    }
    pub(crate) fn take(&mut self, cursor: &Cursor) -> Vec<E> {
        let from = (cursor.get().max(self.first) - self.first) as usize;
        let out = self.entries.iter().skip(from).cloned().collect();
        cursor.set(self.end());
        self.trim();
        out
    }
    fn trim(&mut self) {
        self.cursors.retain(|c| c.strong_count() > 0);
        let end = self.end();
        let min = self.cursors.iter().filter_map(|c| c.upgrade()).map(|c| c.get()).min().unwrap_or(end);
        while self.first < min && !self.entries.is_empty() {
            self.entries.pop_front();
            self.first += 1;
        }
        if self.entries.is_empty() {
            self.first = end;
        }
    }
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Value-level events from an upstream stage. Indices are positions in that stage.
pub(crate) trait Observer<K, T> {
    fn inserted(&self, index: usize, key: &K, item: &T);
    /// `index` is where the key was in the upstream stage.
    fn removed(&self, index: usize, key: &K, item: &T);
    fn updated(&self, key: &K, old: &T, new: &T);
    /// `to` is the index after removing the key from `from`.
    fn moved(&self, key: &K, from: usize, to: usize, item: &T);
}

/// The output side of a stage: its log, its version signal and its observers.
pub(crate) struct Emitter<K, T> {
    log: RefCell<Log<Change<K>>>,
    version: Signal<u64>,
    observers: RefCell<Vec<Weak<dyn Observer<K, T>>>>,
}

impl<K: Clone + 'static, T: 'static> Emitter<K, T> {
    fn new() -> Self {
        Emitter { log: RefCell::new(Log::new()), version: Signal::detached(0), observers: RefCell::new(Vec::new()) }
    }
    fn bump(&self) {
        self.version.set_internal(self.version.get_untracked() + 1);
    }
    fn live(&self) -> Vec<Rc<dyn Observer<K, T>>> {
        let mut obs = self.observers.borrow_mut();
        obs.retain(|o| o.strong_count() > 0);
        obs.iter().filter_map(|o| o.upgrade()).collect()
    }
    fn insert(&self, index: usize, k: &K, t: &T) {
        self.log.borrow_mut().push(Change::Insert { key: k.clone(), index });
        self.bump();
        self.live().iter().for_each(|o| o.inserted(index, k, t));
    }
    fn remove(&self, index: usize, k: &K, t: &T) {
        self.log.borrow_mut().push(Change::Remove { key: k.clone(), index });
        self.bump();
        self.live().iter().for_each(|o| o.removed(index, k, t));
    }
    fn update(&self, k: &K, old: &T, new: &T) {
        self.live().iter().for_each(|o| o.updated(k, old, new));
    }
    fn moved(&self, from: usize, to: usize, k: &K, t: &T) {
        self.log.borrow_mut().push(Change::Move { key: k.clone(), from, to });
        self.bump();
        self.live().iter().for_each(|o| o.moved(k, from, to, t));
    }
}

impl<K, T> Drop for Emitter<K, T> {
    fn drop(&mut self) {
        self.version.dispose();
    }
}

/// An ordered keyed collection: a list or a view.
pub(crate) trait Stage<K, T> {
    fn index_of(&self, k: &K) -> Option<usize>;
    fn len(&self) -> usize;
    fn snapshot(&self) -> Vec<K>;
    fn item(&self, k: &K) -> Option<ReadSignal<T>>;
    fn emitter(&self) -> &Emitter<K, T>;
}

/// A view's closure, run as one: untracked (even when the view is built inside a binding), a
/// signal read reported (`VIEW_READS_STATE`), a write rejected.
fn view_fn<A: ?Sized + 'static, R: 'static>(f: impl Fn(&A) -> R + 'static) -> impl Fn(&A) -> R + 'static {
    move |a| rt(|rt| rt.with_frame(Ctx::View, None, None, || f(a)))
}

/// Writes to collections and text follow the layer rules of signals.
pub(crate) fn writable(at: &'static Location<'static>) -> bool {
    rt(|rt| {
        let code = match rt.ctx() {
            Ctx::Build => &codes::WRITE_IN_BUILD,
            Ctx::Derive | Ctx::Observe | Ctx::View => &codes::WRITE_IN_DERIVE,
            _ => return true,
        };
        rt.diag(code, "collection write outside a handler or `watch` callback; rejected".into(), at);
        false
    })
}

// ---------------------------------------------------------------------------------------------
// Source: a handle to any stage
// ---------------------------------------------------------------------------------------------

/// A handle to a stage, for rendering and for building further views.
pub struct Source<K: 'static, T: 'static>(Rc<dyn Stage<K, T>>);

impl<K, T> Clone for Source<K, T> {
    fn clone(&self) -> Self {
        Source(self.0.clone())
    }
}

impl<K, T> PartialEq for Source<K, T> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::addr_eq(Rc::as_ptr(&self.0), Rc::as_ptr(&other.0))
    }
}

impl<K, T> Source<K, T>
where
    K: Eq + Hash + Clone + 'static,
    T: Clone + 'static,
{
    /// Subscribes the running observer to this stage's structure.
    pub(crate) fn track(&self) {
        self.0.emitter().version.get();
    }

    /// Keys in order, untracked.
    pub(crate) fn snapshot(&self) -> Vec<K> {
        self.0.snapshot()
    }

    /// A reader of this stage's change log, positioned at its end.
    pub(crate) fn subscribe(&self) -> Cursor {
        self.0.emitter().log.borrow_mut().subscribe()
    }

    /// The changes since `cursor`, advancing it.
    pub(crate) fn take(&self, cursor: &Cursor) -> Vec<Change<K>> {
        self.0.emitter().log.borrow_mut().take(cursor)
    }

    /// Number of keys; tracks the structure.
    #[track_caller]
    pub fn len(&self) -> usize {
        self.track();
        self.0.len()
    }

    #[track_caller]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Keys in order (O(n)); tracks the structure.
    #[track_caller]
    pub fn keys(&self) -> Vec<K> {
        self.track();
        self.0.snapshot()
    }

    pub fn item(&self, k: &K) -> Option<ReadSignal<T>> {
        self.0.item(k)
    }

    /// Entries held in this stage's log (to check it stays bounded).
    pub fn log_len(&self) -> usize {
        self.0.emitter().log.borrow().len()
    }

    fn attach<V: Stage<K, T> + Observer<K, T> + 'static>(&self, v: Rc<V>) -> Derived<K, T> {
        let obs: Rc<dyn Observer<K, T>> = v.clone();
        self.0.emitter().observers.borrow_mut().push(Rc::downgrade(&obs));
        let version = v.emitter().version;
        // Scope-owned: the view lives as long as the scope that created it.
        Derived { holder: Signal::scoped(v as Rc<dyn Stage<K, T>>), version }
    }

    /// The items matching `pred`, in upstream order; maintained incrementally.
    pub fn filter(&self, pred: impl Fn(&T) -> bool + 'static) -> Derived<K, T> {
        let pred = view_fn(pred);
        let up = self.0.clone();
        let members = Seq::from_keys(up.snapshot().into_iter().filter(|k| up.item(k).is_some_and(|i| i.with_untracked(|t| pred(t)))));
        self.attach(Rc::new(FilterInner { up, pred: Box::new(pred), seq: RefCell::new(members), emitter: Emitter::new() }))
    }

    /// The items transformed by `f`, under the same keys and in the same order; maintained
    /// incrementally: an item's change calls `f` once, and reaches what follows only if the
    /// result differs. `f` is a pure function of the item (a read signal is reported).
    pub fn map<U: Clone + PartialEq + 'static>(&self, f: impl Fn(&T) -> U + 'static) -> Derived<K, U> {
        let f = view_fn(f);
        let up = self.0.clone();
        let out = up
            .snapshot()
            .into_iter()
            .filter_map(|k| up.item(&k).map(|it| (k, Signal::detached(it.with_untracked(|t| f(t))))))
            .collect();
        let m = Rc::new(MapInner { up: up.clone(), f: Box::new(f), out: RefCell::new(out), emitter: Emitter::new() });
        let obs: Rc<dyn Observer<K, T>> = m.clone();
        up.emitter().observers.borrow_mut().push(Rc::downgrade(&obs));
        let version = m.emitter.version;
        Derived { holder: Signal::scoped(m as Rc<dyn Stage<K, U>>), version }
    }

    /// The items ordered by `key` (ties keep the order in which they entered the view);
    /// maintained incrementally: a key change moves one item.
    pub fn sort_by<S: Ord + Clone + 'static>(&self, key: impl Fn(&T) -> S + 'static) -> Derived<K, T> {
        let key = view_fn(key);
        let up = self.0.clone();
        let mut entries: Vec<(K, (S, u64))> = up
            .snapshot()
            .into_iter()
            .enumerate()
            .filter_map(|(i, k)| up.item(&k).map(|it| (k, (it.with_untracked(|t| key(t)), i as u64))))
            .collect();
        entries.sort_by(|a, b| a.1.cmp(&b.1));
        let next = entries.len() as u64;
        let seq = Seq::from_keys(entries.iter().map(|e| e.0.clone()));
        let ord = entries.into_iter().collect();
        self.attach(Rc::new(SortedInner {
            up,
            key: Box::new(key),
            seq: RefCell::new(seq),
            ord: RefCell::new(ord),
            next: Cell::new(next),
            emitter: Emitter::new(),
        }))
    }

    /// Groups the items by `key`. Groups are ordered by key; each group's members keep upstream
    /// order. Group counts and membership are maintained incrementally.
    pub fn group_by<G>(&self, key: impl Fn(&T) -> G + 'static) -> Groups<G, K, T>
    where
        G: Ord + Hash + Clone + Display + 'static,
    {
        let key = view_fn(key);
        let up = self.0.clone();
        let g = Rc::new(GroupsInner {
            up: up.clone(),
            key: Box::new(key),
            groups: RefCell::new(Seq::new()),
            members: RefCell::new(HashMap::new()),
            member_of: RefCell::new(HashMap::new()),
            emitter: Emitter::new(),
        });
        for (i, k) in up.snapshot().into_iter().enumerate() {
            if let Some(item) = up.item(&k) {
                item.with_untracked(|t| g.inserted(i, &k, t));
            }
        }
        // Building emitted into empty logs with no readers; nothing to keep.
        let obs: Rc<dyn Observer<K, T>> = g.clone();
        up.emitter().observers.borrow_mut().push(Rc::downgrade(&obs));
        let version = g.emitter.version;
        Groups { holder: Signal::scoped(g), version }
    }
}

// ---------------------------------------------------------------------------------------------
// List
// ---------------------------------------------------------------------------------------------

struct ListInner<K, T> {
    seq: RefCell<Seq<K>>,
    items: RefCell<HashMap<K, Signal<T>>>,
    emitter: Emitter<K, T>,
    key_fn: fn(&T) -> K,
}

impl<K, T> Drop for ListInner<K, T> {
    fn drop(&mut self) {
        for s in self.items.get_mut().values() {
            s.dispose();
        }
    }
}

impl<K, T> Stage<K, T> for ListInner<K, T>
where
    K: Eq + Hash + Clone + 'static,
    T: Clone + 'static,
{
    fn index_of(&self, k: &K) -> Option<usize> {
        self.seq.borrow().index_of(k)
    }
    fn len(&self) -> usize {
        self.seq.borrow().len()
    }
    fn snapshot(&self) -> Vec<K> {
        self.seq.borrow().to_vec()
    }
    fn item(&self, k: &K) -> Option<ReadSignal<T>> {
        self.items.borrow().get(k).map(|s| s.read_only())
    }
    fn emitter(&self) -> &Emitter<K, T> {
        &self.emitter
    }
}

/// A keyed list as named component state (like [`state`]): survives hot reload; its items are
/// disposed when it is.
pub struct List<K, T> {
    holder: Signal<Rc<ListInner<K, T>>>,
}

impl<K, T> Clone for List<K, T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<K, T> Copy for List<K, T> {}

#[track_caller]
pub fn list<K, T>(key: &'static str, init: impl FnOnce() -> Vec<T>, key_fn: fn(&T) -> K) -> List<K, T>
where
    K: Eq + Hash + Clone + 'static,
    T: Clone + 'static,
{
    let at = Location::caller();
    let holder = state(key, || {
        let mut seq = Seq::new();
        let mut items = HashMap::new();
        for t in init() {
            let k = key_fn(&t);
            if items.contains_key(&k) {
                rt(|rt| rt.diag(&codes::DUPLICATE_KEY, "initial list items share a key; later duplicates dropped".into(), at));
                continue;
            }
            seq.push(k.clone());
            items.insert(k, Signal::detached(t));
        }
        Rc::new(ListInner { seq: RefCell::new(seq), items: RefCell::new(items), emitter: Emitter::new(), key_fn })
    });
    List { holder }
}

impl<K, T> List<K, T>
where
    K: Eq + Hash + Clone + 'static,
    T: Clone + 'static,
{
    fn inner(&self) -> Rc<ListInner<K, T>> {
        self.holder.get_untracked()
    }

    pub fn source(&self) -> Source<K, T> {
        Source(self.inner())
    }
    /// Alias of [`List::source`].
    pub fn all(&self) -> Source<K, T> {
        self.source()
    }
    #[track_caller]
    pub fn len(&self) -> usize {
        self.source().len()
    }
    #[track_caller]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    #[track_caller]
    pub fn keys(&self) -> Vec<K> {
        self.source().keys()
    }
    #[track_caller]
    pub fn last_key(&self) -> Option<K> {
        let inner = self.inner();
        inner.emitter.version.get();
        inner.seq.borrow().last().cloned()
    }
    pub fn item(&self, k: &K) -> Option<ReadSignal<T>> {
        self.inner().item(k)
    }
    pub fn index_of(&self, k: &K) -> Option<usize> {
        self.inner().index_of(k)
    }
    pub fn filter(&self, pred: impl Fn(&T) -> bool + 'static) -> Derived<K, T> {
        self.source().filter(pred)
    }
    /// The items transformed by `f`, same keys and order ([`Source::map`]).
    pub fn map<U: Clone + PartialEq + 'static>(&self, f: impl Fn(&T) -> U + 'static) -> Derived<K, U> {
        self.source().map(f)
    }
    pub fn sort_by<S: Ord + Clone + 'static>(&self, key: impl Fn(&T) -> S + 'static) -> Derived<K, T> {
        self.source().sort_by(key)
    }
    pub fn group_by<G: Ord + Hash + Clone + Display + 'static>(&self, key: impl Fn(&T) -> G + 'static) -> Groups<G, K, T> {
        self.source().group_by(key)
    }

    #[track_caller]
    pub fn push(&self, t: T) {
        let n = self.inner().len();
        self.insert_at(Location::caller(), n, t);
    }

    #[track_caller]
    pub fn insert(&self, index: usize, t: T) {
        self.insert_at(Location::caller(), index, t);
    }

    fn insert_at(&self, at: &'static Location<'static>, index: usize, t: T) {
        if !writable(at) {
            return;
        }
        let inner = self.inner();
        let k = (inner.key_fn)(&t);
        if inner.items.borrow().contains_key(&k) {
            rt(|rt| rt.diag(&codes::DUPLICATE_KEY, "list insert with a key already present; ignored".into(), at));
            return;
        }
        let index = index.min(inner.len());
        inner.items.borrow_mut().insert(k.clone(), Signal::detached(t.clone()));
        inner.seq.borrow_mut().insert(index, k.clone());
        inner.emitter.insert(index, &k, &t);
    }

    /// Changes one item: its readers and the observers are notified, nothing else.
    #[track_caller]
    pub fn update(&self, k: &K, f: impl FnOnce(&mut T)) {
        if !writable(Location::caller()) {
            return;
        }
        let inner = self.inner();
        let Some(s) = inner.items.borrow().get(k).copied() else { return };
        let old = s.get_untracked();
        s.update(f);
        let new = s.get_untracked();
        inner.emitter.update(k, &old, &new);
    }

    #[track_caller]
    pub fn remove(&self, k: &K) {
        if writable(Location::caller()) {
            self.remove_key(k);
        }
    }

    fn remove_key(&self, k: &K) {
        let inner = self.inner();
        let Some(s) = inner.items.borrow_mut().remove(k) else { return };
        let index = inner.seq.borrow_mut().remove(k).expect("key in seq");
        let old = s.get_untracked();
        inner.emitter.remove(index, k, &old);
        s.dispose();
    }

    /// Keeps the items matching `pred` (one O(n) scan, then O(log n) per removal).
    #[track_caller]
    pub fn retain(&self, pred: impl Fn(&T) -> bool) {
        if !writable(Location::caller()) {
            return;
        }
        let doomed: Vec<K> = {
            let inner = self.inner();
            let items = inner.items.borrow();
            inner.seq.borrow().to_vec().into_iter().filter(|k| !items[k].with_untracked(|t| pred(t))).collect()
        };
        for k in doomed {
            self.remove_key(&k);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Derived views
// ---------------------------------------------------------------------------------------------

/// A handle to a derived view (`filter`, `sort_by`). `Copy`; owned by the scope that made it.
pub struct Derived<K: 'static, T: 'static> {
    holder: Signal<Rc<dyn Stage<K, T>>>,
    version: Signal<u64>,
}

impl<K, T> Clone for Derived<K, T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<K, T> Copy for Derived<K, T> {}

impl<K, T> Derived<K, T>
where
    K: Eq + Hash + Clone + 'static,
    T: Clone + 'static,
{
    pub fn source(&self) -> Source<K, T> {
        Source(self.holder.get_untracked())
    }
    /// Number of items, O(1); tracks the view.
    #[track_caller]
    pub fn len(&self) -> usize {
        self.version.get();
        self.holder.get_untracked().len()
    }
    #[track_caller]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    #[track_caller]
    pub fn keys(&self) -> Vec<K> {
        self.source().keys()
    }
    pub fn filter(&self, pred: impl Fn(&T) -> bool + 'static) -> Derived<K, T> {
        self.source().filter(pred)
    }
    /// The items transformed by `f`, same keys and order ([`Source::map`]).
    pub fn map<U: Clone + PartialEq + 'static>(&self, f: impl Fn(&T) -> U + 'static) -> Derived<K, U> {
        self.source().map(f)
    }
    pub fn sort_by<S: Ord + Clone + 'static>(&self, key: impl Fn(&T) -> S + 'static) -> Derived<K, T> {
        self.source().sort_by(key)
    }
    pub fn group_by<G: Ord + Hash + Clone + Display + 'static>(&self, key: impl Fn(&T) -> G + 'static) -> Groups<G, K, T> {
        self.source().group_by(key)
    }
}

// ---- filter ----

struct FilterInner<K, T> {
    up: Rc<dyn Stage<K, T>>,
    pred: Box<dyn Fn(&T) -> bool>,
    seq: RefCell<Seq<K>>,
    emitter: Emitter<K, T>,
}

impl<K: Eq + Hash + Clone + 'static, T: Clone + 'static> FilterInner<K, T> {
    /// Rank of a new member whose upstream index is `ui`: members before it upstream.
    fn rank(&self, ui: usize) -> usize {
        self.seq.borrow().count_while(|m| self.up.index_of(m).expect("member upstream") < ui)
    }
    fn add(&self, ui: usize, k: &K, t: &T) {
        let r = self.rank(ui);
        self.seq.borrow_mut().insert(r, k.clone());
        self.emitter.insert(r, k, t);
    }
}

impl<K: Eq + Hash + Clone + 'static, T: Clone + 'static> Observer<K, T> for FilterInner<K, T> {
    fn inserted(&self, ui: usize, k: &K, t: &T) {
        if (self.pred)(t) {
            self.add(ui, k, t);
        }
    }
    fn removed(&self, _: usize, k: &K, t: &T) {
        let i = self.seq.borrow_mut().remove(k);
        if let Some(i) = i {
            self.emitter.remove(i, k, t);
        }
    }
    fn updated(&self, k: &K, old: &T, new: &T) {
        let member = self.seq.borrow().contains(k);
        match (member, (self.pred)(new)) {
            (false, true) => self.add(self.up.index_of(k).expect("key upstream"), k, new),
            (true, false) => self.removed(0, k, old),
            (true, true) => self.emitter.update(k, old, new),
            (false, false) => {}
        }
    }
    fn moved(&self, k: &K, _: usize, to: usize, t: &T) {
        let from = self.seq.borrow_mut().remove(k);
        if let Some(from) = from {
            let r = self.rank(to);
            self.seq.borrow_mut().insert(r, k.clone());
            if r != from {
                self.emitter.moved(from, r, k, t);
            }
        }
    }
}

impl<K: Eq + Hash + Clone + 'static, T: Clone + 'static> Stage<K, T> for FilterInner<K, T> {
    fn index_of(&self, k: &K) -> Option<usize> {
        self.seq.borrow().index_of(k)
    }
    fn len(&self) -> usize {
        self.seq.borrow().len()
    }
    fn snapshot(&self) -> Vec<K> {
        self.seq.borrow().to_vec()
    }
    fn item(&self, k: &K) -> Option<ReadSignal<T>> {
        self.up.item(k)
    }
    fn emitter(&self) -> &Emitter<K, T> {
        &self.emitter
    }
}

// ---- sort ----

struct SortedInner<K, T, S> {
    up: Rc<dyn Stage<K, T>>,
    key: Box<dyn Fn(&T) -> S>,
    seq: RefCell<Seq<K>>,
    /// Sort key plus the order in which the item entered the view (stable ties).
    ord: RefCell<HashMap<K, (S, u64)>>,
    next: Cell<u64>,
    emitter: Emitter<K, T>,
}

impl<K: Eq + Hash + Clone + 'static, T: Clone + 'static, S: Ord + Clone + 'static> SortedInner<K, T, S> {
    fn rank(&self, o: &(S, u64)) -> usize {
        let ord = self.ord.borrow();
        self.seq.borrow().count_while(|m| ord[m] < *o)
    }
}

impl<K: Eq + Hash + Clone + 'static, T: Clone + 'static, S: Ord + Clone + 'static> Observer<K, T> for SortedInner<K, T, S> {
    fn inserted(&self, _: usize, k: &K, t: &T) {
        let o = ((self.key)(t), self.next.get());
        self.next.set(o.1 + 1);
        let r = self.rank(&o);
        self.ord.borrow_mut().insert(k.clone(), o);
        self.seq.borrow_mut().insert(r, k.clone());
        self.emitter.insert(r, k, t);
    }
    fn removed(&self, _: usize, k: &K, t: &T) {
        self.ord.borrow_mut().remove(k);
        let i = self.seq.borrow_mut().remove(k);
        if let Some(i) = i {
            self.emitter.remove(i, k, t);
        }
    }
    fn updated(&self, k: &K, old: &T, new: &T) {
        let s = (self.key)(new);
        let Some((s0, n)) = self.ord.borrow().get(k).cloned() else { return };
        if s != s0 {
            let from = self.seq.borrow_mut().remove(k).expect("member");
            let o = (s, n);
            let r = self.rank(&o);
            self.ord.borrow_mut().insert(k.clone(), o);
            self.seq.borrow_mut().insert(r, k.clone());
            if r != from {
                self.emitter.moved(from, r, k, new);
            }
        }
        self.emitter.update(k, old, new);
    }
    fn moved(&self, _: &K, _: usize, _: usize, _: &T) {
        // Upstream order does not affect a sorted view.
    }
}

impl<K: Eq + Hash + Clone + 'static, T: Clone + 'static, S: Ord + Clone + 'static> Stage<K, T> for SortedInner<K, T, S> {
    fn index_of(&self, k: &K) -> Option<usize> {
        self.seq.borrow().index_of(k)
    }
    fn len(&self) -> usize {
        self.seq.borrow().len()
    }
    fn snapshot(&self) -> Vec<K> {
        self.seq.borrow().to_vec()
    }
    fn item(&self, k: &K) -> Option<ReadSignal<T>> {
        self.up.item(k)
    }
    fn emitter(&self) -> &Emitter<K, T> {
        &self.emitter
    }
}

// ---- group ----

// ---------------------------------------------------------------------------------------------
// map
// ---------------------------------------------------------------------------------------------

/// `map`: one output signal per key; order and positions are the upstream's.
struct MapInner<K, T, U> {
    up: Rc<dyn Stage<K, T>>,
    f: Box<dyn Fn(&T) -> U>,
    out: RefCell<HashMap<K, Signal<U>>>,
    emitter: Emitter<K, U>,
}

impl<K, T, U> Drop for MapInner<K, T, U> {
    fn drop(&mut self) {
        for s in self.out.get_mut().values() {
            s.dispose();
        }
    }
}

impl<K: Eq + Hash + Clone + 'static, T: Clone + 'static, U: Clone + PartialEq + 'static> Observer<K, T> for MapInner<K, T, U> {
    fn inserted(&self, index: usize, k: &K, t: &T) {
        let u = (self.f)(t);
        self.out.borrow_mut().insert(k.clone(), Signal::detached(u.clone()));
        self.emitter.insert(index, k, &u);
    }
    fn removed(&self, index: usize, k: &K, _: &T) {
        let s = self.out.borrow_mut().remove(k);
        if let Some(s) = s {
            let u = s.get_untracked();
            self.emitter.remove(index, k, &u);
            s.dispose();
        }
    }
    fn updated(&self, k: &K, _: &T, new: &T) {
        let u = (self.f)(new);
        let s = self.out.borrow().get(k).copied();
        if let Some(s) = s {
            let old = s.get_untracked();
            if old != u {
                s.set_internal(u.clone());
                self.emitter.update(k, &old, &u);
            }
        }
    }
    fn moved(&self, k: &K, from: usize, to: usize, _: &T) {
        let s = self.out.borrow().get(k).copied();
        if let Some(s) = s {
            self.emitter.moved(from, to, k, &s.get_untracked());
        }
    }
}

impl<K: Eq + Hash + Clone + 'static, T: Clone + 'static, U: Clone + PartialEq + 'static> Stage<K, U> for MapInner<K, T, U> {
    fn index_of(&self, k: &K) -> Option<usize> {
        self.up.index_of(k)
    }
    fn len(&self) -> usize {
        self.up.len()
    }
    fn snapshot(&self) -> Vec<K> {
        self.up.snapshot()
    }
    fn item(&self, k: &K) -> Option<ReadSignal<U>> {
        self.out.borrow().get(k).map(|s| s.read_only())
    }
    fn emitter(&self) -> &Emitter<K, U> {
        &self.emitter
    }
}

/// One group's summary, the item of a `Groups` stage.
#[derive(Clone, PartialEq, Debug)]
pub struct GroupInfo<G> {
    pub key: G,
    pub len: usize,
}

/// One group's members, in upstream order: a stage of its own.
struct Members<K, T> {
    up: Rc<dyn Stage<K, T>>,
    seq: RefCell<Seq<K>>,
    emitter: Emitter<K, T>,
}

impl<K: Eq + Hash + Clone + 'static, T: Clone + 'static> Members<K, T> {
    fn rank(&self, ui: usize) -> usize {
        self.seq.borrow().count_while(|m| self.up.index_of(m).expect("member upstream") < ui)
    }
    fn add(&self, ui: usize, k: &K, t: &T) {
        let r = self.rank(ui);
        self.seq.borrow_mut().insert(r, k.clone());
        self.emitter.insert(r, k, t);
    }
    fn drop_key(&self, k: &K, t: &T) {
        let i = self.seq.borrow_mut().remove(k);
        if let Some(i) = i {
            self.emitter.remove(i, k, t);
        }
    }
    fn reposition(&self, k: &K, to: usize, t: &T) {
        let from = self.seq.borrow_mut().remove(k).expect("member");
        let r = self.rank(to);
        self.seq.borrow_mut().insert(r, k.clone());
        if r != from {
            self.emitter.moved(from, r, k, t);
        }
    }
}

impl<K: Eq + Hash + Clone + 'static, T: Clone + 'static> Stage<K, T> for Members<K, T> {
    fn index_of(&self, k: &K) -> Option<usize> {
        self.seq.borrow().index_of(k)
    }
    fn len(&self) -> usize {
        self.seq.borrow().len()
    }
    fn snapshot(&self) -> Vec<K> {
        self.seq.borrow().to_vec()
    }
    fn item(&self, k: &K) -> Option<ReadSignal<T>> {
        self.up.item(k)
    }
    fn emitter(&self) -> &Emitter<K, T> {
        &self.emitter
    }
}

struct GroupEntry<G, K, T> {
    members: Rc<Members<K, T>>,
    info: Signal<GroupInfo<G>>,
}

struct GroupsInner<G, K, T> {
    up: Rc<dyn Stage<K, T>>,
    key: Box<dyn Fn(&T) -> G>,
    groups: RefCell<Seq<G>>,
    members: RefCell<HashMap<G, GroupEntry<G, K, T>>>,
    member_of: RefCell<HashMap<K, G>>,
    emitter: Emitter<G, GroupInfo<G>>,
}

impl<G, K, T> Drop for GroupsInner<G, K, T> {
    fn drop(&mut self) {
        for e in self.members.get_mut().values() {
            e.info.dispose();
        }
    }
}

impl<G, K, T> GroupsInner<G, K, T>
where
    G: Ord + Hash + Clone + Display + 'static,
    K: Eq + Hash + Clone + 'static,
    T: Clone + 'static,
{
    fn entry(&self, g: &G) -> (Rc<Members<K, T>>, Signal<GroupInfo<G>>) {
        if let Some(e) = self.members.borrow().get(g) {
            return (e.members.clone(), e.info);
        }
        let members = Rc::new(Members { up: self.up.clone(), seq: RefCell::new(Seq::new()), emitter: Emitter::new() });
        let info = Signal::detached(GroupInfo { key: g.clone(), len: 0 });
        self.members.borrow_mut().insert(g.clone(), GroupEntry { members: members.clone(), info });
        let r = self.groups.borrow().count_while(|x| x < g);
        self.groups.borrow_mut().insert(r, g.clone());
        self.emitter.insert(r, g, &info.get_untracked());
        (members, info)
    }

    fn resize(&self, g: &G, info: Signal<GroupInfo<G>>, delta: isize) {
        let old = info.get_untracked();
        let new = GroupInfo { key: old.key.clone(), len: (old.len as isize + delta) as usize };
        info.set_internal(new.clone());
        if new.len == 0 {
            let i = self.groups.borrow_mut().remove(g).expect("group");
            self.members.borrow_mut().remove(g);
            self.emitter.remove(i, g, &new);
            info.dispose();
        } else {
            self.emitter.update(g, &old, &new);
        }
    }
}

impl<G, K, T> Observer<K, T> for GroupsInner<G, K, T>
where
    G: Ord + Hash + Clone + Display + 'static,
    K: Eq + Hash + Clone + 'static,
    T: Clone + 'static,
{
    fn inserted(&self, ui: usize, k: &K, t: &T) {
        let g = (self.key)(t);
        let (members, info) = self.entry(&g);
        members.add(ui, k, t);
        self.member_of.borrow_mut().insert(k.clone(), g.clone());
        self.resize(&g, info, 1);
    }
    fn removed(&self, _: usize, k: &K, t: &T) {
        let Some(g) = self.member_of.borrow_mut().remove(k) else { return };
        let (members, info) = {
            let m = self.members.borrow();
            let e = &m[&g];
            (e.members.clone(), e.info)
        };
        members.drop_key(k, t);
        self.resize(&g, info, -1);
    }
    fn updated(&self, k: &K, old: &T, new: &T) {
        let Some(g0) = self.member_of.borrow().get(k).cloned() else { return };
        if (self.key)(new) == g0 {
            let members = self.members.borrow()[&g0].members.clone();
            members.emitter.update(k, old, new);
        } else {
            self.removed(0, k, old);
            self.inserted(self.up.index_of(k).expect("key upstream"), k, new);
        }
    }
    fn moved(&self, k: &K, _: usize, to: usize, t: &T) {
        let Some(g) = self.member_of.borrow().get(k).cloned() else { return };
        let members = self.members.borrow()[&g].members.clone();
        members.reposition(k, to, t);
    }
}

impl<G, K, T> Stage<G, GroupInfo<G>> for GroupsInner<G, K, T>
where
    G: Ord + Hash + Clone + Display + 'static,
    K: Eq + Hash + Clone + 'static,
    T: Clone + 'static,
{
    fn index_of(&self, g: &G) -> Option<usize> {
        self.groups.borrow().index_of(g)
    }
    fn len(&self) -> usize {
        self.groups.borrow().len()
    }
    fn snapshot(&self) -> Vec<G> {
        self.groups.borrow().to_vec()
    }
    fn item(&self, g: &G) -> Option<ReadSignal<GroupInfo<G>>> {
        self.members.borrow().get(g).map(|e| e.info.read_only())
    }
    fn emitter(&self) -> &Emitter<G, GroupInfo<G>> {
        &self.emitter
    }
}

/// Groups of a collection: a stage of `GroupInfo` items keyed by group, plus each group's members.
pub struct Groups<G: 'static, K: 'static, T: 'static> {
    holder: Signal<Rc<GroupsInner<G, K, T>>>,
    version: Signal<u64>,
}

impl<G, K, T> Clone for Groups<G, K, T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<G, K, T> Copy for Groups<G, K, T> {}

impl<G, K, T> Groups<G, K, T>
where
    G: Ord + Hash + Clone + Display + 'static,
    K: Eq + Hash + Clone + 'static,
    T: Clone + 'static,
{
    /// The groups, ordered by key; each item is the group's `GroupInfo` (key and size).
    pub fn source(&self) -> Source<G, GroupInfo<G>> {
        Source(self.holder.get_untracked())
    }
    /// Number of groups; tracks the groups.
    #[track_caller]
    pub fn len(&self) -> usize {
        self.version.get();
        self.holder.get_untracked().groups.borrow().len()
    }
    #[track_caller]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// One group's members, in upstream order (`None` if the group does not exist).
    pub fn members(&self, g: &G) -> Option<Source<K, T>> {
        self.holder.get_untracked().members.borrow().get(g).map(|e| Source(e.members.clone() as Rc<dyn Stage<K, T>>))
    }
}

// ---- flatten: group headers and members as one stage ----

/// A key of a flattened grouping: a group's header or one of its members.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum FlatKey<G, K> {
    Header(G),
    Item(K),
}

impl<G: Display, K: Display> Display for FlatKey<G, K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FlatKey::Header(g) => write!(f, "h:{g}"),
            FlatKey::Item(k) => write!(f, "i:{k}"),
        }
    }
}

/// The item of a flattened grouping.
#[derive(Clone, PartialEq, Debug)]
pub enum FlatItem<G, T> {
    Header(GroupInfo<G>),
    Item(T),
}

impl<G, T> FlatItem<G, T> {
    pub fn item(&self) -> Option<&T> {
        match self {
            FlatItem::Item(t) => Some(t),
            FlatItem::Header(_) => None,
        }
    }
    pub fn header(&self) -> Option<&GroupInfo<G>> {
        match self {
            FlatItem::Header(h) => Some(h),
            FlatItem::Item(_) => None,
        }
    }
}

struct FlatInner<G, K, T> {
    groups: Rc<GroupsInner<G, K, T>>,
    /// Group order, mirrored to find the header that follows a new group.
    order: RefCell<Seq<G>>,
    seq: RefCell<Seq<FlatKey<G, K>>>,
    items: RefCell<FlatSignals<G, K, T>>,
    adapters: RefCell<HashMap<G, Rc<dyn Observer<K, T>>>>,
    me: RefCell<Weak<FlatInner<G, K, T>>>,
    emitter: Emitter<FlatKey<G, K>, FlatItem<G, T>>,
}

impl<G, K, T> Drop for FlatInner<G, K, T> {
    fn drop(&mut self) {
        for s in self.items.get_mut().values() {
            s.dispose();
        }
    }
}

/// Forwards one group's member events to the flattened stage.
struct MemberAdapter<G, K, T> {
    flat: Weak<FlatInner<G, K, T>>,
    group: G,
}

impl<G, K, T> Observer<K, T> for MemberAdapter<G, K, T>
where
    G: Ord + Hash + Clone + Display + 'static,
    K: Eq + Hash + Clone + 'static,
    T: Clone + 'static,
{
    fn inserted(&self, r: usize, k: &K, t: &T) {
        if let Some(f) = self.flat.upgrade() {
            let at = f.header_index(&self.group) + 1 + r;
            f.put(at, FlatKey::Item(k.clone()), FlatItem::Item(t.clone()));
        }
    }
    fn removed(&self, _: usize, k: &K, _: &T) {
        if let Some(f) = self.flat.upgrade() {
            f.take(&FlatKey::Item(k.clone()));
        }
    }
    fn updated(&self, k: &K, old: &T, new: &T) {
        if let Some(f) = self.flat.upgrade() {
            f.replace(&FlatKey::Item(k.clone()), FlatItem::Item(old.clone()), FlatItem::Item(new.clone()));
        }
    }
    fn moved(&self, k: &K, _: usize, to: usize, _: &T) {
        if let Some(f) = self.flat.upgrade() {
            let key = FlatKey::Item(k.clone());
            let from = f.seq.borrow_mut().remove(&key).expect("member in flat");
            let at = f.header_index(&self.group) + 1 + to;
            f.seq.borrow_mut().insert(at, key.clone());
            let item = f.items.borrow()[&key].get_untracked();
            f.emitter.moved(from, at, &key, &item);
        }
    }
}

impl<G, K, T> FlatInner<G, K, T>
where
    G: Ord + Hash + Clone + Display + 'static,
    K: Eq + Hash + Clone + 'static,
    T: Clone + 'static,
{
    fn header_index(&self, g: &G) -> usize {
        self.seq.borrow().index_of(&FlatKey::Header(g.clone())).expect("header")
    }
    fn put(&self, at: usize, key: FlatKey<G, K>, item: FlatItem<G, T>) {
        self.seq.borrow_mut().insert(at, key.clone());
        self.items.borrow_mut().insert(key.clone(), Signal::detached(item.clone()));
        self.emitter.insert(at, &key, &item);
    }
    fn take(&self, key: &FlatKey<G, K>) {
        let i = self.seq.borrow_mut().remove(key);
        let s = self.items.borrow_mut().remove(key);
        if let (Some(i), Some(s)) = (i, s) {
            let item = s.get_untracked();
            self.emitter.remove(i, key, &item);
            s.dispose();
        }
    }
    fn replace(&self, key: &FlatKey<G, K>, old: FlatItem<G, T>, new: FlatItem<G, T>) {
        let s = self.items.borrow().get(key).copied();
        if let Some(s) = s {
            s.set_internal(new.clone());
            self.emitter.update(key, &old, &new);
        }
    }
    fn watch_members(&self, g: &G) {
        let members = self.groups.members.borrow()[g].members.clone();
        let adapter: Rc<dyn Observer<K, T>> = Rc::new(MemberAdapter { flat: self.me.borrow().clone(), group: g.clone() });
        members.emitter.observers.borrow_mut().push(Rc::downgrade(&adapter));
        self.adapters.borrow_mut().insert(g.clone(), adapter);
    }
}

impl<G, K, T> Observer<G, GroupInfo<G>> for FlatInner<G, K, T>
where
    G: Ord + Hash + Clone + Display + 'static,
    K: Eq + Hash + Clone + 'static,
    T: Clone + 'static,
{
    fn inserted(&self, gr: usize, g: &G, info: &GroupInfo<G>) {
        self.order.borrow_mut().insert(gr, g.clone());
        let next = self.order.borrow().nth(gr + 1).cloned();
        let at = match next {
            Some(n) => self.header_index(&n),
            None => self.seq.borrow().len(),
        };
        self.put(at, FlatKey::Header(g.clone()), FlatItem::Header(info.clone()));
        // Members arrive after the group is created: watch them from now on.
        self.watch_members(g);
    }
    fn removed(&self, _: usize, g: &G, _: &GroupInfo<G>) {
        self.order.borrow_mut().remove(g);
        self.adapters.borrow_mut().remove(g);
        self.take(&FlatKey::Header(g.clone()));
    }
    fn updated(&self, g: &G, old: &GroupInfo<G>, new: &GroupInfo<G>) {
        self.replace(&FlatKey::Header(g.clone()), FlatItem::Header(old.clone()), FlatItem::Header(new.clone()));
    }
    fn moved(&self, _: &G, _: usize, _: usize, _: &GroupInfo<G>) {
        // Groups are ordered by key; they never move.
    }
}

impl<G, K, T> Stage<FlatKey<G, K>, FlatItem<G, T>> for FlatInner<G, K, T>
where
    G: Ord + Hash + Clone + Display + 'static,
    K: Eq + Hash + Clone + 'static,
    T: Clone + 'static,
{
    fn index_of(&self, k: &FlatKey<G, K>) -> Option<usize> {
        self.seq.borrow().index_of(k)
    }
    fn len(&self) -> usize {
        self.seq.borrow().len()
    }
    fn snapshot(&self) -> Vec<FlatKey<G, K>> {
        self.seq.borrow().to_vec()
    }
    fn item(&self, k: &FlatKey<G, K>) -> Option<ReadSignal<FlatItem<G, T>>> {
        self.items.borrow().get(k).map(|s| s.read_only())
    }
    fn emitter(&self) -> &Emitter<FlatKey<G, K>, FlatItem<G, T>> {
        &self.emitter
    }
}

impl<G, K, T> Groups<G, K, T>
where
    G: Ord + Hash + Clone + Display + 'static,
    K: Eq + Hash + Clone + 'static,
    T: Clone + 'static,
{
    /// Each group's header followed by its members, as one stage (DBSP's flat_map): suitable
    /// for `virtual_each`. Positions are O(log n): a member sits at its header's index + 1 + its
    /// rank in the group.
    pub fn flatten(&self) -> Derived<FlatKey<G, K>, FlatItem<G, T>> {
        let groups = self.holder.get_untracked();
        let flat = Rc::new(FlatInner {
            groups: groups.clone(),
            order: RefCell::new(Seq::new()),
            seq: RefCell::new(Seq::new()),
            items: RefCell::new(HashMap::new()),
            adapters: RefCell::new(HashMap::new()),
            me: RefCell::new(Weak::new()),
            emitter: Emitter::new(),
        });
        *flat.me.borrow_mut() = Rc::downgrade(&flat);
        for g in groups.groups.borrow().to_vec() {
            let (members, info) = {
                let m = groups.members.borrow();
                (m[&g].members.clone(), m[&g].info)
            };
            let n = flat.order.borrow().len();
            flat.order.borrow_mut().insert(n, g.clone());
            let at = flat.seq.borrow().len();
            flat.put(at, FlatKey::Header(g.clone()), FlatItem::Header(info.get_untracked()));
            for k in members.snapshot() {
                if let Some(it) = members.item(&k) {
                    let at = flat.seq.borrow().len();
                    flat.put(at, FlatKey::Item(k), FlatItem::Item(it.get_untracked()));
                }
            }
            flat.watch_members(&g);
        }
        let obs: Rc<dyn Observer<G, GroupInfo<G>>> = flat.clone();
        groups.emitter.observers.borrow_mut().push(Rc::downgrade(&obs));
        let version = flat.emitter.version;
        Derived { holder: Signal::scoped(flat as Rc<dyn Stage<FlatKey<G, K>, FlatItem<G, T>>>), version }
    }
}

#[cfg(test)]
mod tests {
    use crate::reactive::graph::{Ctx, Runtime};
    use crate::reactive::list::{Change, FlatKey, list};

    #[derive(Clone, PartialEq, Debug)]
    struct Row {
        id: u32,
        a: u8,
        b: u8,
    }

    /// Differential test (DBSP style): every view, after every random operation, equals the same
    /// view computed from scratch; replaying each view's log reproduces its keys.
    #[test]
    fn composed_views_match_from_scratch_and_logs_replay() {
        let rt = Runtime::new();
        rt.enter(|| {
            let scope = rt.create_scope(None, "t");
            // Idle, not Build: the checks below read views outside any binding on purpose.
            rt.with_frame(Ctx::Idle, None, Some(Some(scope)), || {
                let l = list("rows", Vec::<Row>::new, |r| r.id);
                let f = l.filter(|r| r.a % 2 == 0);
                let s = f.sort_by(|r| (r.b, r.id));
                let g = l.group_by(|r| r.a % 3);
                let flat = l.sort_by(|r| (r.b, r.id)).group_by(|r| r.a % 3).flatten();
                // map after a sort (moves pass through it) and a filter after a map.
                let m = s.map(|r| r.a as u32 * 10 + r.b as u32);
                let mf = l.map(|r| r.b).filter(|b| *b >= 2);
                let m_cursor = m.source().0.emitter().log.borrow_mut().subscribe();
                let mut m_replay: Vec<u32> = Vec::new();
                let flat_cursor = flat.source().0.emitter().log.borrow_mut().subscribe();
                let mut flat_replay: Vec<FlatKey<u8, u32>> = Vec::new();
                let logs = [l.source(), f.source(), s.source()];
                let cursors: Vec<_> = logs.iter().map(|src| src.0.emitter().log.borrow_mut().subscribe()).collect();
                let mut replay: Vec<Vec<u32>> = vec![Vec::new(); 3];
                let mut x: u64 = 99;
                let mut rnd = |m: u64| {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    x % m.max(1)
                };
                let mut next = 0u32;
                for step in 0..3_000 {
                    rt.with_frame(Ctx::Handler, None, None, || {
                        let keys = l.keys();
                        match rnd(10) {
                            0..=3 => {
                                l.insert(rnd(keys.len() as u64 + 1) as usize, Row { id: next, a: rnd(6) as u8, b: rnd(5) as u8 });
                                next += 1;
                            }
                            4 | 5 if !keys.is_empty() => l.remove(&keys[rnd(keys.len() as u64) as usize]),
                            6..=8 if !keys.is_empty() => {
                                let (va, vb) = (rnd(6) as u8, rnd(5) as u8);
                                let pick = rnd(2);
                                l.update(&keys[rnd(keys.len() as u64) as usize], |r| if pick == 0 { r.a = va } else { r.b = vb });
                            }
                            9 if step % 97 == 0 => l.retain(|r| r.b != 0),
                            _ => {}
                        }
                    });
                    rt.flush();
                    let all: Vec<Row> = l.keys().iter().map(|k| l.item(k).unwrap().get_untracked()).collect();
                    let want_f: Vec<u32> = all.iter().filter(|r| r.a % 2 == 0).map(|r| r.id).collect();
                    let mut sorted: Vec<&Row> = all.iter().filter(|r| r.a % 2 == 0).collect();
                    sorted.sort_by_key(|r| (r.b, r.id));
                    let want_s: Vec<u32> = sorted.iter().map(|r| r.id).collect();
                    assert_eq!(f.keys(), want_f, "filter at step {step}");
                    assert_eq!(m.keys(), want_s, "map keys at step {step}");
                    for (k, r) in sorted.iter().map(|r| (r.id, r)) {
                        assert_eq!(m.source().item(&k).unwrap().get_untracked(), r.a as u32 * 10 + r.b as u32, "map value at step {step}");
                    }
                    let want_mf: Vec<u32> = all.iter().filter(|r| r.b >= 2).map(|r| r.id).collect();
                    assert_eq!(mf.keys(), want_mf, "filter after map at step {step}");
                    for ch in m.source().0.emitter().log.borrow_mut().take(&m_cursor) {
                        match ch {
                            Change::Insert { key, index } => m_replay.insert(index, key),
                            Change::Remove { key, index } => assert_eq!(m_replay.remove(index), key),
                            Change::Move { key, from, to } => {
                                assert_eq!(m_replay.remove(from), key);
                                m_replay.insert(to, key);
                            }
                        }
                    }
                    assert_eq!(m_replay, want_s, "map log replay at step {step}");
                    assert_eq!(s.keys(), want_s, "sort at step {step}");
                    let mut want_g: Vec<u8> = all.iter().map(|r| r.a % 3).collect();
                    want_g.sort();
                    want_g.dedup();
                    assert_eq!(g.source().keys(), want_g, "groups at step {step}");
                    for gk in &want_g {
                        let members: Vec<u32> = all.iter().filter(|r| r.a % 3 == *gk).map(|r| r.id).collect();
                        assert_eq!(g.members(gk).unwrap().keys(), members, "group {gk} at step {step}");
                        assert_eq!(g.source().item(gk).unwrap().get_untracked().len, members.len());
                    }
                    // flatten: every group's header, then its members ordered by (b, id).
                    let mut want_flat = Vec::new();
                    for gk in &want_g {
                        want_flat.push(FlatKey::Header(*gk));
                        let mut ms: Vec<&Row> = all.iter().filter(|r| r.a % 3 == *gk).collect();
                        ms.sort_by_key(|r| (r.b, r.id));
                        want_flat.extend(ms.iter().map(|r| FlatKey::Item(r.id)));
                    }
                    assert_eq!(flat.keys(), want_flat, "flatten at step {step}");
                    for ch in flat.source().0.emitter().log.borrow_mut().take(&flat_cursor) {
                        match ch {
                            Change::Insert { key, index } => flat_replay.insert(index, key),
                            Change::Remove { key, index } => assert_eq!(flat_replay.remove(index), key),
                            Change::Move { key, from, to } => {
                                assert_eq!(flat_replay.remove(from), key);
                                flat_replay.insert(to, key);
                            }
                        }
                    }
                    assert_eq!(flat_replay, want_flat, "flatten log replay at step {step}");
                    for (i, src) in logs.iter().enumerate() {
                        for ch in src.0.emitter().log.borrow_mut().take(&cursors[i]) {
                            match ch {
                                Change::Insert { key, index } => replay[i].insert(index, key),
                                Change::Remove { key, index } => assert_eq!(replay[i].remove(index), key),
                                Change::Move { key, from, to } => {
                                    assert_eq!(replay[i].remove(from), key);
                                    replay[i].insert(to, key);
                                }
                            }
                        }
                        assert_eq!(replay[i], src.keys(), "log replay of stage {i} at step {step}");
                    }
                }
            });
        });
        assert!(rt.diagnostics().is_empty(), "{:?}", rt.diagnostics());
    }

    /// `map` calls its closure once per changed item and passes an update on only when the result
    /// differs; a view closure that reads a signal is reported.
    #[test]
    fn map_recomputes_changed_items_only_and_view_reads_are_reported() {
        use crate::reactive::graph::state;
        use std::cell::Cell;
        use std::rc::Rc;
        let rt = Runtime::new();
        rt.enter(|| {
            let scope = rt.create_scope(None, "t");
            rt.with_frame(Ctx::Idle, None, Some(Some(scope)), || {
                let l = list("rows", || (0..100u32).map(|id| Row { id, a: 0, b: 0 }).collect(), |r| r.id);
                let calls = Rc::new(Cell::new(0));
                let c = calls.clone();
                let m = l.map(move |r| {
                    c.set(c.get() + 1);
                    r.a
                });
                // Downstream of the map: counts the updates that reach it.
                let seen = Rc::new(Cell::new(0));
                let s2 = seen.clone();
                let after = m.filter(move |_| {
                    s2.set(s2.get() + 1);
                    true
                });
                assert_eq!(calls.get(), 100);
                let (c0, s0) = (calls.get(), seen.get());
                rt.with_frame(Ctx::Handler, None, None, || l.update(&7, |r| r.b = 9));
                assert_eq!(calls.get(), c0 + 1, "one item changed: one call");
                assert_eq!(seen.get(), s0, "its mapped value did not change: nothing passed on");
                rt.with_frame(Ctx::Handler, None, None, || l.update(&7, |r| r.a = 3));
                assert_eq!(seen.get(), s0 + 1, "a changed value is passed on");
                assert_eq!(m.source().item(&7).unwrap().get_untracked(), 3);
                assert_eq!(after.keys().len(), 100);
                rt.with_frame(Ctx::Handler, None, None, || l.remove(&7));
                assert!(m.source().item(&7).is_none());

                // A view closure reading state: reported once, at the read.
                let limit = state("limit", || 5u8);
                let _stale = l.filter(move |r| r.a < limit.get());
                rt.with_frame(Ctx::Handler, None, None, || l.update(&8, |r| r.a = 1));
            });
        });
        let d = rt.diagnostics();
        assert_eq!(d.iter().filter(|d| d.code.id == "VIEW_READS_STATE").count(), 1, "{d:?}");
    }
}

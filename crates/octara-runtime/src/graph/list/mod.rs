//! Keyed reactive lists with per-row reads and incrementally maintained views.

mod filter;
mod group;
mod join;
mod map;
pub mod seq;
mod sort;
#[cfg(test)]
mod tests;

use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

use octara_model::diagnostics::{Diagnostic, codes};
use rustc_hash::{FxHashMap, FxHashSet};
use smallvec::SmallVec;

use self::seq::Seq;
use super::delta::{Changes, Cursor, DeltaLog};
use crate::arena::{NodeKind, ReactiveId, ReactiveWorld};
use crate::cx::{BuildCx, ReadCx, WriteCx};

pub use self::group::{FlatItem, FlatKey, GroupInfo, Groups};
pub use self::join::JOIN_FANOUT_ROWS;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListDelta<K> {
    Insert(K),
    Remove(K),
    Move { key: K, to: usize },
    Update(K),
    Reset,
}

macro_rules! list_handle {
    ($(#[$attrs:meta])* $name:ident) => {
        $(#[$attrs])*
        pub struct $name<K, T> {
            id: ReactiveId,
            marker: PhantomData<fn() -> (K, T)>,
        }

        impl<K, T> $name<K, T> {
            fn new(id: ReactiveId) -> Self {
                Self { id, marker: PhantomData }
            }
        }

        impl<K, T> Copy for $name<K, T> {}
        impl<K, T> Clone for $name<K, T> {
            fn clone(&self) -> Self { *self }
        }
        impl<K, T> PartialEq for $name<K, T> {
            fn eq(&self, other: &Self) -> bool { self.id == other.id }
        }
        impl<K, T> Eq for $name<K, T> {}
        impl<K, T> Hash for $name<K, T> {
            fn hash<H: Hasher>(&self, state: &mut H) { self.id.hash(state); }
        }
        impl<K, T> std::fmt::Debug for $name<K, T> {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.debug_tuple(stringify!($name)).field(&self.id).finish()
            }
        }
    };
}

list_handle!(
    /// A mutable keyed list. Writes require a [`WriteCx`].
    StateList
);
list_handle!(
    /// Read-only access to a source list or an incrementally computed view.
    ReadList
);

impl<K, T> StateList<K, T> {
    pub fn read_only(self) -> ReadList<K, T> {
        ReadList::new(self.id)
    }
}

impl<K, T> From<StateList<K, T>> for ReadList<K, T> {
    fn from(list: StateList<K, T>) -> Self {
        list.read_only()
    }
}

/// Inputs accepted by list reads.
pub trait IntoReadList<K, T> {
    fn into_read_list(self) -> ReadList<K, T>;
}

impl<K, T> IntoReadList<K, T> for ReadList<K, T> {
    fn into_read_list(self) -> ReadList<K, T> {
        self
    }
}

impl<K, T> IntoReadList<K, T> for StateList<K, T> {
    fn into_read_list(self) -> ReadList<K, T> {
        self.read_only()
    }
}

impl<K, T> IntoReadList<K, T> for &ReadList<K, T> {
    fn into_read_list(self) -> ReadList<K, T> {
        *self
    }
}

impl<K, T> IntoReadList<K, T> for &StateList<K, T> {
    fn into_read_list(self) -> ReadList<K, T> {
        self.read_only()
    }
}

/// A key-addressed row input. It follows removal and reinsertion of the same key.
pub struct RowSignal<K, T> {
    list: ReadList<K, T>,
    key: K,
}

impl<K: Clone, T> Clone for RowSignal<K, T> {
    fn clone(&self) -> Self {
        Self { list: self.list, key: self.key.clone() }
    }
}

impl<K: Eq, T> PartialEq for RowSignal<K, T> {
    fn eq(&self, other: &Self) -> bool {
        self.list == other.list && self.key == other.key
    }
}

impl<K: Eq, T> Eq for RowSignal<K, T> {}

impl<K, T> ReadList<K, T> {
    pub fn row(self, key: K) -> RowSignal<K, T> {
        RowSignal { list: self, key }
    }
}

impl<K: Eq + Hash + 'static, T: 'static> RowSignal<K, T> {
    pub fn key(&self) -> &K {
        &self.key
    }

    pub fn with<R>(&self, read: &mut ReadCx<'_>, f: impl FnOnce(Option<&T>) -> R) -> R {
        read.with_row(self.list, &self.key, f)
    }

    pub fn read(&self, read: &mut ReadCx<'_>) -> Option<T>
    where
        T: Clone,
    {
        read.get_row(self.list, &self.key)
    }
}

struct ListData<K, T> {
    seq: Seq<K>,
    rows: FxHashMap<K, T>,
    upstream: Option<ReadList<K, T>>,
    log: DeltaLog<ListDelta<K>>,
    structural_log: DeltaLog<ListDelta<K>>,
    structural: ReactiveId,
    row_nodes: FxHashMap<K, ReactiveId>,
    clone_key: fn(&K) -> K,
    keyed_upstream: Option<KeyedUpstream<K>>,
    parameter: Option<ReactiveId>,
    keyed_match: Option<KeyedUpstream<K>>,
}

type RowLookup<K> = fn(&mut ReactiveWorld, ReactiveId, &K) -> Option<ReactiveId>;

struct KeyedUpstream<K> {
    id: ReactiveId,
    lookup: RowLookup<K>,
}

impl<K> Copy for KeyedUpstream<K> {}
impl<K> Clone for KeyedUpstream<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> KeyedUpstream<K> {
    fn row(self, world: &mut ReactiveWorld, key: &K) -> Option<ReactiveId> {
        (self.lookup)(world, self.id, key)
    }
}

impl<K: Eq + Hash + 'static, T: 'static> ReadList<K, T> {
    fn data(self, world: &ReactiveWorld) -> &ListData<K, T> {
        world.value(self.id)
    }

    fn data_mut(self, world: &mut ReactiveWorld) -> &mut ListData<K, T> {
        world.value_mut(self.id)
    }

    // Callers pull the list once before borrowing values from one or more inputs.
    fn value<'a>(self, world: &'a ReactiveWorld, key: &K) -> Option<&'a T> {
        let mut list = self;
        loop {
            let data = list.data(world);
            if !data.seq.contains(key) {
                return None;
            }
            match data.upstream {
                Some(upstream) => list = upstream,
                None => return data.rows.get(key),
            }
        }
    }

    fn row_node(self, world: &mut ReactiveWorld, key: &K) -> ReactiveId {
        let data = self.data(world);
        if let Some(&node) = data.row_nodes.get(key) {
            return node;
        }
        let clone_key = data.clone_key;
        let upstream = data.keyed_upstream;
        let keyed_match = data.keyed_match;
        let parameter = data.parameter;
        let (scope, window) = world.owner(self.id);
        let node = world.alloc_node(NodeKind::RowSource, scope, window);
        self.data_mut(world).row_nodes.insert(clone_key(key), node);
        if let Some(input) = upstream.and_then(|upstream| upstream.row(world, key)) {
            world.add_dependency(node, input);
            if let Some(parameter) = parameter {
                world.add_dependency(node, parameter);
            }
            let keyed_match = keyed_match.map(|source| (source, clone_key(key)));
            world.init_compute(node, move |read| {
                read.world.ensure_clean(self.id);
                if let Some((source, key)) = &keyed_match
                    && let Some(matched) = source.row(read.world, key)
                {
                    read.track(matched);
                }
                false
            });
        } else if world.hot(self.id).kind == NodeKind::ListOp {
            world.add_dependency(node, self.id);
            world.init_compute(node, |_| false);
        }
        node
    }

    fn set_keyed_upstream<U: 'static>(self, world: &mut ReactiveWorld, upstream: ReadList<K, U>, parameter: Option<ReactiveId>) {
        let data = self.data_mut(world);
        data.keyed_upstream =
            Some(KeyedUpstream { id: upstream.id, lookup: |world, id, key| Some(ReadList::<K, U>::new(id).row_node(world, key)) });
        data.parameter = parameter;
    }

    fn reconcile(self, world: &mut ReactiveWorld, rows: Vec<(K, T)>) -> bool
    where
        K: Clone,
        T: PartialEq,
    {
        let mut next = FxHashSet::default();
        for (key, _) in &rows {
            if !next.insert(key) {
                world.report(Diagnostic::new(&codes::DUPLICATE_ROW_KEY, "Cannot reconcile a list with duplicate keys"));
                return false;
            }
        }

        let data = self.data_mut(world);
        let mut deltas = Vec::new();
        for key in data.seq.to_vec() {
            if !next.contains(&key) {
                data.seq.remove(&key);
                data.rows.remove(&key);
                deltas.push(ListDelta::Remove(key));
            }
        }
        drop(next);
        for (index, (key, value)) in rows.into_iter().enumerate() {
            if let Some(previous) = data.rows.get_mut(&key) {
                if data.seq.move_to(&key, index) {
                    deltas.push(ListDelta::Move { key: key.clone(), to: index });
                }
                if *previous != value {
                    *previous = value;
                    deltas.push(ListDelta::Update(key));
                }
            } else {
                data.seq.insert(index, key.clone());
                data.rows.insert(key.clone(), value);
                deltas.push(ListDelta::Insert(key));
            }
        }
        self.emit(world, deltas)
    }

    fn emit(self, world: &mut ReactiveWorld, deltas: impl IntoIterator<Item = ListDelta<K>>) -> bool {
        let data = self.data_mut(world);
        let mut changed = false;
        let mut topology_changed = false;
        let mut row_nodes = SmallVec::<[ReactiveId; 4]>::new();
        let structural_node = data.structural;
        for delta in deltas {
            changed = true;
            match &delta {
                ListDelta::Reset => {
                    row_nodes.extend(data.row_nodes.values().copied());
                    data.structural_log.reset();
                    topology_changed = true;
                }
                ListDelta::Insert(key) | ListDelta::Remove(key) => {
                    row_nodes.extend(data.row_nodes.get(key).copied());
                    topology_changed = true;
                }
                ListDelta::Move { .. } => topology_changed = true,
                ListDelta::Update(key) => row_nodes.extend(data.row_nodes.get(key).copied()),
            }
            match delta {
                ListDelta::Reset => data.log.reset(),
                ListDelta::Update(_) => data.log.push(delta),
                _ => {
                    let clone_key = data.clone_key;
                    let topology_delta = match &delta {
                        ListDelta::Insert(key) => ListDelta::Insert(clone_key(key)),
                        ListDelta::Remove(key) => ListDelta::Remove(clone_key(key)),
                        ListDelta::Move { key, to } => ListDelta::Move { key: clone_key(key), to: *to },
                        _ => unreachable!(),
                    };
                    data.structural_log.push(topology_delta);
                    data.log.push(delta);
                }
            }
        }
        if topology_changed {
            world.mark_changed(structural_node);
        }
        for node in row_nodes {
            world.mark_changed(node);
        }
        changed
    }
}

impl BuildCx<'_> {
    #[track_caller]
    pub fn list<K: Clone + Eq + Hash + 'static, T: 'static>(&mut self) -> StateList<K, T> {
        StateList::new(self.allocate_list(NodeKind::ListSource, None::<ReadList<K, T>>).id)
    }

    /// Creates a list with initial rows available to the view that declares it.
    #[track_caller]
    pub fn list_from<K: Clone + Eq + Hash + 'static, T: 'static>(&mut self, rows: impl IntoIterator<Item = (K, T)>) -> StateList<K, T> {
        let list = self.list();
        let data = list.read_only().data_mut(self.world);
        let mut duplicate = false;
        for (key, item) in rows {
            if let std::collections::hash_map::Entry::Vacant(row) = data.rows.entry(key.clone()) {
                row.insert(item);
                data.seq.push(key);
            } else {
                duplicate = true;
                break;
            }
        }
        if duplicate {
            data.rows.clear();
            data.seq = Seq::new();
            self.world.report(Diagnostic::new(&codes::DUPLICATE_ROW_KEY, "Cannot initialize a list with duplicate keys"));
        }
        list
    }

    /// Derive keyed rows from tracked reads. Equal values keep their row identity
    /// and only the resulting insertions, removals, moves, and edits are published.
    /// Prefer a list operator when transforming an existing list incrementally.
    #[track_caller]
    pub fn derive_list<K: Clone + Eq + Hash + 'static, T: PartialEq + 'static>(
        &mut self,
        mut derive: impl FnMut(&mut ReadCx<'_>) -> Vec<(K, T)> + 'static,
    ) -> ReadList<K, T> {
        let list = self.allocate_list(NodeKind::ListOp, None);
        self.world.init_compute(list.id, move |read| {
            let rows = derive(read);
            list.reconcile(read.world, rows)
        });
        list
    }

    fn allocate_list<K: Clone + Eq + Hash + 'static, T: 'static>(
        &mut self,
        kind: NodeKind,
        upstream: Option<ReadList<K, T>>,
    ) -> ReadList<K, T> {
        let structural = self.world.alloc_node(NodeKind::RowSource, self.scope, self.window);
        let data = ListData {
            seq: Seq::new(),
            rows: FxHashMap::default(),
            upstream,
            log: DeltaLog::default(),
            structural_log: DeltaLog::default(),
            structural,
            row_nodes: FxHashMap::default(),
            clone_key: K::clone,
            keyed_upstream: None,
            parameter: None,
            keyed_match: None,
        };
        let id = self.world.alloc_value(kind, self.scope, self.window, data);
        if kind == NodeKind::ListOp {
            self.world.init_compute(structural, |_| false);
            self.world.add_dependency(structural, id);
        }
        ReadList::new(id)
    }

    #[track_caller]
    pub fn sample_list_keys<K: Clone + Eq + Hash + 'static, T: 'static>(&mut self, list: impl IntoReadList<K, T>) -> Vec<K> {
        ReadCx::new(self.world).keys(list)
    }
}

impl ReadCx<'_> {
    fn list_structure<K: Eq + Hash + 'static, T: 'static>(&mut self, list: ReadList<K, T>) {
        self.world.ensure_clean(list.id);
        self.track(list.data(self.world).structural);
    }

    #[track_caller]
    pub fn len<K: Eq + Hash + 'static, T: 'static>(&mut self, list: impl IntoReadList<K, T>) -> usize {
        let list = list.into_read_list();
        self.list_structure(list);
        list.data(self.world).seq.len()
    }

    #[track_caller]
    pub fn keys<K: Clone + Eq + Hash + 'static, T: 'static>(&mut self, list: impl IntoReadList<K, T>) -> Vec<K> {
        let list = list.into_read_list();
        self.list_structure(list);
        list.data(self.world).seq.to_vec()
    }

    #[track_caller]
    pub fn rank<K: Eq + Hash + 'static, T: 'static>(&mut self, list: impl IntoReadList<K, T>, key: &K) -> Option<usize> {
        let list = list.into_read_list();
        self.list_structure(list);
        list.data(self.world).seq.index_of(key)
    }

    #[track_caller]
    pub fn row<K: Clone + Eq + Hash + 'static, T: 'static>(&mut self, list: impl IntoReadList<K, T>, index: usize) -> Option<K> {
        let list = list.into_read_list();
        self.list_structure(list);
        list.data(self.world).seq.nth(index).cloned()
    }

    #[track_caller]
    pub fn with_row<K: Eq + Hash + 'static, T: 'static, R>(
        &mut self,
        list: impl IntoReadList<K, T>,
        key: &K,
        f: impl FnOnce(Option<&T>) -> R,
    ) -> R {
        let list = list.into_read_list();
        self.world.ensure_clean(list.id);
        if self.observer().is_some() {
            let node = list.row_node(self.world, key);
            self.track(node);
        }
        f(list.value(self.world, key))
    }

    #[track_caller]
    pub fn get_row<K: Eq + Hash + 'static, T: Clone + 'static>(&mut self, list: impl IntoReadList<K, T>, key: &K) -> Option<T> {
        self.with_row(list, key, |row| row.cloned())
    }

    #[track_caller]
    pub fn list_cursor<K: Eq + Hash + 'static, T: 'static>(&mut self, list: impl IntoReadList<K, T>) -> Cursor {
        let list = list.into_read_list();
        self.track(list.id);
        list.data(self.world).log.cursor()
    }

    #[track_caller]
    pub fn list_changes<K: Clone + Eq + Hash + 'static, T: 'static>(
        &mut self,
        list: impl IntoReadList<K, T>,
        cursor: &mut Cursor,
    ) -> Changes<ListDelta<K>> {
        let list = list.into_read_list();
        self.track(list.id);
        list.data(self.world).log.take(cursor)
    }

    #[track_caller]
    pub fn list_topology_cursor<K: Eq + Hash + 'static, T: 'static>(&mut self, list: impl IntoReadList<K, T>) -> Cursor {
        let list = list.into_read_list();
        self.list_structure(list);
        list.data(self.world).structural_log.cursor()
    }

    #[track_caller]
    pub fn list_topology_changes<K: Clone + Eq + Hash + 'static, T: 'static>(
        &mut self,
        list: impl IntoReadList<K, T>,
        cursor: &mut Cursor,
    ) -> Changes<ListDelta<K>> {
        let list = list.into_read_list();
        self.list_structure(list);
        list.data(self.world).structural_log.take(cursor)
    }
}

impl WriteCx<'_> {
    #[track_caller]
    pub fn len<K: Eq + Hash + 'static, T: 'static>(&mut self, list: impl IntoReadList<K, T>) -> usize {
        ReadCx::new(self.world).len(list)
    }

    #[track_caller]
    pub fn keys<K: Clone + Eq + Hash + 'static, T: 'static>(&mut self, list: impl IntoReadList<K, T>) -> Vec<K> {
        ReadCx::new(self.world).keys(list)
    }

    #[track_caller]
    pub fn get_row<K: Eq + Hash + 'static, T: Clone + 'static>(&mut self, list: impl IntoReadList<K, T>, key: &K) -> Option<T> {
        self.with_row(list, key, |row| row.cloned())
    }

    #[track_caller]
    pub fn with_row<K: Eq + Hash + 'static, T: 'static, R>(
        &mut self,
        list: impl IntoReadList<K, T>,
        key: &K,
        f: impl FnOnce(Option<&T>) -> R,
    ) -> R {
        ReadCx::new(self.world).with_row(list, key, f)
    }

    #[track_caller]
    pub fn insert_row<K: Clone + Eq + Hash + 'static, T: 'static>(&mut self, list: StateList<K, T>, index: usize, key: K, item: T) {
        let list = list.read_only();
        let data = list.data_mut(self.world);
        if data.seq.contains(&key) {
            self.world.report(Diagnostic::new(&codes::DUPLICATE_ROW_KEY, "Cannot insert an existing list key"));
            return;
        }
        data.seq.insert(index, key.clone());
        data.rows.insert(key.clone(), item);
        list.emit(self.world, [ListDelta::Insert(key)]);
        self.world.mark_changed(list.id);
    }

    #[track_caller]
    pub fn push_row<K: Clone + Eq + Hash + 'static, T: 'static>(&mut self, list: StateList<K, T>, key: K, item: T) {
        self.insert_row(list, usize::MAX, key, item);
    }

    #[track_caller]
    pub fn update_row<K: Clone + Eq + Hash + 'static, T: 'static>(&mut self, list: StateList<K, T>, key: &K, f: impl FnOnce(&mut T)) {
        let list = list.read_only();
        if let Some(row) = list.data_mut(self.world).rows.get_mut(key) {
            f(row);
            list.emit(self.world, [ListDelta::Update(key.clone())]);
            self.world.mark_changed(list.id);
        }
    }

    #[track_caller]
    pub fn remove_row<K: Clone + Eq + Hash + 'static, T: 'static>(&mut self, list: StateList<K, T>, key: &K) -> Option<T> {
        let list = list.read_only();
        let data = list.data_mut(self.world);
        let row = data.rows.remove(key)?;
        data.seq.remove(key);
        list.emit(self.world, [ListDelta::Remove(key.clone())]);
        self.world.mark_changed(list.id);
        Some(row)
    }

    #[track_caller]
    pub fn move_row<K: Clone + Eq + Hash + 'static, T: 'static>(&mut self, list: StateList<K, T>, key: &K, to: usize) {
        let list = list.read_only();
        let data = list.data_mut(self.world);
        if data.seq.move_to(key, to) {
            let to = data.seq.index_of(key).expect("moved key exists");
            list.emit(self.world, [ListDelta::Move { key: key.clone(), to }]);
            self.world.mark_changed(list.id);
        }
    }

    #[track_caller]
    pub fn reconcile_list<K: Clone + Eq + Hash + 'static, T: PartialEq + 'static>(&mut self, list: StateList<K, T>, rows: Vec<(K, T)>) {
        let list = list.read_only();
        if list.reconcile(self.world, rows) {
            self.world.mark_changed(list.id);
        }
    }
}

/// A batch records each affected key once while preserving its first appearance in the log.
struct Batch<K> {
    keys: Vec<K>,
    flags: FxHashMap<K, EditFlags>,
    reset: bool,
}

#[derive(Default)]
struct EditFlags {
    position: bool,
    value: bool,
}

impl<K: Clone + Eq + Hash> Batch<K> {
    fn new(changes: Changes<ListDelta<K>>) -> Self {
        let mut batch = Self { keys: Vec::new(), flags: FxHashMap::default(), reset: false };
        let Changes::Delta(deltas) = changes else {
            batch.reset = true;
            return batch;
        };
        for delta in deltas {
            let (key, position, value) = match delta {
                ListDelta::Insert(key) | ListDelta::Remove(key) => (key, true, true),
                ListDelta::Move { key, .. } => (key, true, false),
                ListDelta::Update(key) => (key, false, true),
                ListDelta::Reset => {
                    batch.reset = true;
                    break;
                }
            };
            let flags = batch.flags.entry(key.clone()).or_insert_with(|| {
                batch.keys.push(key);
                EditFlags::default()
            });
            flags.position |= position;
            flags.value |= value;
        }
        batch
    }

    fn topology_keys(&self) -> impl Iterator<Item = &K> {
        self.keys.iter().filter(|key| self.flags[*key].position)
    }

    fn value_keys(&self) -> impl Iterator<Item = &K> {
        self.keys.iter().filter(|key| self.flags[*key].value)
    }
}

/// Mirrors a batch using final upstream ranks. Insertions carry keys only, so resolve their
/// positions after upstream has finished applying the batch.
fn mirror<K: Clone + Eq + Hash>(output: &mut Seq<K>, input: &Seq<K>, batch: &Batch<K>, deltas: &mut Vec<ListDelta<K>>) {
    let mut live = Vec::new();
    for key in batch.topology_keys() {
        if let Some(rank) = input.index_of(key) {
            live.push((rank, key));
        } else if output.remove(key).is_some() {
            deltas.push(ListDelta::Remove(key.clone()));
        }
    }
    live.sort_unstable_by_key(|(rank, _)| *rank);
    for (rank, key) in live.into_iter().rev() {
        let anchor = input.nth(rank + 1).and_then(|next| output.index_of(next)).unwrap_or(output.len());
        place_before(output, key, anchor, deltas);
    }
}

fn place_before<K: Clone + Eq + Hash>(seq: &mut Seq<K>, key: &K, anchor: usize, deltas: &mut Vec<ListDelta<K>>) {
    if let Some(from) = seq.index_of(key) {
        let to = anchor - usize::from(from < anchor);
        if seq.move_to(key, to) {
            deltas.push(ListDelta::Move { key: key.clone(), to });
        }
    } else {
        seq.insert(anchor, key.clone());
        deltas.push(ListDelta::Insert(key.clone()));
    }
}

enum Parameter<P> {
    Constant(P),
    Signal(crate::graph::value::ReadSignal<P>),
}

impl<P: 'static> Parameter<P> {
    fn id(&self) -> Option<ReactiveId> {
        match self {
            Self::Constant(_) => None,
            Self::Signal(signal) => Some(signal.id()),
        }
    }

    fn track(&self, read: &mut ReadCx<'_>, previous: &mut Option<u64>) -> bool {
        let Self::Signal(signal) = self else { return false };
        let id = signal.id();
        read.track(id);
        let revision = read.world.revision(id);
        previous.replace(revision) != Some(revision)
    }

    fn get<'a>(&'a self, world: &'a ReactiveWorld) -> &'a P {
        match self {
            Self::Constant(value) => value,
            Self::Signal(signal) => world.value(signal.id()),
        }
    }
}

use super::*;
use std::fmt::{self, Display};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupInfo<G> {
    pub key: G,
    pub size: usize,
}

/// Flat member keys retain their identity as members change groups.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FlatKey<G, K> {
    Header(G),
    Item(K),
}

impl<G: Display, K: Display> Display for FlatKey<G, K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Header(group) => write!(f, "h:{group}"),
            Self::Item(key) => write!(f, "i:{key}"),
        }
    }
}

/// A header or a live input for a member. Keeping a row input avoids copying member values.
pub enum FlatItem<G, K, T> {
    Header(GroupInfo<G>),
    Item(RowSignal<K, T>),
}

impl<G: Clone, K: Clone, T> Clone for FlatItem<G, K, T> {
    fn clone(&self) -> Self {
        match self {
            Self::Header(info) => Self::Header(info.clone()),
            Self::Item(row) => Self::Item(row.clone()),
        }
    }
}

impl<G, K, T> FlatItem<G, K, T> {
    pub fn item(&self) -> Option<&RowSignal<K, T>> {
        match self {
            Self::Header(_) => None,
            Self::Item(item) => Some(item),
        }
    }

    pub fn header(&self) -> Option<&GroupInfo<G>> {
        match self {
            Self::Header(info) => Some(info),
            Self::Item(_) => None,
        }
    }
}

type FlatList<G, K, T> = ReadList<FlatKey<G, K>, FlatItem<G, K, T>>;

pub struct Groups<G, K, T> {
    headers: ReadList<G, GroupInfo<G>>,
    flat: FlatList<G, K, T>,
}

impl<G, K, T> Copy for Groups<G, K, T> {}
impl<G, K, T> Clone for Groups<G, K, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<G, K, T> Groups<G, K, T> {
    pub fn read_only(self) -> ReadList<G, GroupInfo<G>> {
        self.headers
    }

    pub fn flatten(self) -> FlatList<G, K, T> {
        self.flat
    }
}

struct GroupIndex<G, K> {
    groups: Seq<G>,
    members: FxHashMap<G, Seq<K>>,
    membership: FxHashMap<K, G>,
    #[cfg(test)]
    comparisons: usize,
}

impl<G: Clone + Ord + Hash, K: Clone + Eq + Hash> GroupIndex<G, K> {
    fn new() -> Self {
        Self {
            groups: Seq::new(),
            members: FxHashMap::default(),
            membership: FxHashMap::default(),
            #[cfg(test)]
            comparisons: 0,
        }
    }

    fn remove_member(&mut self, key: &K) -> Option<G> {
        let group = self.membership.remove(key)?;
        let members = self.members.get_mut(&group).expect("group exists");
        members.remove(key);
        self.groups.set_weight(&group, (members.len() + 1) as f64);
        Some(group)
    }

    fn insert_member(&mut self, upstream: &Seq<K>, key: &K, group: &G) -> (Option<usize>, usize) {
        let mut created_header = None;
        if !self.groups.contains(group) {
            let rank = self.groups.count_while(|other| other < group);
            created_header = Some(self.groups.prefix_weight(rank) as usize);
            self.groups.insert(rank, group.clone());
        }
        let members = self.members.entry(group.clone()).or_default();
        let order = upstream.order_key(key);
        let within = members.count_while(|other| {
            #[cfg(test)]
            {
                self.comparisons += 1;
            }
            upstream.order_key(other) < order
        });
        members.insert(within, key.clone());
        self.membership.insert(key.clone(), group.clone());
        let group_rank = self.groups.index_of(group).expect("group header exists");
        self.groups.set_weight(group, (members.len() + 1) as f64);
        let flat_rank = self.groups.prefix_weight(group_rank) as usize + 1 + within;
        (created_header, flat_rank)
    }
}

impl<K: Clone + Eq + Hash + 'static, T: 'static> ReadList<K, T> {
    /// Groups ordered by key, with members in the order of the upstream list.
    #[track_caller]
    pub fn group_by<G: Clone + Ord + Hash + 'static>(self, build: &mut BuildCx<'_>, key_fn: impl Fn(&T) -> G + 'static) -> Groups<G, K, T> {
        let flat: FlatList<G, K, T> = build.allocate_list(NodeKind::ListOp, None);
        flat.data_mut(build.world).keyed_upstream = Some(KeyedUpstream {
            id: self.id,
            lookup: |world, id, key| match key {
                FlatKey::Header(_) => None,
                FlatKey::Item(key) => Some(ReadList::<K, T>::new(id).row_node(world, key)),
            },
        });
        let headers = build.allocate_list(NodeKind::ListOp, None);
        build.world.add_dependency(headers.id, flat.id);
        build.world.init_compute(headers.id, |_| false);
        let mut cursor = Cursor::default();
        let mut index = GroupIndex::new();
        build.world.init_compute(flat.id, move |read| {
            let batch = Batch::new(read.list_changes(self, &mut cursor));
            let world = &mut *read.world;
            let mut seq = std::mem::take(&mut flat.data_mut(world).seq);
            let previous_has_rows = !seq.is_empty();
            if batch.reset {
                index = GroupIndex::new();
                seq = Seq::new();
            }

            let upstream = &self.data(world).seq;
            upstream.prepare_order();
            let mut changed_groups = std::collections::BTreeSet::new();
            let mut members = Vec::new();
            let mut remaining = Vec::new();
            let mut header_deltas = Vec::new();
            let mut flat_deltas = Vec::new();
            let keys = if batch.reset { upstream.to_vec() } else { batch.keys.clone() };
            for key in keys {
                let flags = batch.flags.get(&key);
                let old_group = index.membership.get(&key);
                let new_group = match flags {
                    Some(flags) if !flags.value => old_group.cloned(),
                    _ => self.value(world, &key).map(&key_fn),
                };
                let flat_key = FlatKey::Item(key.clone());
                if old_group == new_group.as_ref() && flags.is_some_and(|flags| !flags.position) {
                    if new_group.is_some() {
                        flat_deltas.push(ListDelta::Update(flat_key));
                    }
                    continue;
                }
                let old_rank = seq.index_of(&flat_key);
                if let Some(group) = index.remove_member(&key) {
                    changed_groups.insert(group);
                    seq.remove(&flat_key);
                }
                if let Some(group) = new_group {
                    let order = upstream.order_key(&key);
                    members.push((order, key, group, old_rank));
                } else if old_rank.is_some() {
                    flat_deltas.push(ListDelta::Remove(flat_key));
                }
            }

            members.sort_unstable_by_key(|(order, ..)| *order);
            let mut new_headers = Vec::new();
            for (_, key, group, old_rank) in members {
                let (header, flat_rank) = index.insert_member(upstream, &key, &group);
                if let Some(header_rank) = header {
                    seq.insert(header_rank, FlatKey::Header(group.clone()));
                    new_headers.push(group.clone());
                }
                changed_groups.insert(group);
                let flat_key = FlatKey::Item(key);
                seq.insert(flat_rank, flat_key.clone());
                if let Some(from) = old_rank {
                    remaining.push((flat_key, from));
                } else {
                    flat_deltas.push(ListDelta::Insert(flat_key));
                }
            }

            let mut infos = Vec::new();
            for group in changed_groups {
                let size = index.members[&group].len();
                let header = FlatKey::Header(group.clone());
                if size == 0 {
                    index.members.remove(&group);
                    index.groups.remove(&group);
                    seq.remove(&header);
                    flat_deltas.push(ListDelta::Remove(header));
                    header_deltas.push(ListDelta::Remove(group));
                } else {
                    let rank = index.groups.index_of(&group).expect("header present");
                    infos.push((rank, GroupInfo { key: group, size }));
                }
            }

            let other_topology_changes = remaining.len() > 1
                || !new_headers.is_empty()
                || flat_deltas.iter().any(|delta| matches!(delta, ListDelta::Insert(_) | ListDelta::Remove(_)));
            for (key, from) in remaining {
                let to = seq.index_of(&key).expect("moved member exists");
                if from != to || other_topology_changes {
                    flat_deltas.push(ListDelta::Move { key: key.clone(), to });
                }
                if let FlatKey::Item(member) = &key
                    && batch.flags[member].value
                {
                    flat_deltas.push(ListDelta::Update(key));
                }
            }
            flat_deltas.extend(new_headers.into_iter().map(FlatKey::Header).map(ListDelta::Insert));

            let flat_data = flat.data_mut(world);
            flat_data.seq = seq;
            if batch.reset {
                flat_data.rows.clear();
            }
            for delta in &flat_deltas {
                match delta {
                    ListDelta::Insert(FlatKey::Item(key)) => {
                        flat_data.rows.insert(FlatKey::Item(key.clone()), FlatItem::Item(self.row(key.clone())));
                    }
                    ListDelta::Remove(key) => {
                        flat_data.rows.remove(key);
                    }
                    _ => {}
                }
            }

            if batch.reset {
                let data = headers.data_mut(world);
                data.seq = Seq::new();
                data.rows.clear();
            }
            for delta in &header_deltas {
                if let ListDelta::Remove(key) = delta {
                    let data = headers.data_mut(world);
                    data.seq.remove(key);
                    data.rows.remove(key);
                }
            }
            infos.sort_unstable_by_key(|(rank, _)| *rank);
            for (rank, info) in infos {
                let data = headers.data_mut(world);
                let key = info.key.clone();
                if !data.seq.contains(&key) {
                    data.seq.insert(rank, key.clone());
                    header_deltas.push(ListDelta::Insert(key.clone()));
                } else if data.rows.get(&key) != Some(&info) {
                    header_deltas.push(ListDelta::Update(key.clone()));
                    flat_deltas.push(ListDelta::Update(FlatKey::Header(key.clone())));
                } else {
                    continue;
                }
                data.rows.insert(key.clone(), info.clone());
                flat.data_mut(world).rows.insert(FlatKey::Header(key), FlatItem::Header(info));
            }

            if batch.reset && (previous_has_rows || !flat_deltas.is_empty()) {
                header_deltas = vec![ListDelta::Reset];
                flat_deltas = vec![ListDelta::Reset];
            }
            if headers.emit(world, header_deltas) {
                world.mark_changed(headers.id);
            }
            flat.emit(world, flat_deltas)
        });
        Groups { headers, flat }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_rank_search_scales_logarithmically() {
        const MOVES: usize = 256;

        fn reorder(size: usize) -> (usize, usize) {
            let mut upstream = Seq::from_keys(0..size);
            upstream.prepare_order();
            let mut index = GroupIndex::new();
            for key in 0..size {
                index.insert_member(&upstream, &key, &0);
            }
            let mut peak = 0;
            let mut total = 0;
            for step in 0..MOVES {
                let key = (step * 79 + 13) % size;
                let to = (step * 199 + 47) % size;
                upstream.move_to(&key, to);
                index.remove_member(&key);
                let before = index.comparisons;
                assert_eq!(index.insert_member(&upstream, &key, &0), (None, to + 1));
                let comparisons = index.comparisons - before;
                peak = peak.max(comparisons);
                total += comparisons;
            }
            assert_eq!(index.members[&0].to_vec(), upstream.to_vec());
            (peak, total)
        }

        let small = reorder(1_024);
        let large = reorder(65_536);
        eprintln!("group_by {MOVES} moves, peak and total comparisons: 1,024 members {small:?}, 65,536 members {large:?}");
        assert!(large.0 <= 64);
        assert!(large.1 < 2 * small.1);
    }
}

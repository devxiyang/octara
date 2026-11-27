use super::*;
use crate::graph::value::IntoRead;

impl<K: Clone + Eq + Hash + 'static, T: 'static> ReadList<K, T> {
    /// Sorts by key, breaking ties by the time the row entered the view.
    #[track_caller]
    pub fn sort_by<S: Ord + 'static>(self, build: &mut BuildCx<'_>, key_fn: impl Fn(&T) -> S + 'static) -> Self {
        self.sort_parameter(build, Parameter::Constant(()), move |value, ()| key_fn(value))
    }

    #[track_caller]
    pub fn sort_by_with<P: 'static, S: Ord + 'static>(
        self,
        build: &mut BuildCx<'_>,
        parameter: impl IntoRead<P>,
        key_fn: impl Fn(&T, &P) -> S + 'static,
    ) -> Self {
        self.sort_parameter(build, Parameter::Signal(parameter.into_read()), key_fn)
    }

    fn sort_parameter<P: 'static, S: Ord + 'static>(
        self,
        build: &mut BuildCx<'_>,
        parameter: Parameter<P>,
        key_fn: impl Fn(&T, &P) -> S + 'static,
    ) -> Self {
        let output = build.allocate_list(NodeKind::ListOp, Some(self));
        output.set_keyed_upstream(build.world, self, parameter.id());
        let mut cursor = Cursor::default();
        let mut parameter_revision = None;
        let mut order = FxHashMap::<K, (S, u64)>::default();
        let mut insertion = 0_u64;
        build.world.init_compute(output.id, move |read| {
            let batch = Batch::new(read.list_changes(self, &mut cursor));
            let parameter_changed = parameter.track(read, &mut parameter_revision);
            let world = &mut *read.world;
            let mut seq = std::mem::take(&mut output.data_mut(world).seq);
            let upstream = &self.data(world).seq;
            let parameter = parameter.get(world);
            let mut deltas = Vec::new();
            let mut reposition = Vec::new();

            if batch.reset || parameter_changed {
                for key in seq.to_vec() {
                    if !upstream.contains(&key) {
                        seq.remove(&key);
                        order.remove(&key);
                        deltas.push(ListDelta::Remove(key));
                    }
                }
            }

            let keys = if batch.reset || parameter_changed { upstream.to_vec() } else { batch.value_keys().cloned().collect() };
            for key in keys {
                let Some(value) = self.value(world, &key) else {
                    if seq.remove(&key).is_some() {
                        order.remove(&key);
                        deltas.push(ListDelta::Remove(key));
                    }
                    continue;
                };
                let sort_key = key_fn(value, parameter);
                if let Some((old_key, _)) = order.get_mut(&key) {
                    if *old_key != sort_key {
                        *old_key = sort_key;
                        reposition.push(key.clone());
                    }
                    if batch.flags.get(&key).is_some_and(|flags| flags.value) {
                        deltas.push(ListDelta::Update(key));
                    }
                } else {
                    order.insert(key.clone(), (sort_key, insertion));
                    insertion += 1;
                    reposition.push(key);
                }
            }

            if batch.reset || parameter_changed {
                let mut desired = order.keys().cloned().collect::<Vec<_>>();
                desired.sort_unstable_by(|a, b| order[a].cmp(&order[b]));
                reorder_minimal(&mut seq, &desired, &mut deltas);
                if batch.reset && (!seq.is_empty() || !deltas.is_empty()) {
                    deltas = vec![ListDelta::Reset];
                }
            } else {
                reposition.sort_unstable_by(|a, b| order[a].cmp(&order[b]));
                reorder_changed(&mut seq, &order, &reposition, &mut deltas);
            }
            output.data_mut(world).seq = seq;
            output.emit(world, deltas)
        });
        output
    }
}

/// Computes final anchors against the unchanged rows, then applies only actual moves.
fn reorder_changed<K: Clone + Eq + Hash, S: Ord>(
    seq: &mut Seq<K>,
    order: &FxHashMap<K, (S, u64)>,
    changed: &[K],
    deltas: &mut Vec<ListDelta<K>>,
) {
    let mut previous = changed.iter().filter_map(|key| Some((seq.index_of(key)?, key))).collect::<Vec<_>>();
    for key in changed {
        seq.remove(key);
    }
    let mut anchors = Vec::with_capacity(changed.len());
    for key in changed {
        let rank = seq.count_while(|other| order[other] < order[key]);
        seq.insert(rank, key.clone());
        // Later insertions can immediately precede the old anchor.
        anchors.push((rank, key));
    }
    let anchors = anchors.into_iter().map(|(rank, key)| (seq.nth(rank + 1).cloned(), key)).collect::<Vec<_>>();
    for key in changed {
        seq.remove(key);
    }
    previous.sort_unstable_by_key(|(rank, _)| *rank);
    for (rank, key) in previous {
        seq.insert(rank, key.clone());
    }
    for (next, key) in anchors.into_iter().rev() {
        let anchor = next.as_ref().and_then(|next| seq.index_of(next)).unwrap_or(seq.len());
        place_before(seq, key, anchor, deltas);
    }
}

/// Keep a longest increasing subsequence of existing ranks, then insert or move every other
/// key at most once. This avoids transient moves when a parameter changes all sort keys.
fn reorder_minimal<K: Clone + Eq + Hash>(seq: &mut Seq<K>, desired: &[K], deltas: &mut Vec<ListDelta<K>>) {
    let mut tails = Vec::<(usize, usize)>::new();
    let mut previous = vec![None; desired.len()];
    for (index, key) in desired.iter().enumerate() {
        let Some(rank) = seq.index_of(key) else { continue };
        let end = tails.partition_point(|(current, _)| *current < rank);
        previous[index] = end.checked_sub(1).map(|left| tails[left].1);
        if end == tails.len() {
            tails.push((rank, index));
        } else {
            tails[end] = (rank, index);
        }
    }
    let mut keep = vec![false; desired.len()];
    let mut current = tails.last().map(|entry| entry.1);
    while let Some(index) = current {
        keep[index] = true;
        current = previous[index];
    }
    for (index, key) in desired.iter().enumerate().rev() {
        if !keep[index] {
            let anchor = desired.get(index + 1).and_then(|next| seq.index_of(next)).unwrap_or(seq.len());
            place_before(seq, key, anchor, deltas);
        }
    }
}

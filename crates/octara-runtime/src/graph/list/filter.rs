use super::*;
use crate::graph::value::IntoRead;

impl<K: Clone + Eq + Hash + 'static, T: 'static> ReadList<K, T> {
    #[track_caller]
    pub fn filter(self, build: &mut BuildCx<'_>, predicate: impl Fn(&T) -> bool + 'static) -> Self {
        self.filter_parameter(build, Parameter::Constant(()), move |value, ()| predicate(value))
    }

    #[track_caller]
    pub fn filter_with<P: 'static>(
        self,
        build: &mut BuildCx<'_>,
        parameter: impl IntoRead<P>,
        predicate: impl Fn(&T, &P) -> bool + 'static,
    ) -> Self {
        self.filter_parameter(build, Parameter::Signal(parameter.into_read()), predicate)
    }

    fn filter_parameter<P: 'static>(
        self,
        build: &mut BuildCx<'_>,
        parameter: Parameter<P>,
        predicate: impl Fn(&T, &P) -> bool + 'static,
    ) -> Self {
        let output = build.allocate_list(NodeKind::ListOp, Some(self));
        output.set_keyed_upstream(build.world, self, parameter.id());
        let mut cursor = Cursor::default();
        let mut parameter_revision = None;
        // The weighted input mirror gives each included row a rank in O(log n).
        let mut included_order = Seq::new();
        build.world.init_compute(output.id, move |read| {
            let batch = Batch::new(read.list_changes(self, &mut cursor));
            let parameter_changed = parameter.track(read, &mut parameter_revision);
            let world = &mut *read.world;
            let mut seq = std::mem::take(&mut output.data_mut(world).seq);
            let upstream = &self.data(world).seq;
            let parameter = parameter.get(world);

            if batch.reset {
                included_order = Seq::new();
                let mut rebuilt = Seq::new();
                for key in upstream.to_vec() {
                    let included = self.value(world, &key).is_some_and(|value| predicate(value, parameter));
                    included_order.insert_weighted(included_order.len(), key.clone(), if included { 1.0 } else { 0.0 });
                    if included {
                        rebuilt.push(key);
                    }
                }
                let changed = !seq.is_empty() || !rebuilt.is_empty();
                output.data_mut(world).seq = rebuilt;
                return changed && output.emit(world, [ListDelta::Reset]);
            }

            let mut deltas = Vec::new();
            let mut relocate = Vec::new();
            let mut mirrored = Vec::new();
            mirror(&mut included_order, upstream, &batch, &mut mirrored);
            for delta in mirrored {
                if let ListDelta::Remove(key) = delta
                    && seq.remove(&key).is_some()
                {
                    deltas.push(ListDelta::Remove(key));
                }
            }

            let value_keys = if parameter_changed { upstream.to_vec() } else { batch.value_keys().cloned().collect() };
            for key in value_keys {
                let was_included = seq.contains(&key);
                let included = self.value(world, &key).is_some_and(|value| predicate(value, parameter));
                included_order.set_weight(&key, if included { 1.0 } else { 0.0 });
                match (was_included, included) {
                    (true, false) => {
                        seq.remove(&key);
                        deltas.push(ListDelta::Remove(key));
                    }
                    (false, true) => {
                        let rank = upstream.index_of(&key).expect("included row exists");
                        relocate.push((rank, key));
                    }
                    (true, true) if batch.flags.get(&key).is_some_and(|flags| flags.value) => {
                        deltas.push(ListDelta::Update(key));
                    }
                    _ => {}
                }
            }

            for key in batch.topology_keys() {
                if let Some(rank) = upstream.index_of(key)
                    && seq.contains(key)
                {
                    relocate.push((rank, key.clone()));
                }
            }
            relocate.sort_unstable_by_key(|(rank, _)| *rank);
            for (rank, key) in relocate.into_iter().rev() {
                let next_included = included_order.index_at_weight(included_order.prefix_weight(rank + 1));
                let anchor = included_order.nth(next_included).and_then(|next| seq.index_of(next)).unwrap_or(seq.len());
                place_before(&mut seq, &key, anchor, &mut deltas);
            }
            output.data_mut(world).seq = seq;
            output.emit(world, deltas)
        });
        output
    }
}

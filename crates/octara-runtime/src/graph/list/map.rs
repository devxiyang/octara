use super::*;
use crate::graph::value::IntoRead;

impl<K: Clone + Eq + Hash + 'static, T: 'static> ReadList<K, T> {
    #[track_caller]
    pub fn map<U: PartialEq + 'static>(self, build: &mut BuildCx<'_>, project: impl Fn(&T) -> U + 'static) -> ReadList<K, U> {
        self.map_parameter(build, Parameter::Constant(()), move |value, ()| project(value))
    }

    #[track_caller]
    pub fn map_with<P: 'static, U: PartialEq + 'static>(
        self,
        build: &mut BuildCx<'_>,
        parameter: impl IntoRead<P>,
        project: impl Fn(&T, &P) -> U + 'static,
    ) -> ReadList<K, U> {
        self.map_parameter(build, Parameter::Signal(parameter.into_read()), project)
    }

    fn map_parameter<P: 'static, U: PartialEq + 'static>(
        self,
        build: &mut BuildCx<'_>,
        parameter: Parameter<P>,
        project: impl Fn(&T, &P) -> U + 'static,
    ) -> ReadList<K, U> {
        let output = build.allocate_list(NodeKind::ListOp, None);
        output.set_keyed_upstream(build.world, self, parameter.id());
        let mut cursor = Cursor::default();
        let mut parameter_revision = None;
        build.world.init_compute(output.id, move |read| {
            let batch = Batch::new(read.list_changes(self, &mut cursor));
            let parameter_changed = parameter.track(read, &mut parameter_revision);
            let world = &mut *read.world;
            let mut seq = std::mem::take(&mut output.data_mut(world).seq);
            let upstream = &self.data(world).seq;
            let keys = if batch.reset || parameter_changed { upstream.to_vec() } else { batch.value_keys().cloned().collect() };
            let parameter = parameter.get(world);
            let rows = keys
                .into_iter()
                .map(|key| {
                    let row = self.value(world, &key).map(|value| project(value, parameter));
                    (key, row)
                })
                .collect::<Vec<_>>();
            let mut deltas = Vec::new();
            if batch.reset {
                let changed = !seq.is_empty() || !upstream.is_empty();
                let updated_order = Seq::from_keys(upstream.to_vec());
                let data = output.data_mut(world);
                data.seq = updated_order;
                data.rows = rows.into_iter().filter_map(|(key, row)| Some((key, row?))).collect();
                return changed && output.emit(world, [ListDelta::Reset]);
            }
            mirror(&mut seq, upstream, &batch, &mut deltas);
            let data = output.data_mut(world);
            data.seq = seq;
            for delta in &deltas {
                if let ListDelta::Remove(key) = delta {
                    data.rows.remove(key);
                }
            }
            for (key, row) in rows {
                if let Some(row) = row {
                    match data.rows.entry(key) {
                        std::collections::hash_map::Entry::Occupied(mut existing) if *existing.get() != row => {
                            deltas.push(ListDelta::Update(existing.key().clone()));
                            existing.insert(row);
                        }
                        std::collections::hash_map::Entry::Vacant(slot) => {
                            slot.insert(row);
                        }
                        _ => {}
                    }
                }
            }
            output.emit(world, deltas)
        });
        output
    }
}

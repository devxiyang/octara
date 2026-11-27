use super::*;

use rustc_hash::FxHashSet;

pub const JOIN_FANOUT_ROWS: usize = 10_000;

struct JoinIndex<K, R> {
    by_left: FxHashMap<K, (R, usize)>,
    by_right: FxHashMap<R, SmallVec<[K; 2]>>,
}

struct JoinState<K, RK, R> {
    right: ReadList<RK, R>,
    index: JoinIndex<K, RK>,
}

impl<K: Clone + Eq + Hash, R: Clone + Eq + Hash> JoinIndex<K, R> {
    fn new() -> Self {
        Self { by_left: FxHashMap::default(), by_right: FxHashMap::default() }
    }

    fn unlink(&mut self, left: &K) {
        if let Some((right, index)) = self.by_left.remove(left) {
            let rows = self.by_right.get_mut(&right).expect("matched reverse index");
            rows.swap_remove(index);
            if let Some(swapped) = rows.get(index) {
                self.by_left.get_mut(swapped).expect("matched forward index").1 = index;
            }
            if rows.is_empty() {
                self.by_right.remove(&right);
            }
        }
    }

    fn link(&mut self, left: &K, right: R) {
        if self.by_left.get(left).is_some_and(|(current, _)| *current == right) {
            return;
        }
        self.unlink(left);
        let rows = self.by_right.entry(right.clone()).or_default();
        self.by_left.insert(left.clone(), (right, rows.len()));
        rows.push(left.clone());
    }
}

impl<K: Clone + Eq + Hash + 'static, T: 'static> ReadList<K, T> {
    /// Lookup-joins with the right list. Left keys and order are preserved when a match is missing.
    #[track_caller]
    pub fn join<RK: Clone + Eq + Hash + 'static, R: 'static, U: PartialEq + 'static>(
        self,
        build: &mut BuildCx<'_>,
        right: impl IntoReadList<RK, R>,
        key: impl Fn(&T) -> RK + 'static,
        project: impl Fn(&T, Option<&R>) -> U + 'static,
    ) -> ReadList<K, U> {
        let output = build.allocate_list(NodeKind::ListOp, None);
        let right = right.into_read_list();
        let join_state = build.world.alloc_value(
            NodeKind::ValueSource,
            build.scope,
            build.window,
            JoinState::<K, RK, R> { right, index: JoinIndex::new() },
        );
        output.set_keyed_upstream(build.world, self, None);
        output.data_mut(build.world).keyed_match = Some(KeyedUpstream {
            id: join_state,
            lookup: |world, id, key| {
                let state = world.value::<JoinState<K, RK, R>>(id);
                let (matched, _) = state.index.by_left.get(key)?;
                let right = state.right;
                let matched = matched.clone();
                Some(right.row_node(world, &matched))
            },
        });
        let mut left_cursor = Cursor::default();
        let mut right_cursor = Cursor::default();
        build.world.init_compute(output.id, move |read| {
            let left_batch = Batch::new(read.list_changes(self, &mut left_cursor));
            let right_batch = Batch::new(read.list_changes(right, &mut right_cursor));
            let world = &mut *read.world;
            let mut index = std::mem::replace(&mut world.value_mut::<JoinState<K, RK, R>>(join_state).index, JoinIndex::new());
            let mut seq = std::mem::take(&mut output.data_mut(world).seq);
            let left_order = &self.data(world).seq;
            let mut deltas = Vec::new();

            let left_changes = if left_batch.reset {
                index = JoinIndex::new();
                left_order.to_vec()
            } else {
                left_batch.value_keys().cloned().collect()
            };
            for left in &left_changes {
                match self.value(world, left) {
                    Some(row) => index.link(left, key(row)),
                    None => index.unlink(left),
                }
            }

            let mut seen = FxHashSet::default();
            let mut affected = Vec::new();
            let mut fanout = 0;
            for left in left_changes {
                seen.insert(left.clone());
                affected.push(left);
            }
            if right_batch.reset && !left_batch.reset {
                affected.extend(left_order.to_vec().into_iter().filter(|left| seen.insert(left.clone())));
            } else if !left_batch.reset {
                for right_key in right_batch.value_keys() {
                    if let Some(matches) = index.by_right.get(right_key) {
                        fanout = fanout.max(matches.len());
                        affected.extend(matches.iter().filter(|left| seen.insert((*left).clone())).cloned());
                    }
                }
            }

            let rows = affected
                .into_iter()
                .filter_map(|left| {
                    let (right_key, _) = index.by_left.get(&left)?;
                    let left_value = self.value(world, &left)?;
                    let value = project(left_value, right.value(world, right_key));
                    Some((left, value))
                })
                .collect::<Vec<_>>();
            let mut seq_is_rebuilt = None;
            if left_batch.reset {
                seq_is_rebuilt = Some((!seq.is_empty() || !left_order.is_empty(), Seq::from_keys(left_order.to_vec())));
            } else {
                mirror(&mut seq, left_order, &left_batch, &mut deltas);
            }
            world.value_mut::<JoinState<K, RK, R>>(join_state).index = index;
            if let Some((changed, rebuilt)) = seq_is_rebuilt {
                let data = output.data_mut(world);
                data.seq = rebuilt;
                data.rows = rows.into_iter().collect();
                return changed && output.emit(world, [ListDelta::Reset]);
            }

            let data = output.data_mut(world);
            data.seq = seq;
            for delta in &deltas {
                if let ListDelta::Remove(key) = delta {
                    data.rows.remove(key);
                }
            }
            for (left, row) in rows {
                match data.rows.entry(left) {
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
            if fanout > JOIN_FANOUT_ROWS {
                world.report(Diagnostic::new(&codes::JOIN_FANOUT, format!("One join key matched {fanout} left rows")));
            }
            output.emit(world, deltas)
        });
        output
    }
}

use super::*;
use crate::graph::delta::DEFAULT_DELTA_LOG_CAP;
use std::cell::Cell;
use std::rc::Rc;

#[test]
fn derived_reads_are_fresh_and_only_changed_key_observers_run() {
    struct Row(i64);
    #[derive(PartialEq)]
    struct Projected(i64);

    let mut world = ReactiveWorld::new();
    let scope = world.root_scope();
    let calls = Rc::new(Cell::new(0));
    let observed = Rc::new(Cell::new(0));
    let (list, mapped, first, second, positive) = {
        let mut build = world.build(scope, None);
        let list = build.list::<u32, Row>();
        let mapped = list.read_only().map(&mut build, {
            let calls = Rc::clone(&calls);
            move |row| {
                calls.set(calls.get() + 1);
                Projected(row.0 * 2)
            }
        });
        let positive = mapped.filter(&mut build, |row| row.0 > 0);
        let first = build.memo(move |read| read.with_row(positive, &1, |row| row.map(|r| r.0)));
        let second = build.memo({
            let observed = Rc::clone(&observed);
            move |read| {
                observed.set(observed.get() + 1);
                read.with_row(positive, &2, |row| row.map(|r| r.0))
            }
        });
        (list, mapped, first, second, positive)
    };

    let mut write = world.write(scope, None);
    for i in 0..200 {
        write.push_row(list, i, Row(i.into()));
    }
    assert_eq!(write.get(first), Some(2));
    assert_eq!(write.get(second), Some(4));
    assert_eq!(calls.get(), 200);
    assert_eq!(observed.get(), 1);

    for i in 1..=100 {
        write.update_row(list, &1, |row| row.0 = -i);
    }
    assert_eq!(calls.get(), 200);
    assert_eq!(write.get(first), None);
    assert_eq!(write.with_row(mapped, &1, |row| row.map(|r| r.0)), Some(-200));
    assert_eq!(write.len(positive), 198);
    assert_eq!(write.get(second), Some(4));
    assert_eq!(calls.get(), 201);
    assert_eq!(observed.get(), 1);

    let copied = list;
    write.update_row(copied, &1, |row| row.0 = 3);
    assert_eq!(write.get(first), Some(6));
    assert_eq!(write.keys(positive).first(), Some(&1));
}

#[test]
fn operators_settle_mixed_batches_in_final_order() {
    use FlatKey::{Header, Item};

    let mut world = ReactiveWorld::new();
    let scope = world.root_scope();
    let (list, right, sorted, filtered, map, grouped, joined, param) = {
        let mut build = world.build(scope, None);
        let list = build.list::<u32, i32>();
        let right = build.list::<i32, i32>();
        let param = build.signal(1);
        let data = list.read_only();
        let sorted = data.sort_by_with(&mut build, param, |row, direction| row * direction);
        let filtered = data.filter_with(&mut build, param, |row, direction| row * direction > 0);
        let mapped = data.map_with(&mut build, param, |row, direction| row * direction);
        let grouped = data.group_by(&mut build, |row| row % 2);
        let joined = data.join(&mut build, right, |row| row % 2, |row, matched| *row + matched.copied().unwrap_or_default());
        (list, right, sorted, filtered, mapped, grouped, joined, param)
    };

    let mut write = world.write(scope, None);
    for (key, value) in [(0, 1), (1, 3), (2, 2), (3, 2)] {
        write.push_row(list, key, value);
    }
    assert_eq!(write.keys(sorted), [0, 2, 3, 1]);
    assert_eq!(write.keys(filtered), [0, 1, 2, 3]);
    assert_eq!(write.keys(map), [0, 1, 2, 3]);
    assert_eq!(write.keys(joined), [0, 1, 2, 3]);
    assert_eq!(write.keys(grouped.flatten()), [Header(0), Item(2), Item(3), Header(1), Item(0), Item(1)]);

    write.move_row(list, &3, 0);
    write.update_row(list, &0, |row| *row = 4);
    write.insert_row(list, 1, 4, -2);
    write.remove_row(list, &1);
    write.move_row(list, &2, 0);
    write.push_row(right, 0, 20);

    assert_eq!(write.keys(list), [2, 3, 4, 0]);
    assert_eq!(write.keys(sorted), [4, 2, 3, 0]);
    assert_eq!(write.keys(filtered), [2, 3, 0]);
    assert_eq!(write.keys(map), [2, 3, 4, 0]);
    assert_eq!(write.keys(joined), [2, 3, 4, 0]);
    assert_eq!(write.keys(grouped.flatten()), [Header(0), Item(2), Item(3), Item(4), Item(0)]);
    assert_eq!(write.get_row(grouped.read_only(), &0).unwrap().size, 4);
    assert_eq!(write.get_row(joined, &0), Some(24));
    assert_eq!(write.get_row(joined, &4), Some(18));

    write.set(param, -1);
    assert_eq!(write.keys(sorted), [0, 2, 3, 4]);
    assert_eq!(write.keys(filtered), [4]);
    assert_eq!(write.get_row(map, &0), Some(-4));
}

#[test]
fn overflow_rebuilds_derived_views_without_losing_authoritative_values() {
    let mut world = ReactiveWorld::new();
    let scope = world.root_scope();
    let (list, mapped) = {
        let mut build = world.build(scope, None);
        let list = build.list();
        let map = list.read_only().map(&mut build, |value: &usize| value * 2);
        (list, map)
    };

    world.write(scope, None).push_row(list, 1, 0);
    assert_eq!(world.read().get_row(mapped, &1), Some(0));
    let mut cursor = world.read().list_cursor(list);
    let mut topology = world.read().list_topology_cursor(list);
    let mut write = world.write(scope, None);
    for _ in 0..=DEFAULT_DELTA_LOG_CAP {
        write.update_row(list, &1, |value| *value += 1);
    }
    assert_eq!(write.get_row(mapped, &1), Some((DEFAULT_DELTA_LOG_CAP + 1) * 2));
    assert_eq!(world.read().list_changes(list, &mut cursor), Changes::Reset);
    assert_eq!(world.read().list_topology_changes(list, &mut topology), Changes::Delta(vec![]));
}

#[test]
fn a_missing_source_key_tracks_only_its_presence() {
    let mut world = ReactiveWorld::new();
    let scope = world.root_scope();
    let calls = Rc::new(Cell::new(0));
    let (list, row) = {
        let mut build = world.build(scope, None);
        let list = build.list();
        let row = build.memo({
            let calls = Rc::clone(&calls);
            move |read| {
                calls.set(calls.get() + 1);
                read.get_row(list, &0)
            }
        });
        (list, row)
    };

    let mut write = world.write(scope, None);
    assert_eq!(write.get(row), None);
    write.push_row(list, 1, "other row");
    assert_eq!(write.get(row), None);
    assert_eq!(calls.get(), 1);
    write.insert_row(list, 0, 0, "present");
    assert_eq!(write.get(row), Some("present"));
    write.move_row(list, &0, 1);
    assert_eq!(write.get(row), Some("present"));
    assert_eq!(calls.get(), 2);
    write.remove_row(list, &0);
    assert_eq!(write.get(row), None);
    assert_eq!(calls.get(), 3);
}

#[test]
fn affected_join_rows_rewire_when_their_projected_value_stays_equal() {
    let mut world = ReactiveWorld::new();
    let scope = world.root_scope();
    let (left, right, joined, first, second) = {
        let mut build = world.build(scope, None);
        let left = build.list::<u32, u32>();
        let right = build.list::<u32, u32>();
        let joined = left.read_only().join(&mut build, right, |row| *row, |_, matched| matched.copied());
        let first = build.memo(move |read| read.get_row(joined, &1).flatten());
        let second = build.memo(move |read| read.get_row(joined, &2).flatten());
        (left, right, joined, first, second)
    };

    let mut write = world.write(scope, None);
    write.push_row(left, 1, 10);
    write.push_row(left, 2, 20);
    write.push_row(right, 10, 0);
    write.push_row(right, 11, 0);
    write.push_row(right, 20, 2);
    assert_eq!(write.get(first), Some(0));
    assert_eq!(write.get(second), Some(2));

    write.update_row(left, &1, |row| *row = 11);
    assert_eq!(write.get(first), Some(0));
    write.update_row(right, &11, |row| *row = 1);
    assert_eq!(write.get(first), Some(1));

    let other_row = joined.data(&world).row_nodes[&2];
    assert_eq!(world.hot(other_row).state, crate::arena::NodeState::Clean);
    world.write(scope, None).update_row(right, &10, |row| *row = 5);
    assert_eq!(world.hot(first.id()).state, crate::arena::NodeState::Clean);
    assert_eq!(world.hot(second.id()).state, crate::arena::NodeState::Clean);
}

#[test]
fn batched_operators_match_vector_reference() {
    use FlatKey::{Header, Item};
    use std::collections::BTreeMap;

    let mut world = ReactiveWorld::new();
    let scope = world.root_scope();
    let (source, filter, sort, map, group, flat_map) = {
        let mut build = world.build(scope, None);
        let source = build.list::<usize, usize>();
        let read = source.read_only();
        let filter = read.filter(&mut build, |value| value.is_multiple_of(2));
        let sort = read.sort_by(&mut build, |value| *value);
        let map = filter.map(&mut build, |value| *value * 2);
        let group = read.group_by(&mut build, |value| value % 3);
        let flat_map = group.flatten().map(&mut build, |value| value.header().map(|info| info.size));
        (source, filter, sort, map, group, flat_map)
    };

    let mut rows = Vec::<(usize, usize)>::new();
    let mut random = 42_u64;
    let mut pick = |n: usize| {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        random as usize % n.max(1)
    };
    let mut write = world.write(scope, None);
    for i in 0..3_500 {
        let index = pick(rows.len());
        match pick(5) {
            0 | 1 => {
                let value = pick(10);
                let at = pick(rows.len() + 1);
                rows.insert(at, (i, value));
                write.insert_row(source, at, i, value);
            }
            2 if !rows.is_empty() => {
                let (key, value) = rows.remove(index);
                assert_eq!(write.remove_row(source, &key), Some(value));
            }
            3 if !rows.is_empty() => {
                let old = rows[index].1;
                let value = if pick(2) == 0 { old + 3 } else { (old + 1) % 10 };
                rows[index].1 = value;
                write.update_row(source, &rows[index].0, |row| *row = value);
            }
            4 if !rows.is_empty() => {
                let to = pick(rows.len());
                let row = rows.remove(index);
                write.move_row(source, &row.0, to);
                rows.insert(to, row);
            }
            _ => {}
        }
        if i % 7 != 6 {
            continue;
        }

        let even = rows.iter().filter(|(_, value)| value.is_multiple_of(2)).map(|&(key, _)| key).collect::<Vec<_>>();
        assert_eq!(write.keys(filter), even, "filter at {i}");
        assert_eq!(write.keys(map), even, "map at {i}");
        let mut sorted = rows.clone();
        sorted.sort_unstable_by_key(|&(key, value)| (value, key));
        assert_eq!(write.keys(sort), sorted.into_iter().map(|(key, _)| key).collect::<Vec<_>>(), "sort at {i}");

        let mut groups = BTreeMap::<_, Vec<_>>::new();
        for &(key, value) in &rows {
            groups.entry(value % 3).or_default().push(key);
        }
        let flat = groups
            .iter()
            .flat_map(|(&group, keys)| std::iter::once(Header(group)).chain(keys.iter().copied().map(Item)))
            .collect::<Vec<_>>();
        assert_eq!(write.keys(group.flatten()), flat, "flatten at {i}");
        assert_eq!(write.keys(flat_map), flat, "flatten projection at {i}");
        for (key, rows) in groups {
            assert_eq!(write.get_row(flat_map, &Header(key)), Some(Some(rows.len())));
        }
    }
}

#[test]
fn derived_list_publishes_keyed_deltas_and_keeps_equal_rows() {
    #[derive(PartialEq)]
    struct Entry(u32);

    let mut world = crate::ReactiveWorld::new();
    let scope = world.root_scope();
    let mut build = world.build(scope, None);
    let changed = build.signal(false);
    let rows = build.derive_list(move |read| {
        if read.get(changed) { vec![(2, Entry(20)), (1, Entry(1)), (3, Entry(3))] } else { vec![(1, Entry(1)), (2, Entry(2))] }
    });
    let value = build.memo(move |read| read.with_row(rows, &1, |row| row.unwrap().0));

    let before = world.live_nodes();
    assert_eq!(world.build(scope, None).read(|r| r.keys(rows)), [1, 2]);
    assert_eq!(world.live_nodes(), before);
    assert_eq!(world.read().get(value), 1);
    let mut cursor = world.read().list_cursor(rows);

    world.write(scope, None).set(changed, true);
    let mut read = world.read();
    assert_eq!(read.keys(rows), [2, 1, 3]);
    assert_eq!(read.get(value), 1);
    assert_eq!(
        read.list_changes(rows, &mut cursor),
        Changes::Delta(vec![ListDelta::Move { key: 2, to: 0 }, ListDelta::Update(2), ListDelta::Insert(3)])
    );
    assert!(world.validate_integrity());
}

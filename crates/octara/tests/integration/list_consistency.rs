//! A collection write becomes visible through every branch before a join evaluates it.
use octara::platform::HeadlessPlatform;
use octara::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq)]
struct Row {
    id: u32,
    revision: u32,
    selected: bool,
}

fn snapshot<T: Clone + 'static>(list: &ReadList<u32, T>) -> Vec<(u32, T)> {
    list.keys().into_iter().map(|key| (key, list.item(&key).unwrap().get())).collect()
}

#[test]
fn same_source_map_branches_rejoin_without_mixed_item_versions() {
    for right_first in [false, true] {
        let app = App::new(HeadlessPlatform::default());
        let left_calls = Rc::new(Cell::new(0));
        let right_calls = Rc::new(Cell::new(0));
        let observations = Rc::new(RefCell::new(Vec::new()));
        let (source, joined, cached) = app.run(|| {
            let source = state_list("rows", || (0..128).map(|id| Row { id, revision: 0, selected: true }).collect(), |row| row.id);
            let map = |calls: Rc<Cell<usize>>| {
                source.read_only().map(move |row| {
                    calls.set(calls.get() + 1);
                    (row.id, row.revision)
                })
            };
            let (left, right) = if right_first {
                let right = map(right_calls.clone());
                (map(left_calls.clone()), right)
            } else {
                let left = map(left_calls.clone());
                (left, map(right_calls.clone()))
            };
            let observed = observations.clone();
            let joined = left.join(
                right,
                |row| row.0,
                move |left, right| {
                    let pair = (left.1, right.map(|row| row.1));
                    observed.borrow_mut().push(pair);
                    pair
                },
            );
            let cached_list = joined.clone();
            let cached = memo(move || cached_list.item(&7).unwrap().get());
            (source, joined, cached)
        });
        observations.borrow_mut().clear();
        assert_eq!(app.run(|| cached.get()), (0, Some(0)));

        app.run(|| {
            for revision in 1..=3 {
                observations.borrow_mut().clear();
                let before = (left_calls.get(), right_calls.get());
                source.update(&7, |row| row.revision = revision);
                assert_eq!(joined.item(&7).unwrap().get(), (revision, Some(revision)));
                assert_eq!(cached.get(), (revision, Some(revision)), "a cached item must refresh before the write returns");
                assert_eq!(left_calls.get(), before.0 + 1, "only the changed left item is projected");
                assert_eq!(right_calls.get(), before.1 + 1, "only the changed right item is projected");
                let seen = observations.borrow();
                assert!(!seen.is_empty(), "the changed join row must be projected");
                assert!(seen.len() <= 2, "a write must not rebuild all join rows: {seen:?}");
                assert!(
                    seen.iter().all(|pair| *pair == (revision, Some(revision))),
                    "one source write must not project mixed branch versions (right_first={right_first}): {seen:?}"
                );
            }
        });
        assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
        app.shutdown();
    }
}

#[test]
fn filtered_diamonds_do_not_project_transient_membership_or_versions() {
    for right_first in [false, true] {
        let app = App::new(HeadlessPlatform::default());
        let calls = Rc::new([const { Cell::new(0usize) }; 4]);
        let observations = Rc::new(RefCell::new(Vec::new()));
        let (source, left, right, joined, cached) = app.run(|| {
            let source = state_list("rows", || (0..128).map(|id| Row { id, revision: 0, selected: true }).collect(), |row| row.id);
            let make_left = || {
                let counted_filter = calls.clone();
                let counted_map = calls.clone();
                source
                    .read_only()
                    .filter(move |row| {
                        counted_filter[0].set(counted_filter[0].get() + 1);
                        row.selected
                    })
                    .map(move |row| {
                        counted_map[1].set(counted_map[1].get() + 1);
                        (row.id, row.revision)
                    })
            };
            let make_right = || {
                let counted_map = calls.clone();
                let counted_filter = calls.clone();
                source
                    .read_only()
                    .map(move |row| {
                        counted_map[2].set(counted_map[2].get() + 1);
                        row.clone()
                    })
                    .filter(move |row| {
                        counted_filter[3].set(counted_filter[3].get() + 1);
                        row.selected
                    })
                    .map(|row| (row.id, row.revision))
            };
            let (left, right) = if right_first {
                let right = make_right();
                (make_left(), right)
            } else {
                let left = make_left();
                (left, make_right())
            };
            let observed = observations.clone();
            let joined = left.join(
                &right,
                |row| row.0,
                move |left, right| {
                    let pair = (left.1, right.map(|row| row.1));
                    observed.borrow_mut().push(pair);
                    pair
                },
            );
            let cached_source = source.read_only();
            let cached_join = joined.clone();
            let cached = memo(move || (snapshot(&cached_source), snapshot(&cached_join)));
            (source, left, right, joined, cached)
        });
        app.run(|| {
            for step in 0..6 {
                observations.borrow_mut().clear();
                let before = calls.each_ref().map(Cell::get);
                match step {
                    0 => source.update(&7, |row| {
                        row.revision = 1;
                        row.selected = false;
                    }),
                    1 => source.update(&7, |row| {
                        row.revision = 2;
                        row.selected = true;
                    }),
                    2 => source.remove(&7),
                    3 => source.insert(4, Row { id: 7, revision: 3, selected: true }),
                    4 => source.update(&7, |row| row.revision = 4),
                    5 => source.update(&7, |row| {
                        row.revision = 5;
                        row.selected = false;
                    }),
                    _ => unreachable!(),
                }
                let all = snapshot(&source.read_only());
                let expected: Vec<_> = all
                    .iter()
                    .filter(|(_, row)| row.selected)
                    .map(|(key, row)| (*key, (row.revision, Some(row.revision))))
                    .collect();
                assert_eq!(snapshot(&joined), expected, "direct join reads after step {step}");
                assert_eq!(cached.get(), (all, expected), "a memo combining source and derived snapshots must refresh synchronously");
                assert_eq!(left.keys(), right.keys(), "filter membership must agree after each write");
                for (index, count) in calls.iter().enumerate() {
                    assert!(count.get() <= before[index] + 1, "step {step} rebuilt operator {index} over unchanged rows");
                }
                let seen = observations.borrow();
                if let Some(row) = source.item(&7).map(|item| item.get()).filter(|row| row.selected) {
                    assert!(!seen.is_empty(), "a changed visible row must be projected at step {step}");
                    assert!(seen.len() <= 2, "a write must not rebuild unchanged join rows: {seen:?}");
                    assert!(
                        seen.iter().all(|pair| *pair == (row.revision, Some(row.revision))),
                        "filtered branches must reach the same source version (step={step}, right_first={right_first}): {seen:?}"
                    );
                } else {
                    assert!(seen.is_empty(), "removing both sides must not project a transient missing partner (step={step}, right_first={right_first}): {seen:?}");
                }
            }
        });
        assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
        app.shutdown();
    }
}

#[test]
fn a_filtered_right_branch_projects_final_missing_and_restored_matches() {
    for right_first in [false, true] {
        let app = App::new(HeadlessPlatform::default());
        let observations = Rc::new(RefCell::new(Vec::new()));
        let (source, joined, cached) = app.run(|| {
            let source = state_list("rows", || vec![Row { id: 7, revision: 0, selected: true }], |row| row.id);
            let make_left = || source.read_only().map(Clone::clone);
            let make_right = || source.read_only().filter(|row| row.selected).map(|row| row.revision);
            let (left, right) = if right_first {
                let right = make_right();
                (make_left(), right)
            } else {
                let left = make_left();
                (left, make_right())
            };
            let observed = observations.clone();
            let joined = left.join(
                right,
                |row| row.id,
                move |left, right| {
                    let value = (left.revision, right.copied());
                    observed.borrow_mut().push(value);
                    value
                },
            );
            let cached_list = joined.clone();
            let cached = memo(move || snapshot(&cached_list));
            (source, joined, cached)
        });
        app.run(|| {
            for revision in 1..=4 {
                observations.borrow_mut().clear();
                let selected = revision % 2 == 0;
                source.update(&7, |row| {
                    row.revision = revision;
                    row.selected = selected;
                });
                let expected = (revision, selected.then_some(revision));
                assert_eq!(snapshot(&joined), vec![(7, expected)]);
                assert_eq!(cached.get(), vec![(7, expected)], "a missing right item keeps the left row live");
                let seen = observations.borrow();
                assert!(!seen.is_empty() && seen.len() <= 2, "only the changed left row is projected: {seen:?}");
                assert!(
                    seen.iter().all(|value| *value == expected),
                    "a filtered right branch must have its final membership before projection (right_first={right_first}): {seen:?}"
                );
            }
        });
        assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
        app.shutdown();
    }
}

#[test]
fn a_lookup_key_change_cannot_pair_the_new_left_item_with_its_old_right_key() {
    for right_first in [false, true] {
        let app = App::new(HeadlessPlatform::default());
        let observations = Rc::new(RefCell::new(Vec::new()));
        let calls = Rc::new([const { Cell::new(0usize) }; 2]);
        let (source, joined, cached) = app.run(|| {
            let source = state_list("rows", || (0..128).map(|id| Row { id, revision: 0, selected: true }).collect(), |row| row.id);
            let make_left = || {
                let counted = calls.clone();
                source.read_only().map(move |row| {
                    counted[0].set(counted[0].get() + 1);
                    (row.id, if row.selected { row.id } else { row.id + 1 }, row.revision)
                })
            };
            let make_right = || {
                let counted = calls.clone();
                source.read_only().map(move |row| {
                    counted[1].set(counted[1].get() + 1);
                    (row.id, row.revision)
                })
            };
            let (left, right) = if right_first {
                let right = make_right();
                (make_left(), right)
            } else {
                let left = make_left();
                (left, make_right())
            };
            let observed = observations.clone();
            let joined = left.join(
                right,
                |row| row.1,
                move |left, right| {
                    let value = (*left, right.copied());
                    observed.borrow_mut().push(value);
                    value
                },
            );
            let cached_list = joined.clone();
            let cached = memo(move || snapshot(&cached_list));
            (source, joined, cached)
        });
        app.run(|| {
            for revision in 1..=4 {
                observations.borrow_mut().clear();
                let before = calls.each_ref().map(Cell::get);
                let selected = revision % 2 == 0;
                source.update(&7, |row| {
                    row.revision = revision;
                    row.selected = selected;
                });
                let expected = if selected { ((7, 7, revision), Some((7, revision))) } else { ((7, 8, revision), Some((8, 0))) };
                assert_eq!(joined.item(&7).unwrap().get(), expected);
                assert_eq!(cached.get(), snapshot(&joined), "the cached join must follow the reassigned lookup immediately");
                assert_eq!(calls.each_ref().map(Cell::get), before.map(|count| count + 1), "one changed item per map");
                let seen = observations.borrow();
                assert!(!seen.is_empty() && seen.len() <= 2, "only the reassigned row is projected: {seen:?}");
                assert!(
                    seen.iter().all(|value| *value == expected),
                    "an old right-key notification must not project the left item's new lookup (right_first={right_first}): {seen:?}"
                );
            }
        });
        assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
        app.shutdown();
    }
}

#[test]
fn chained_joins_wait_for_deeper_inputs_before_delivering_a_direct_source_change() {
    for right_first in [false, true] {
        let app = App::new(HeadlessPlatform::default());
        let first_observations = Rc::new(RefCell::new(Vec::new()));
        let final_observations = Rc::new(RefCell::new(Vec::new()));
        let (source, joined, cached) = app.run(|| {
            let source = state_list("rows", || (0..128).map(|id| Row { id, revision: 0, selected: true }).collect(), |row| row.id);
            let branch = || source.read_only().map(|row| (row.id, row.revision));
            let (left, right) = if right_first {
                let right = branch();
                (branch(), right)
            } else {
                let left = branch();
                (left, branch())
            };
            let observed_first = first_observations.clone();
            let first = left.join(
                right,
                |row| row.0,
                move |left, right| {
                    let value = (left.0, left.1, right.map(|row| row.1));
                    observed_first.borrow_mut().push(value);
                    value
                },
            );
            let observed_final = final_observations.clone();
            let joined = first.join(
                source,
                |row| row.0,
                move |left, right| {
                    let value = (left.0, left.1, left.2, right.map(|row| row.revision));
                    observed_final.borrow_mut().push(value);
                    value
                },
            );
            let cached_list = joined.clone();
            let cached = memo(move || snapshot(&cached_list));
            (source, joined, cached)
        });
        app.run(|| {
            for revision in 1..=3 {
                first_observations.borrow_mut().clear();
                final_observations.borrow_mut().clear();
                source.update(&7, |row| row.revision = revision);
                let expected = (7, revision, Some(revision), Some(revision));
                assert_eq!(joined.item(&7).unwrap().get(), expected);
                assert_eq!(cached.get(), snapshot(&joined), "a memo after both joins must refresh before the write returns");
                let first_seen = first_observations.borrow();
                assert!(!first_seen.is_empty() && first_seen.len() <= 2, "the first join stays incremental: {first_seen:?}");
                assert!(first_seen.iter().all(|value| *value == (7, revision, Some(revision))), "first join: {first_seen:?}");
                let final_seen = final_observations.borrow();
                assert!(!final_seen.is_empty() && final_seen.len() <= 3, "the final join stays incremental: {final_seen:?}");
                assert!(
                    final_seen.iter().all(|value| *value == expected),
                    "the direct source event must wait for the deeper join input (right_first={right_first}): {final_seen:?}"
                );
            }
        });
        assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
        app.shutdown();
    }
}

#[test]
fn creating_a_group_joins_current_counts_instead_of_intermediate_insert_payloads() {
    for right_first in [false, true] {
        let app = App::new(HeadlessPlatform::default());
        let observations = Rc::new(RefCell::new(Vec::new()));
        let calls = Rc::new([const { Cell::new(0usize) }; 2]);
        let (source, joined, cached) = app.run(|| {
            let source = state_list("rows", Vec::<u32>::new, |id| *id);
            let groups = source.read_only().group_by(|_| 0u32).read_only();
            let branch = |index: usize| {
                let counted = calls.clone();
                groups.map(move |group| {
                    counted[index].set(counted[index].get() + 1);
                    (group.key, group.len)
                })
            };
            let (left, right) = if right_first {
                let right = branch(1);
                (branch(0), right)
            } else {
                let left = branch(0);
                (left, branch(1))
            };
            let observed = observations.clone();
            let joined = left.join(
                right,
                |group| group.0,
                move |left, right| {
                    let value = (left.1, right.map(|group| group.1));
                    observed.borrow_mut().push(value);
                    value
                },
            );
            let cached_list = joined.clone();
            let cached = memo(move || snapshot(&cached_list));
            (source, joined, cached)
        });
        app.run(|| {
            assert!(cached.get().is_empty());
            for id in 1..=2 {
                observations.borrow_mut().clear();
                let before = calls.each_ref().map(Cell::get);
                source.push(id);
                assert_eq!(snapshot(&joined), vec![(0, (1, Some(1)))]);
                assert_eq!(cached.get(), vec![(0, (1, Some(1)))], "the new group's count must be visible before push returns");
                for (index, count) in calls.iter().enumerate() {
                    assert!(count.get() <= before[index] + 2, "only the new group's insert and count update reach each branch");
                }
                let seen = observations.borrow();
                assert!(!seen.is_empty() && seen.len() <= 4, "only the created group is projected: {seen:?}");
                assert!(
                    seen.iter().all(|value| *value == (1, Some(1))),
                    "a group's intermediate zero-count insertion must not be joined with its current count (right_first={right_first}): {seen:?}"
                );
                drop(seen);
                observations.borrow_mut().clear();
                source.remove(&id);
                assert!(joined.is_empty());
                assert!(cached.get().is_empty());
                assert!(observations.borrow().is_empty(), "retiring the last group member must not project a removed group");
            }
        });
        assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
        app.shutdown();
    }
}

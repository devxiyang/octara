//! List composition checks both runtimes before allocating or calling application code.
use octara::platform::HeadlessPlatform;
use octara::prelude::*;
use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

const OPERATORS: [&str; 5] = ["filter", "map", "sort", "groups", "join"];

fn items(app: &App, initial: Vec<u32>) -> (StateList<u32, u32>, ReadList<u32, u32>) {
    app.run(|| {
        let items = state_list("items", || initial, |n| *n);
        (items, items.read_only())
    })
}

fn combine(operator: &str, left: &ReadList<u32, u32>, right: &ReadList<u32, u32>, calls: Rc<Cell<usize>>) {
    let key_calls = calls.clone();
    let observe = move |n: &u32| {
        key_calls.set(key_calls.get() + 1);
        *n
    };
    match operator {
        "filter" => {
            left.filter(move |n| observe(n) > 0);
        }
        "map" => {
            left.map(observe);
        }
        "sort" => {
            left.sort_by(observe);
        }
        "groups" => {
            left.group_by(observe);
        }
        "join" => {
            left.join(right, observe, move |left, right| {
                calls.set(calls.get() + 1);
                left + right.copied().unwrap_or_default()
            });
        }
        _ => unreachable!(),
    }
}

fn rejected(app: &App, operator: &str, left: &ReadList<u32, u32>, right: &ReadList<u32, u32>) {
    let before = app.work().reactive_nodes;
    let calls = Rc::new(Cell::new(0));
    let result = catch_unwind(AssertUnwindSafe(|| app.run(|| combine(operator, left, right, calls.clone()))));
    assert!(result.is_err(), "{operator}: a foreign list must be rejected even when empty");
    assert_eq!(calls.get(), 0, "{operator}: reject before invoking key/predicate/projection callbacks");
    assert_eq!(app.work().reactive_nodes, before, "{operator}: reject before allocating nodes");
    assert!(
        app.diagnostics()
            .iter()
            .any(|d| { d.code.id == "WRONG_RUNTIME" && d.location.as_deref().is_some_and(|at| at.contains("list_runtime.rs:")) }),
        "the diagnostic must point to the composition call"
    );
}

#[test]
fn foreign_lists_are_rejected_independently_of_their_contents() {
    for operator in OPERATORS {
        for initial in [vec![], vec![1]] {
            let owner = App::new(HeadlessPlatform::default());
            let (state, foreign) = items(&owner, initial);
            let app = App::new(HeadlessPlatform::default());
            let (_, local) = items(&app, vec![1]);
            let owner_nodes = owner.work().reactive_nodes;
            rejected(&app, operator, &foreign, &local);
            assert_eq!(owner.work().reactive_nodes, owner_nodes);
            // A refused composition must leave no observer that later runs in the wrong App.
            owner.run(|| state.push(2));
            assert_eq!(owner.run(|| foreign.keys().last().copied()), Some(2));
            assert_eq!(app.run(|| local.keys()), vec![1]);
            assert!(owner.diagnostics().is_empty());
            app.shutdown();
            owner.shutdown();
        }
    }
}

#[test]
fn join_validates_the_right_list_before_callbacks_even_without_a_match() {
    for left_items in [vec![], vec![1]] {
        for right_items in [vec![], vec![2]] {
            let app = App::new(HeadlessPlatform::default());
            let (left_state, left) = items(&app, left_items.clone());
            let owner = App::new(HeadlessPlatform::default());
            let (right_state, right) = items(&owner, right_items);
            let owner_nodes = owner.work().reactive_nodes;
            rejected(&app, "join", &left, &right);
            assert_eq!(owner.work().reactive_nodes, owner_nodes);
            app.run(|| left_state.push(3));
            owner.run(|| right_state.push(3));
            assert_eq!(app.run(|| left.keys().last().copied()), Some(3));
            assert_eq!(owner.run(|| right.keys().last().copied()), Some(3));
            assert!(owner.diagnostics().is_empty());
            app.shutdown();
            owner.shutdown();
        }
    }
}

#[test]
fn a_retained_list_cannot_build_in_a_new_app_after_its_runtime_exits() {
    for operator in OPERATORS {
        let owner = App::new(HeadlessPlatform::default());
        let (_, retained) = items(&owner, vec![]);
        owner.shutdown();
        drop(owner);
        let app = App::new(HeadlessPlatform::default());
        let (_, local) = items(&app, vec![]);
        rejected(&app, operator, &retained, &local);
        rejected(&app, "join", &local, &retained);
        app.shutdown();
    }
}

#[test]
fn list_changes_update_the_stage_without_rebuilding_the_composing_memo() {
    let app = App::new(HeadlessPlatform::default());
    let builds = Rc::new(Cell::new(0));
    let counted = builds.clone();
    let (left, right, selected) = app.run(|| {
        let left = state_list("left", Vec::<u32>::new, |n| *n);
        let right = state_list("right", Vec::<u32>::new, |n| *n);
        let selected = memo(move || {
            counted.set(counted.get() + 1);
            left.filter(|n| n % 2 == 1).join(right, |n| *n, |n, r| if r.is_some() { n * 10 } else { *n }).read_only()
        });
        (left, right, selected)
    });
    let before = app.run(|| selected.get());
    assert!(app.run(|| before.is_empty()));
    app.run(|| {
        left.push(1);
        left.push(2);
        right.push(1);
    });
    let after = app.run(|| selected.get());
    assert!(before == after, "validation must not turn list changes into memo dependencies");
    assert_eq!(builds.get(), 1);
    assert_eq!(app.run(|| after.keys()), vec![1]);
    assert_eq!(app.run(|| after.item(&1).unwrap().get()), 10);
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
    app.shutdown();
}

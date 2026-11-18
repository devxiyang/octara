//! A retained read-only list releases its nodes in the App that created it.
use octara::platform::HeadlessPlatform;
use octara::prelude::*;
use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

fn items() -> StateList<u32, u32> {
    state_list("items", || vec![1, 2, 3], |item| *item)
}

fn check_release<K: 'static, T: 'static>(name: &str, foreign: bool, make: impl Fn() -> ReadList<K, T> + 'static) {
    let app = App::new(HeadlessPlatform::default());
    let before = app.work().reactive_nodes;
    let held = Rc::new(RefCell::new(None));
    let saved = held.clone();
    let window = app
        .open_window(WindowOptions::default(), move || {
            *saved.borrow_mut() = Some(make());
            text("owner")
        })
        .unwrap();
    let list = held.borrow_mut().take().unwrap();
    let last = list.clone();
    app.close_window(window.id());
    let retained = app.work().reactive_nodes;
    assert!(retained > before, "{name}: the retained list still owns its nodes");
    drop(list);
    assert_eq!(app.work().reactive_nodes, retained, "{name}: a clone still owns the list");
    if foreign {
        let other = App::new(HeadlessPlatform::default());
        let value = other.run(|| state("other", || 42));
        let other_nodes = other.work().reactive_nodes;
        other.run(|| drop(last));
        assert_eq!(other.work().reactive_nodes, other_nodes, "{name}: never dispose a foreign node");
        assert_eq!(other.run(|| value.get()), 42);
        other.shutdown();
    } else {
        drop(last);
    }
    assert_eq!(app.work().reactive_nodes, before, "{name}: final release must reclaim every retained node");
    app.shutdown();
}

fn check_all_stages(foreign: bool) {
    check_release("state", foreign, || items().read_only());
    check_release("filter", foreign, || items().read_only().filter(|n| n % 2 == 1));
    check_release("sort", foreign, || items().read_only().sort_by(|n| std::cmp::Reverse(*n)));
    check_release("map", foreign, || items().read_only().map(|n| n * 2));
    check_release("join", foreign, || {
        let right = state_list("right", || vec![1u32, 3], |n| *n);
        items().read_only().join(right, |n| *n, |left, right| left + right.copied().unwrap_or_default())
    });
    check_release("groups", foreign, || items().read_only().group_by(|n| n % 2).read_only());
    check_release("members", foreign, || items().read_only().group_by(|n| n % 2).members(&1).unwrap());
    check_release("flatten", foreign, || items().read_only().group_by(|n| n % 2).flatten());
    check_release("text items", foreign, || state_list("items", || vec![(1u32, TextBuffer::new("message"))], |item| item.0).read_only());
}

#[test]
fn retained_lists_release_all_nodes_outside_an_app_entry() {
    check_all_stages(false);
}

#[test]
fn retained_lists_release_all_nodes_inside_a_foreign_app() {
    check_all_stages(true);
}

#[test]
fn shutdown_releases_items_even_with_retained_list_and_app_handles() {
    let app = App::new(HeadlessPlatform::default());
    let payload = Rc::new(());
    let alive = Rc::downgrade(&payload);
    let list = app.run(|| state_list("items", || vec![(1u32, payload)], |item| item.0).read_only());
    app.shutdown();
    assert!(alive.upgrade().is_none(), "final App teardown releases graph values even when a list retains their handles");
    drop(app);
    assert!(alive.upgrade().is_none(), "retaining a list must not retain its runtime or graph values");
    drop(list); // Safe even after the creating runtime has gone away.
}

fn check_retirement<K, T>(make: impl Fn(StateList<u32, u32>, StateList<u32, u32>, Rc<Cell<usize>>) -> ReadList<K, T> + 'static)
where
    K: Eq + std::hash::Hash + Clone + 'static,
    T: Clone + 'static,
{
    let app = App::new(HeadlessPlatform::default());
    let (left, right) = app.run(|| (items(), state_list("right", || vec![1u32, 2, 3], |n| *n)));
    let calls = Rc::new(Cell::new(0));
    let observed = calls.clone();
    let held = Rc::new(RefCell::new(None));
    let saved = held.clone();
    let window = app
        .open_window(WindowOptions::default(), move || {
            *saved.borrow_mut() = Some(make(left, right, observed.clone()));
            text("producer")
        })
        .unwrap();
    let list = held.borrow_mut().take().unwrap();
    let key = app.run(|| list.keys().into_iter().next().unwrap());
    let initial_calls = calls.get();
    assert!(initial_calls > 0);
    app.close_window(window.id());
    app.run(|| {
        left.push(4);
        left.remove(&1);
        right.update(&2, |n| *n = 20);
        right.remove(&3);
    });
    assert_eq!(calls.get(), initial_calls, "a retained cache cannot keep its producer running");
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
    for read in [0, 1, 2] {
        let result = catch_unwind(AssertUnwindSafe(|| {
            app.run(|| match read {
                0 => {
                    list.keys();
                }
                1 => {
                    list.item(&key);
                }
                _ => {
                    list.filter(|_| panic!("a retired list must reject composition before callbacks"));
                }
            })
        }));
        assert!(result.is_err(), "retired producer reads and composition must be rejected");
    }
    assert!(app.diagnostics().iter().any(|d| d.code == "READ_DISPOSED"));
    drop(list);
    app.shutdown();
}

#[test]
fn retained_producers_retire_while_their_app_sources_remain_live() {
    let counted = |calls: Rc<Cell<usize>>| {
        move |n: &u32| {
            calls.set(calls.get() + 1);
            *n
        }
    };
    check_retirement(move |left, _, calls| left.read_only().filter(move |n| counted(calls.clone())(n) % 2 == 1));
    check_retirement(move |left, _, calls| left.read_only().sort_by(counted(calls)));
    check_retirement(move |left, _, calls| left.read_only().map(counted(calls)));
    check_retirement(move |left, right, calls| {
        left.read_only().join(right, counted(calls), |left, right| left + right.copied().unwrap_or_default())
    });
    check_retirement(move |left, _, calls| left.read_only().group_by(move |n| counted(calls.clone())(n) % 2).read_only());
    check_retirement(move |left, _, calls| left.read_only().group_by(move |n| counted(calls.clone())(n) % 2).members(&1).unwrap());
    check_retirement(move |left, _, calls| left.read_only().group_by(move |n| counted(calls.clone())(n) % 2).flatten());
}

#[test]
fn dropping_the_last_read_handle_keeps_a_producer_until_its_scope_retires() {
    let app = App::new(HeadlessPlatform::default());
    let left = app.run(items);
    let calls = Rc::new(Cell::new(0));
    let observed = calls.clone();
    let window = app
        .open_window(WindowOptions::default(), move || {
            let observed = observed.clone();
            drop(left.read_only().map(move |n| {
                observed.set(observed.get() + 1);
                *n
            }));
            text("producer")
        })
        .unwrap();
    let initial = calls.get();
    app.run(|| left.push(4));
    assert_eq!(calls.get(), initial + 1, "execution is owned by the scope");
    app.close_window(window.id());
    app.run(|| left.push(5));
    assert_eq!(calls.get(), initial + 1);
    assert!(app.diagnostics().is_empty());
    app.shutdown();
}

#[test]
fn a_join_cannot_reenter_a_retired_upstream_from_its_live_right_side() {
    let app = App::new(HeadlessPlatform::default());
    let (left, right) = app.run(|| (items(), state_list("right", || vec![1u32, 2, 3], |n| *n)));
    let held = Rc::new(RefCell::new(None));
    let saved = held.clone();
    let window = app
        .open_window(WindowOptions::default(), move || {
            *saved.borrow_mut() = Some(left.read_only().filter(|_| true));
            text("upstream")
        })
        .unwrap();
    let upstream = held.borrow_mut().take().unwrap();
    let calls = Rc::new(Cell::new(0));
    let observed = calls.clone();
    let joined = app.run(|| {
        upstream.join(
            right,
            |n| *n,
            move |left, right| {
                observed.set(observed.get() + 1);
                left + right.copied().unwrap_or_default()
            },
        )
    });
    let initial = calls.get();
    app.close_window(window.id());
    app.run(|| right.update(&1, |n| *n = 10));
    assert_eq!(calls.get(), initial, "a still-live dependent must respect upstream retirement");
    assert!(app.diagnostics().is_empty());
    assert!(catch_unwind(AssertUnwindSafe(|| app.run(|| joined.keys()))).is_err());
    assert!(app.diagnostics().iter().any(|d| d.code == "READ_DISPOSED"));
    app.shutdown();
}

#[test]
fn retiring_a_producer_invalidates_cached_item_reads_in_another_scope() {
    let app = App::new(HeadlessPlatform::default());
    let source = app.run(items);
    let held = Rc::new(RefCell::new(None));
    let saved = held.clone();
    let window = app
        .open_window(WindowOptions::default(), move || {
            *saved.borrow_mut() = Some(source.read_only().filter(|_| true));
            text("producer")
        })
        .unwrap();
    let list = held.borrow_mut().take().unwrap();
    let cached = app.run(|| memo(move || list.item(&1).unwrap().get()));
    assert_eq!(app.run(|| cached.get()), 1);
    app.close_window(window.id());
    assert!(catch_unwind(AssertUnwindSafe(|| app.run(|| cached.get()))).is_err());
    assert!(app.diagnostics().iter().any(|d| d.code == "READ_DISPOSED"));
    app.shutdown();
}

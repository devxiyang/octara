//! A retained read-only list releases its nodes in the App that created it.
use octara::platform::HeadlessPlatform;
use octara::prelude::*;
use std::cell::RefCell;
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
            text("owner").into()
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
    check_release("filter", foreign, || items().filter(|n| n % 2 == 1).read_only());
    check_release("sort", foreign, || items().sort_by(|n| std::cmp::Reverse(*n)).read_only());
    check_release("map", foreign, || items().map(|n| n * 2).read_only());
    check_release("join", foreign, || {
        let right = state_list("right", || vec![1u32, 3], |n| *n);
        items().join(right, |n| *n, |left, right| left + right.copied().unwrap_or_default()).read_only()
    });
    check_release("groups", foreign, || items().group_by(|n| n % 2).read_only());
    check_release("members", foreign, || items().group_by(|n| n % 2).members(&1).unwrap());
    check_release("flatten", foreign, || items().group_by(|n| n % 2).flatten().read_only());
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
fn a_retained_list_does_not_keep_its_runtime_alive() {
    let app = App::new(HeadlessPlatform::default());
    let payload = Rc::new(());
    let alive = Rc::downgrade(&payload);
    let list = app.run(|| state_list("items", || vec![(1u32, payload)], |item| item.0).read_only());
    app.shutdown();
    assert!(alive.upgrade().is_some(), "the retained list still owns its item node");
    drop(app);
    assert!(alive.upgrade().is_none(), "retaining a list must not retain its runtime or graph values");
    drop(list); // Safe even after the creating runtime has gone away.
}

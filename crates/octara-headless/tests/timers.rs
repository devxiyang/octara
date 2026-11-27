use octara::{TargetGuard, Tree, prelude::*};
use octara_headless::toast::{ToastOptions, ToastQueue};
use octara_inspect::HeadlessDriver;
use std::time::Duration;

fn tree(driver: &HeadlessDriver, window: WindowId) -> &Tree {
    driver.app().runtime().window(window).unwrap().tree()
}

fn target(driver: &HeadlessDriver, window: WindowId, name: &str) -> TargetGuard {
    let tree = tree(driver, window);
    let [node] = tree.test_ids(name) else { panic!("expected a single node for {name}") };
    tree.guard(*node)
}

fn present(driver: &HeadlessDriver, window: WindowId, name: &str) -> bool {
    !tree(driver, window).test_ids(name).is_empty()
}

fn advance(driver: &mut HeadlessDriver, millis: u64) {
    driver.advance_to(Duration::from_millis(millis)).unwrap();
}

#[test]
fn toast_timers_preserve_remaining_time_and_publish_live_policies() {
    struct Message(&'static str);

    let mut driver = HeadlessDriver::default();
    let queue = driver.app_mut().build(ToastQueue::<Message>::new).unwrap();
    let window = driver
        .app_mut()
        .open_window(WindowOptions::default(), move |_| {
            col().child(each(queue.items(), move |build, id, item| {
                let label = build.read(|read| read.with(item, |toast| toast.value.0));
                queue.item(build, text(label).test_id(format!("toast-{id}")), *id)
            }))
        })
        .unwrap();
    let (polite, assertive, persistent, dismissed) = driver
        .app_mut()
        .update_window(window, move |write| {
            let polite =
                queue.push(write, Message("Same text"), ToastOptions { duration: Some(Duration::from_millis(100)), ..Default::default() });
            let assertive =
                queue.push(write, Message("Same text"), ToastOptions { live: Live::Assertive, duration: Some(Duration::from_millis(50)) });
            let persistent = queue.push(write, Message("Keep"), ToastOptions { duration: None, ..Default::default() });
            let dismissed =
                queue.push(write, Message("Remove"), ToastOptions { duration: Some(Duration::from_millis(5)), ..Default::default() });
            assert!(queue.dismiss(write, dismissed));
            assert!(!queue.dismiss(write, dismissed));
            (polite, assertive, persistent, dismissed)
        })
        .unwrap();
    let polite_name = format!("toast-{polite}");
    let assertive_name = format!("toast-{assertive}");
    let persistent_name = format!("toast-{persistent}");
    assert!(!present(&driver, window, &format!("toast-{dismissed}")));
    let polite_node = target(&driver, window, &polite_name).node().unwrap();
    let assertive_node = target(&driver, window, &assertive_name).node().unwrap();
    assert_ne!(polite_node, assertive_node);
    assert_eq!(tree(&driver, window).semantics(polite_node).unwrap().live, Some(Live::Polite));
    assert_eq!(tree(&driver, window).semantics(assertive_node).unwrap().live, Some(Live::Assertive));
    assert_eq!(driver.app().runtime().next_deadline(), Some(Duration::from_millis(50)));

    advance(&mut driver, 30);
    driver
        .app_mut()
        .update_window(window, move |write| {
            assert!(queue.pause(write, polite));
            assert!(!queue.pause(write, polite));
            assert!(!queue.resume(write, persistent));
        })
        .unwrap();
    advance(&mut driver, 49);
    assert!(present(&driver, window, &assertive_name));
    advance(&mut driver, 50);
    assert!(!present(&driver, window, &assertive_name));
    advance(&mut driver, 120);
    assert_eq!(target(&driver, window, &polite_name).node(), Some(polite_node));

    driver
        .app_mut()
        .update_window(window, move |write| {
            assert!(queue.resume(write, polite));
            assert!(!queue.resume(write, polite));
        })
        .unwrap();
    advance(&mut driver, 189);
    assert!(present(&driver, window, &polite_name));
    advance(&mut driver, 190);
    assert!(!present(&driver, window, &polite_name));
    assert!(present(&driver, window, &persistent_name));
    assert_eq!(driver.app().runtime().next_deadline(), None);
}

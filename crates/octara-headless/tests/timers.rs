use octara::{HostReply, Input, InteractionEvent, PointerId, TargetGuard, Tree, prelude::*};
use octara_headless::{
    toast::{ToastOptions, ToastQueue},
    tooltip::{TooltipGroup, TooltipOptions, TooltipState},
};
use octara_inspect::{AgentCommand, AgentRequest, HeadlessDriver};
use std::time::Duration;

const TOOLTIP_OPTIONS: TooltipOptions =
    TooltipOptions { open_delay: Duration::from_millis(100), close_delay: Duration::from_millis(30), cooldown: Duration::from_millis(50) };

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

fn send(driver: &mut HeadlessDriver, window: WindowId, event: InteractionEvent) {
    driver.dispatch(window, Input::Interaction(event)).unwrap();
}

fn advance(driver: &mut HeadlessDriver, millis: u64) {
    driver.advance_to(Duration::from_millis(millis)).unwrap();
}

fn pointer(driver: &mut HeadlessDriver, window: WindowId, x: f32, y: f32) {
    send(driver, window, InteractionEvent::PointerMove { id: PointerId(1), pos: Point::new(x, y), mods: Modifiers::empty() });
}

fn escape(driver: &mut HeadlessDriver, window: WindowId) {
    send(driver, window, InteractionEvent::Key(KeyEvent::new(Key::Escape, Modifiers::empty())));
}

#[test]
fn tooltip_cancels_warmup_and_shares_cooldown_between_anchors() {
    let mut driver = HeadlessDriver::default();
    let show_second = driver.app_mut().build(|build| build.signal(true)).unwrap();
    let window = driver
        .app_mut()
        .open_window(WindowOptions::default(), move |build| {
            let group = TooltipGroup::new(build);
            let first = TooltipState::in_group(build, group, TOOLTIP_OPTIONS);
            row()
                .child(
                    first.trigger(el().test_id("first").w(50).h(20), Placement::BELOW, None, |_| text("First help").test_id("first-help")),
                )
                .child(show(
                    move |read| read.get(show_second),
                    move |build| {
                        TooltipState::in_group(build, group, TOOLTIP_OPTIONS).trigger(
                            el().test_id("second").w(50).h(20),
                            Placement::BELOW,
                            None,
                            |_| text("Second help").test_id("second-help"),
                        )
                    },
                ))
        })
        .unwrap();

    pointer(&mut driver, window, 20.0, 10.0);
    advance(&mut driver, 99);
    assert!(!present(&driver, window, "first-help"));
    send(&mut driver, window, InteractionEvent::PointerLeave);
    advance(&mut driver, 120);
    assert!(!present(&driver, window, "first-help"));

    pointer(&mut driver, window, 20.0, 10.0);
    advance(&mut driver, 219);
    assert!(!present(&driver, window, "first-help"));
    advance(&mut driver, 220);
    let anchor = target(&driver, window, "first");
    let panel = target(&driver, window, "first-help");
    let receipt = driver.execute(AgentRequest::new(Some(window), AgentCommand::Observe));
    let nodes = receipt.snapshot.unwrap().nodes;
    assert_eq!(nodes.iter().find(|node| node.target == anchor).unwrap().relations.described_by, Some(panel));
    let node = nodes.iter().find(|node| node.target == panel).unwrap();
    assert_eq!(node.role, Some(Role::Tooltip));
    assert!(node.geometry.unwrap().rect.y > 20.0);

    // Crossing the placement gap leaves the anchor before entering the panel.
    pointer(&mut driver, window, 20.0, 22.0);
    advance(&mut driver, 225);
    pointer(&mut driver, window, 20.0, 30.0);
    advance(&mut driver, 400);
    assert!(present(&driver, window, "first-help"));

    pointer(&mut driver, window, 70.0, 10.0);
    assert!(!present(&driver, window, "first-help"));
    assert!(present(&driver, window, "second-help"));
    escape(&mut driver, window);
    assert!(!present(&driver, window, "second-help"));
    let nodes = driver.execute(AgentRequest::new(Some(window), AgentCommand::Observe)).snapshot.unwrap().nodes;
    let second = nodes.iter().find(|node| node.test_id.as_deref() == Some("second")).unwrap();
    assert_eq!(second.relations.described_by, None);

    advance(&mut driver, 449);
    assert!(!present(&driver, window, "second-help"));
    pointer(&mut driver, window, 20.0, 10.0);
    assert!(present(&driver, window, "first-help"));

    send(&mut driver, window, InteractionEvent::PointerLeave);
    advance(&mut driver, 478);
    assert!(present(&driver, window, "first-help"));
    advance(&mut driver, 479);
    assert!(!present(&driver, window, "first-help"));
    advance(&mut driver, 529);
    pointer(&mut driver, window, 70.0, 10.0);
    advance(&mut driver, 628);
    assert!(!present(&driver, window, "second-help"));
    advance(&mut driver, 629);
    assert!(present(&driver, window, "second-help"));

    advance(&mut driver, 700);
    driver.app_mut().update(move |write| write.set(show_second, false)).unwrap();
    assert!(!present(&driver, window, "second-help"));
    pointer(&mut driver, window, 20.0, 10.0);
    advance(&mut driver, 799);
    assert!(!present(&driver, window, "first-help"));
    advance(&mut driver, 800);
    assert!(present(&driver, window, "first-help"));
}

#[test]
fn tooltip_focus_uses_the_delay_and_escape_cancels_a_pending_open() {
    let mut driver = HeadlessDriver::default();
    let window = driver
        .app_mut()
        .open_window(WindowOptions::default(), |build| {
            TooltipState::new(build, TOOLTIP_OPTIONS)
                .trigger(el().test_id("anchor").w(50).h(20), Placement::BELOW, None, |_| text("Keyboard help").test_id("help"))
        })
        .unwrap();

    let anchor = target(&driver, window, "anchor");
    let focus = |driver: &mut HeadlessDriver, target| {
        driver.dispatch(window, Input::Host(HostReply::FocusChanged { target, visible: true })).unwrap();
    };
    focus(&mut driver, Some(anchor));
    advance(&mut driver, 40);
    assert!(!present(&driver, window, "help"));
    escape(&mut driver, window);
    advance(&mut driver, 100);
    assert!(!present(&driver, window, "help"));
    assert_eq!(tree(&driver, window).focus(), anchor.node());

    focus(&mut driver, None);
    focus(&mut driver, Some(anchor));
    advance(&mut driver, 199);
    assert!(!present(&driver, window, "help"));
    advance(&mut driver, 200);
    assert!(present(&driver, window, "help"));
    focus(&mut driver, None);
    assert!(!present(&driver, window, "help"));
}

#[test]
fn tooltip_cancels_pending_open_when_its_hovered_trigger_is_disabled() {
    let mut driver = HeadlessDriver::default();
    let disabled = driver.app_mut().build(|build| build.signal(false)).unwrap();
    let window = driver
        .app_mut()
        .open_window(WindowOptions::default(), move |build| {
            TooltipState::new(build, TOOLTIP_OPTIONS)
                .trigger(el().disabled(disabled).w(50).h(20), Placement::BELOW, None, |_| text("Hover help").test_id("help"))
        })
        .unwrap();

    pointer(&mut driver, window, 20.0, 10.0);
    advance(&mut driver, 40);
    driver.app_mut().update(move |write| write.set(disabled, true)).unwrap();
    advance(&mut driver, 100);
    assert!(!present(&driver, window, "help"));

    driver.app_mut().update(move |write| write.set(disabled, false)).unwrap();
    pointer(&mut driver, window, 20.0, 10.0);
    advance(&mut driver, 199);
    assert!(!present(&driver, window, "help"));
    advance(&mut driver, 200);
    assert!(present(&driver, window, "help"));
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
                let label = build.read(|read| item.with(read, |toast| toast.unwrap().value.0));
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

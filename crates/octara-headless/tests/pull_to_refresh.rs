use octara::model::input::{InteractionEvent, PointerId, PointerKind};
use octara::prelude::*;
use octara::runtime::Input;
use octara_headless::PullToRefresh;
use octara_inspect::HeadlessDriver;
use std::time::Duration;

const CONTACT: PointerId = PointerId(1);

fn touch(driver: &mut HeadlessDriver, window: WindowId, event: InteractionEvent) {
    driver.dispatch(window, Input::Interaction(event)).unwrap();
}

fn down(driver: &mut HeadlessDriver, window: WindowId, y: f32) {
    let pos = Point::new(30.0, y);
    let target = driver.app().runtime().window(window).unwrap().hit_test(pos).unwrap();
    touch(
        driver,
        window,
        InteractionEvent::PointerDown {
            id: CONTACT,
            kind: PointerKind::Touch,
            target,
            pos,
            button: Button::Primary,
            mods: Modifiers::empty(),
        },
    );
}

fn moved(driver: &mut HeadlessDriver, window: WindowId, y: f32) {
    touch(driver, window, InteractionEvent::PointerMove { id: CONTACT, pos: Point::new(30.0, y), mods: Modifiers::empty() });
}

fn release(driver: &mut HeadlessDriver, window: WindowId, y: f32) {
    let pos = Point::new(30.0, y);
    let target = driver.app().runtime().window(window).unwrap().hit_test(pos);
    touch(driver, window, InteractionEvent::PointerUp { id: CONTACT, target, pos, mods: Modifiers::empty() });
}

fn scrollport() -> El {
    col().h(140).w(100).overflow_y(Overflow::Scroll).child(el().h(240).shrink(0.0))
}

#[test]
fn threshold_haptics_refresh_and_spring_follow_the_virtual_clock() {
    let mut driver = HeadlessDriver::default();
    let (state, refreshes, haptics) = driver.app_mut().build(|b| (PullToRefresh::new(b, 20.0), b.signal(0), b.signal(0))).unwrap();
    let window = driver
        .app_mut()
        .open_window(WindowOptions { touch: true, ..WindowOptions::default() }, move |_| {
            state.attach_with_haptic(
                scrollport(),
                move |w| w.update(refreshes, |count| *count += 1),
                move |w| w.update(haptics, |count| *count += 1),
            )
        })
        .unwrap();

    down(&mut driver, window, 20.0);
    moved(&mut driver, window, 40.0);
    release(&mut driver, window, 40.0);
    driver.advance_to(Duration::from_secs(1)).unwrap();
    assert_eq!(driver.app_mut().update(|w| (w.get(refreshes), w.get(haptics))).unwrap(), (0, 0));

    down(&mut driver, window, 20.0);
    moved(&mut driver, window, 80.0);
    touch(&mut driver, window, InteractionEvent::PointerCancel { id: CONTACT });
    driver.advance_to(Duration::from_secs(2)).unwrap();
    assert_eq!(driver.app_mut().update(|w| (w.get(refreshes), w.get(haptics))).unwrap(), (0, 1));

    down(&mut driver, window, 20.0);
    moved(&mut driver, window, 80.0);
    release(&mut driver, window, 80.0);
    let read_state = |w: &mut WriteCx<'_>| (w.read(|r| state.distance(r)), w.read(|r| state.is_refreshing(r)));
    assert_eq!(driver.app_mut().update(read_state).unwrap(), (30.0, true));
    assert_eq!(driver.app_mut().update(|w| (w.get(refreshes), w.get(haptics))).unwrap(), (1, 2));
    assert!(driver.app().runtime().next_deadline().is_some());

    driver.advance_to(Duration::from_millis(2100)).unwrap();
    let (distance, refreshing) = driver.app_mut().update(read_state).unwrap();
    assert!(distance > 0.0 && distance < 30.0 && refreshing);
    driver.advance_to(Duration::from_secs(3)).unwrap();
    assert_eq!(driver.app_mut().update(read_state).unwrap(), (0.0, true));
    driver.app_mut().update(|w| state.finish(w)).unwrap();
    assert_eq!(driver.app_mut().update(read_state).unwrap(), (0.0, false));
}

#[test]
fn keyboard_dismissal_follows_the_scrollport_policy() {
    for policy in [ScrollDismissesKeyboard::Never, ScrollDismissesKeyboard::Immediately, ScrollDismissesKeyboard::Interactively] {
        let mut driver = HeadlessDriver::default();
        let window = driver
            .app_mut()
            .open_window(WindowOptions { touch: true, ..WindowOptions::default() }, move |_| {
                col().child((input().h(30).autofocus(), scrollport().scroll_dismisses_keyboard(policy)))
            })
            .unwrap();
        let focus = |driver: &HeadlessDriver| driver.app().runtime().window(window).unwrap().tree().focus().is_some();
        assert!(focus(&driver));
        driver.app_mut().runtime_mut().set_keyboard_open(window, true);
        down(&mut driver, window, 45.0);
        assert!(focus(&driver));
        moved(&mut driver, window, 57.0);
        assert_eq!(focus(&driver), policy != ScrollDismissesKeyboard::Immediately);
        moved(&mut driver, window, 81.0);
        release(&mut driver, window, 81.0);
        assert_eq!(focus(&driver), policy == ScrollDismissesKeyboard::Never);
    }
}

#[test]
fn a_new_drag_cannot_refresh_with_distance_left_by_a_cancelled_pull() {
    let mut driver = HeadlessDriver::default();
    let (state, refreshes, haptics) = driver.app_mut().build(|b| (PullToRefresh::new(b, 20.0), b.signal(0), b.signal(0))).unwrap();
    let window = driver
        .app_mut()
        .open_window(WindowOptions { touch: true, ..WindowOptions::default() }, move |_| {
            state.attach_with_haptic(
                scrollport(),
                move |w| w.update(refreshes, |count| *count += 1),
                move |w| w.update(haptics, |count| *count += 1),
            )
        })
        .unwrap();

    down(&mut driver, window, 20.0);
    moved(&mut driver, window, 100.0);
    touch(&mut driver, window, InteractionEvent::PointerCancel { id: CONTACT });
    down(&mut driver, window, 40.0);
    moved(&mut driver, window, 28.0);
    release(&mut driver, window, 28.0);
    assert_eq!(driver.app_mut().update(|w| (w.get(refreshes), w.get(haptics))).unwrap(), (0, 1));
}

#[test]
fn closing_in_pre_scroll_retires_the_queued_viewport_and_post_stages() {
    let mut driver = HeadlessDriver::default();
    let window = driver
        .app_mut()
        .open_window(WindowOptions { touch: true, ..WindowOptions::default() }, move |_| {
            scrollport().on_touch_scroll_pre(|w, _| {
                w.close_window(w.window_id().unwrap());
                Default::default()
            })
        })
        .unwrap();
    down(&mut driver, window, 20.0);
    moved(&mut driver, window, 42.0);
    assert!(driver.app().runtime().window(window).is_none());
}

use super::{driver, options, target};
use octara::{CycleReceipt, Input, prelude::*, tree::overlay::LayerPolicy};
use octara_inspect::HeadlessDriver;
use octara_model::input::{Button, ContactTarget, InteractionEvent, Modifiers, PointerId, PointerKind};
use std::{cell::RefCell, rc::Rc, time::Duration};

const POINTER: PointerId = PointerId(1);

fn send(driver: &mut HeadlessDriver, window: WindowId, event: InteractionEvent) -> CycleReceipt {
    driver.dispatch(window, Input::Interaction(event)).unwrap()
}

fn hit(driver: &HeadlessDriver, window: WindowId, pos: Point) -> Option<ContactTarget> {
    driver.app().runtime().window(window).unwrap().hit_test(pos)
}

fn down(driver: &mut HeadlessDriver, window: WindowId, pos: Point) {
    let target = hit(driver, window, pos).expect("contact hit");
    let event = InteractionEvent::PointerDown {
        id: POINTER,
        kind: PointerKind::Touch,
        target,
        pos,
        button: Button::Primary,
        mods: Modifiers::empty(),
    };
    assert!(send(driver, window, event).delivered);
}

fn up(driver: &mut HeadlessDriver, window: WindowId, pos: Point) {
    let target = hit(driver, window, pos);
    assert!(send(driver, window, InteractionEvent::PointerUp { id: POINTER, target, pos, mods: Modifiers::empty() }).delivered);
}

fn moved(driver: &mut HeadlessDriver, window: WindowId, pos: Point) {
    send(driver, window, InteractionEvent::PointerMove { id: POINTER, pos, mods: Modifiers::empty() });
}

#[test]
fn long_press_uses_the_virtual_clock_and_an_inclusive_eight_pixel_slop() {
    let mut driver = driver();
    let events = Rc::new(RefCell::new(Vec::new()));
    let window = driver
        .app_mut()
        .open_window(options(), {
            let events = events.clone();
            move |_| {
                el().size(60).on_gesture(move |_, event| {
                    events.borrow_mut().push(*event);
                    true
                })
            }
        })
        .unwrap();
    let at = Duration::from_millis;
    let deadline = |driver: &HeadlessDriver| driver.app().runtime().next_deadline();
    let start = Point::new(16.0, 16.0);
    let edge = Point::new(24.0, 16.0);
    let changed = Point::new(22.0, 16.0);
    let outside = Point::new(25.0, 16.0);
    let long = |phase, position| GestureEvent::LongPress { phase, position, pointer: PointerKind::Touch };

    assert!(hit(&driver, window, start).unwrap().action.is_none());
    down(&mut driver, window, start);
    moved(&mut driver, window, edge);
    assert_eq!(deadline(&driver), Some(at(500)));
    driver.advance_to(at(499)).unwrap();
    assert!(events.borrow().is_empty());
    assert!(driver.advance_to(at(500)).unwrap().handled);
    assert_eq!(deadline(&driver), None);
    moved(&mut driver, window, changed);
    up(&mut driver, window, changed);
    assert_eq!(
        *events.borrow(),
        [long(GesturePhase::Started, edge), long(GesturePhase::Changed, changed), long(GesturePhase::Ended, changed)]
    );

    events.borrow_mut().clear();
    down(&mut driver, window, start);
    moved(&mut driver, window, outside);
    moved(&mut driver, window, start);
    assert_eq!(deadline(&driver), None);
    driver.advance_to(at(1000)).unwrap();
    up(&mut driver, window, start);
    assert!(events.borrow().is_empty());

    down(&mut driver, window, start);
    driver.advance_to(at(1500)).unwrap();
    send(&mut driver, window, InteractionEvent::PointerCancel { id: POINTER });
    assert_eq!(*events.borrow(), [long(GesturePhase::Started, start), long(GesturePhase::Cancelled, start)]);
}

#[test]
fn double_tap_reuses_tap_timing_and_preserves_press_handlers() {
    let mut driver = driver();
    let events = Rc::new(RefCell::new(Vec::new()));
    let (presses, double_presses) = driver.app_mut().build(|b| (b.signal(0), b.signal(0))).unwrap();
    let window = driver
        .app_mut()
        .open_window(options(), {
            let events = events.clone();
            move |_| {
                row()
                    .on_gesture(move |_, event| {
                        events.borrow_mut().push(*event);
                        true
                    })
                    .child(el().size(60).test_id("gesture-only"))
                    .child(el().size(60).on_press(move |w| w.update(presses, |count| *count += 1)).on_double_press(move |w| {
                        w.update(double_presses, |count| *count += 1);
                    }))
            }
        })
        .unwrap();
    let tap = |driver: &mut HeadlessDriver, x| {
        let pos = Point::new(x, 16.0);
        down(driver, window, pos);
        up(driver, window, pos);
    };
    let at = Duration::from_millis;

    tap(&mut driver, 16.0);
    driver.advance_to(at(400)).unwrap();
    tap(&mut driver, 32.0);
    tap(&mut driver, 74.0);
    driver.advance_to(at(800)).unwrap();
    tap(&mut driver, 90.0);
    assert_eq!(
        *events.borrow(),
        [32.0, 90.0].map(|x| GestureEvent::DoubleTap { position: Point::new(x, 16.0), pointer: PointerKind::Touch })
    );
    let counts = driver.app_mut().update(|w| (w.get(presses), w.get(double_presses))).unwrap();
    assert_eq!(counts, (2, 1));

    events.borrow_mut().clear();
    tap(&mut driver, 16.0);
    driver.advance_to(at(1201)).unwrap();
    tap(&mut driver, 16.0);
    down(&mut driver, window, Point::new(16.0, 16.0));
    moved(&mut driver, window, Point::new(25.0, 16.0));
    up(&mut driver, window, Point::new(16.0, 16.0));
    tap(&mut driver, 16.0);
    assert!(events.borrow().is_empty());
}

#[test]
fn hardware_gestures_keep_independent_hits_until_teardown() {
    let mut driver = driver();
    let events = Rc::new(RefCell::new(Vec::new()));
    let window = driver
        .app_mut()
        .open_window(options(), {
            let events = events.clone();
            move |_| {
                let target = |name| {
                    let events = events.clone();
                    el().size(60).on_gesture(move |_, event| {
                        events.borrow_mut().push((name, *event));
                        true
                    })
                };
                row().child(target("left")).child(target("right"))
            }
        })
        .unwrap();
    let left = Point::new(16.0, 16.0);
    let right = Point::new(76.0, 16.0);
    let offscreen = Point::new(-1.0, 16.0);
    let pinch = |phase, anchor| GestureEvent::Pinch { phase, scale: 1.0, delta_scale: 1.0, anchor, modifiers: Modifiers::empty() };
    let rotate = |phase, anchor| GestureEvent::Rotate { phase, radians: 0.0, delta_radians: 0.0, anchor, modifiers: Modifiers::empty() };
    let send_gesture = |driver: &mut HeadlessDriver, event| send(driver, window, InteractionEvent::Gesture(event)).handled;

    assert!(!send_gesture(&mut driver, pinch(GesturePhase::Changed, right)));
    let expected = [
        ("left", pinch(GesturePhase::Started, left)),
        ("right", rotate(GesturePhase::Started, right)),
        ("left", pinch(GesturePhase::Changed, right)),
        ("right", rotate(GesturePhase::Changed, left)),
        ("left", pinch(GesturePhase::Ended, offscreen)),
        ("right", rotate(GesturePhase::Cancelled, left)),
    ];
    for (_, event) in expected {
        assert!(send_gesture(&mut driver, event));
    }
    assert!(!send_gesture(&mut driver, pinch(GesturePhase::Changed, left)));
    assert!(!send_gesture(&mut driver, rotate(GesturePhase::Ended, right)));
    assert_eq!(*events.borrow(), expected);

    assert!(send_gesture(&mut driver, pinch(GesturePhase::Started, left)));
    send(&mut driver, window, InteractionEvent::WindowActive(false));
    send(&mut driver, window, InteractionEvent::WindowActive(true));
    assert!(!send_gesture(&mut driver, pinch(GesturePhase::Changed, left)));
}

#[test]
fn host_gestures_bubble_through_portal_owners_and_respect_modal_scope() {
    let mut driver = driver();
    let events = Rc::new(RefCell::new(Vec::new()));
    let modal = driver.app_mut().build(|b| b.signal(false)).unwrap();
    let window = driver
        .app_mut()
        .open_window(options(), {
            let events = events.clone();
            move |_| {
                let parent_events = events.clone();
                col()
                    .child(
                        el().test_id("gesture-owner")
                            .on_gesture(move |_, event| {
                                parent_events.borrow_mut().push(("owner", *event));
                                true
                            })
                            .portal_part(
                                |_| true,
                                move |_| {
                                    let events = events.clone();
                                    el().size(60).test_id("gesture-portal").on_gesture(move |_, event| {
                                        events.borrow_mut().push(("portal", *event));
                                        matches!(event, GestureEvent::Rotate { .. })
                                    })
                                },
                            ),
                    )
                    .child(el().portal_part(move |r| r.get(modal), |_| el().size(100).layer(LayerPolicy::modal(), |_| {})))
            }
        })
        .unwrap();
    let runtime = driver.app().runtime().window(window).unwrap();
    let owner = target(&driver, window, "gesture-owner").node().unwrap();
    let portal = target(&driver, window, "gesture-portal").node().unwrap();
    assert_eq!(runtime.tree().logical_parent(portal), Some(owner));
    assert_ne!(runtime.tree().physical_parent(portal), Some(owner));
    let bounds = runtime.geometry(portal).unwrap().rect;
    let anchor = Point::new(bounds.x + 16.0, bounds.y + 16.0);
    let pinch = GestureEvent::Pinch { phase: GesturePhase::Started, scale: 1.0, delta_scale: 1.0, anchor, modifiers: Modifiers::CTRL };
    let rotate =
        GestureEvent::Rotate { phase: GesturePhase::Started, radians: 0.0, delta_radians: 0.0, anchor, modifiers: Modifiers::SHIFT };
    assert!(send(&mut driver, window, InteractionEvent::Gesture(pinch)).handled);
    assert!(send(&mut driver, window, InteractionEvent::Gesture(rotate)).handled);
    assert_eq!(*events.borrow(), [("portal", pinch), ("owner", pinch), ("portal", rotate)]);

    events.borrow_mut().clear();
    down(&mut driver, window, anchor);
    driver.app_mut().update(|w| w.set(modal, true)).unwrap();
    assert!(!send(&mut driver, window, InteractionEvent::Gesture(pinch)).handled);
    assert_eq!(driver.app().runtime().next_deadline(), None);
    driver.advance_to(Duration::from_millis(500)).unwrap();
    driver.app_mut().update(|w| w.set(modal, false)).unwrap();
    let rotate =
        GestureEvent::Rotate { phase: GesturePhase::Changed, radians: 0.1, delta_radians: 0.1, anchor, modifiers: Modifiers::SHIFT };
    assert!(!send(&mut driver, window, InteractionEvent::Gesture(rotate)).handled);
    assert!(events.borrow().is_empty());
}

#[test]
fn replacing_a_hit_retires_its_hold_and_tap_history() {
    let mut driver = driver();
    let events = Rc::new(RefCell::new(Vec::new()));
    let generation = driver.app_mut().build(|b| b.signal(0)).unwrap();
    let window = driver
        .app_mut()
        .open_window(options(), {
            let events = events.clone();
            move |_| {
                keyed(
                    move |r| r.get(generation),
                    move |_, _| {
                        let events = events.clone();
                        el().size(60).test_id("replaceable").on_gesture(move |_, event| {
                            events.borrow_mut().push(*event);
                            true
                        })
                    },
                )
            }
        })
        .unwrap();
    let old = target(&driver, window, "replaceable");
    let pos = Point::new(16.0, 16.0);
    let pinch = |phase| GestureEvent::Pinch { phase, scale: 1.0, delta_scale: 1.0, anchor: pos, modifiers: Modifiers::empty() };
    assert!(send(&mut driver, window, InteractionEvent::Gesture(pinch(GesturePhase::Started))).handled);
    events.borrow_mut().clear();
    down(&mut driver, window, pos);
    up(&mut driver, window, pos);
    down(&mut driver, window, pos);
    driver.app_mut().update(|w| w.set(generation, 1)).unwrap();
    assert_ne!(target(&driver, window, "replaceable"), old);
    assert!(!send(&mut driver, window, InteractionEvent::Gesture(pinch(GesturePhase::Changed))).handled);
    assert_eq!(driver.app().runtime().next_deadline(), None);
    up(&mut driver, window, pos);
    down(&mut driver, window, pos);
    up(&mut driver, window, pos);
    driver.advance_to(Duration::from_millis(500)).unwrap();
    assert!(events.borrow().is_empty());
}

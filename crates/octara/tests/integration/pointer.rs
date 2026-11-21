//! Pointer input: presses with capture, double presses, context menus, long presses, drags,
//! touch scrolling with momentum, and the focus ring following the keyboard.

use octara::app::{Button, Event, InputEvent, Modifiers, Phase, PointerEvent, PointerKind, TargetPlatform, TouchScrollStage};
use octara::prelude::*;
use std::time::Duration;

fn center(app: &Window, id: &str) -> (f32, f32) {
    let k = app.kernel();
    let (x, y, w, h) = k.rect(k.find(id).unwrap());
    (x + w / 2.0, y + h / 2.0)
}

fn view() -> View {
    let log = state("log", Vec::<String>::new);
    let push = move |s: String| log.update(|l| l.push(s));
    col()
        .size_full()
        .child(text(move || log.get().join(",")).id("log"))
        .child(
            el().id("button")
                .w(px(120.0))
                .h(px(40.0))
                .focusable()
                .on_press(move || push("press".into()))
                .on_double_press(move || push("double".into()))
                .child("Button"),
        )
        .child(
            el().id("row")
                .w(px(200.0))
                .h(px(40.0))
                .on_secondary_press(move || push("menu".into()))
                .child(el().id("inner").w(px(40.0)).h(px(20.0)).on_press(move || push("inner".into()))),
        )
        .child(el().id("hold").w(px(120.0)).h(px(40.0)).on_press(move || push("hold".into())).on_long_press(move || push("long".into())))
        .child(el().id("knob").w(px(40.0)).h(px(40.0)).on_drag(move |d| push(format!("{:?} {} {}", d.phase, d.dx, d.dy))))
        .child(
            col()
                .id("list")
                .h(px(200.0))
                .overflow_y_scroll()
                .children((0..40).map(move |i| el().id(format!("item-{i}")).h(px(40.0)).on_press(move || push(format!("item{i}"))))),
        )
        .into()
}

/// The handlers run since the last call.
fn log(app: &mut Window) -> String {
    let s = app.kernel().text_of("log").unwrap();
    app.app().send("clear", "").unwrap();
    s
}

fn app() -> Window {
    Window::headless(|| {
        let log = state("log", Vec::<String>::new);
        channel::<String>().1.named("clear").on(move |_| log.set(Vec::new()));
        view()
    })
}

fn touch(app: &mut Window, phase: Phase, (x, y): (f32, f32)) {
    app.event(InputEvent::Pointer(PointerEvent::touch(1, phase, x, y)));
}

#[test]
fn press_follows_the_pointer_out_and_back() {
    let mut app = app();
    let (x, y) = center(&app, "button");
    app.pointer_down(x, y);
    assert!(app.kernel().state_of("button").unwrap().contains(State::PRESSED));
    app.pointer_move(x + 500.0, y);
    assert!(!app.kernel().state_of("button").unwrap().contains(State::PRESSED), "outside: not pressed");
    app.pointer_move(x, y);
    assert!(app.kernel().state_of("button").unwrap().contains(State::PRESSED), "back inside: pressed again");
    app.pointer_up(x, y);
    assert_eq!(log(&mut app), "press");
    app.pointer_down(x, y);
    app.pointer_up(x + 500.0, y);
    assert_eq!(log(&mut app), "", "released outside: no press");
}

#[test]
fn a_touch_press_survives_the_button_moving_when_the_editor_blurs() {
    let mut app = Window::headless(|| {
        let editing = state("editing", || true);
        let calls = state("calls", Vec::<&str>::new);
        col()
            .size_full()
            .child(input().id("editor").w(120).h(40).on_blur(move || {
                calls.update(|calls| calls.push("blur"));
                editing.set(false);
            }))
            .child(show(move || editing.get(), || el().h(100)))
            .child(el().id("done").w(120).h(40).focusable().on_press(move || calls.update(|calls| calls.push("press"))))
            .child(text(move || calls.get().join(",")).id("calls"))
    });

    let editor = center(&app, "editor");
    touch(&mut app, Phase::Down, editor);
    assert_eq!(app.inspect().focus.as_deref(), Some("editor"));
    touch(&mut app, Phase::Up, editor);

    let at = center(&app, "done");
    touch(&mut app, Phase::Down, at);
    assert_eq!(app.inspect().focus.as_deref(), Some("editor"));
    assert_eq!(center(&app, "done"), at);
    assert!(app.kernel().state_of("done").unwrap().contains(State::PRESSED));
    assert_eq!(app.kernel().text_of("calls").as_deref(), Some(""));

    touch(&mut app, Phase::Up, at);
    assert_eq!(app.inspect().focus.as_deref(), Some("done"));
    assert_eq!(center(&app, "done"), (at.0, at.1 - 100.0));
    assert_eq!(app.kernel().text_of("calls").as_deref(), Some("blur,press"));
}

#[test]
fn a_cancelled_touch_or_release_outside_keeps_the_editor_focused() {
    let mut app = Window::headless(|| {
        let presses = state("presses", || 0);
        col()
            .size_full()
            .child(input().id("editor").w(120).h(40))
            .child(el().id("done").w(120).h(40).focusable().on_press(move || presses.update(|n| *n += 1)))
            .child(text(move || presses.get().to_string()).id("presses"))
    });
    app.tap("editor").unwrap();
    let at = center(&app, "done");

    touch(&mut app, Phase::Down, at);
    touch(&mut app, Phase::Cancel, at);
    touch(&mut app, Phase::Up, at);
    assert_eq!(app.inspect().focus.as_deref(), Some("editor"));
    assert_eq!(app.kernel().text_of("presses").as_deref(), Some("0"));

    let outside = (at.0 + 200.0, at.1);
    touch(&mut app, Phase::Down, at);
    touch(&mut app, Phase::Move, outside);
    touch(&mut app, Phase::Up, outside);
    assert_eq!(app.inspect().focus.as_deref(), Some("editor"));
    assert_eq!(app.kernel().text_of("presses").as_deref(), Some("0"));
}

#[test]
fn a_touch_press_does_not_activate_a_button_replaced_when_the_editor_blurs() {
    let mut app = Window::headless(|| {
        let replaced = state("replaced", || false);
        let presses = state("presses", || 0);
        col()
            .size_full()
            .child(input().id("editor").w(120).h(40).on_blur(move || replaced.set(true)))
            .child(keyed(move || replaced.get(), move |_| el().id("done").w(120).h(40).on_press(move || presses.update(|n| *n += 1))))
            .child(text(move || presses.get().to_string()).id("presses"))
    });
    app.tap("editor").unwrap();
    let at = center(&app, "done");
    let original = app.inspect().find("done").unwrap().target.clone();

    touch(&mut app, Phase::Down, at);
    assert_eq!(app.inspect().find("done").unwrap().target, original);
    touch(&mut app, Phase::Up, at);
    assert!(app.inspect().focus.is_none());
    assert_ne!(app.inspect().find("done").unwrap().target, original);
    assert_eq!(center(&app, "done"), at);
    assert_eq!(app.kernel().text_of("presses").as_deref(), Some("0"));
}

#[test]
fn a_mouse_release_must_match_the_button_held_down() {
    let mut app = app();
    let (x, y) = center(&app, "button");
    let mouse = |phase, button| InputEvent::Pointer(PointerEvent::mouse(phase, x, y).button(button));
    app.event(mouse(Phase::Down, Button::Primary));
    for button in [Button::Secondary, Button::Middle] {
        app.event(mouse(Phase::Down, button));
        app.event(mouse(Phase::Up, button));
        assert_eq!(app.held_pointer(), Some((0, PointerKind::Mouse)));
        assert!(app.kernel().state_of("button").unwrap().contains(State::PRESSED));
        assert_eq!(log(&mut app), "");
    }
    app.event(mouse(Phase::Up, Button::Primary));
    assert_eq!(app.held_pointer(), None);
    assert!(!app.kernel().state_of("button").unwrap().contains(State::PRESSED));
    assert_eq!(log(&mut app), "press");
}

#[test]
fn double_press_within_the_interval() {
    let mut app = app();
    let (x, y) = center(&app, "button");
    app.click_at(x, y);
    app.app().advance(Duration::from_millis(200));
    app.click_at(x + 1.0, y);
    assert_eq!(log(&mut app), "press,press,double");
    app.app().advance(Duration::from_millis(600));
    app.click_at(x, y);
    assert_eq!(log(&mut app), "press", "too late for a double press");
}

#[test]
fn right_click_and_control_click_ask_for_a_context_menu() {
    let mut app = app();
    let (x, y) = center(&app, "inner");
    let right = |phase| PointerEvent::mouse(phase, x, y).button(Button::Secondary);
    app.event(InputEvent::Pointer(right(Phase::Down)));
    app.event(InputEvent::Pointer(right(Phase::Up)));
    assert_eq!(log(&mut app), "menu", "the nearest element with a context menu, from the pointer up");
    app.app().set_target_platform(TargetPlatform::MacOs);
    let ctrl = |phase| PointerEvent::mouse(phase, x, y).modifiers(Modifiers::CTRL);
    app.event(InputEvent::Pointer(ctrl(Phase::Down)));
    app.event(InputEvent::Pointer(right(Phase::Down)));
    app.event(InputEvent::Pointer(right(Phase::Up)));
    assert_eq!(app.held_pointer(), Some((0, PointerKind::Mouse)));
    assert_eq!(log(&mut app), "");
    // The primary release completes the context click even if Control has already lifted.
    app.event(InputEvent::Pointer(PointerEvent::mouse(Phase::Up, x, y)));
    assert_eq!(app.held_pointer(), None);
    assert_eq!(log(&mut app), "menu");
    app.app().set_target_platform(TargetPlatform::Windows);
    app.event(InputEvent::Pointer(ctrl(Phase::Down)));
    app.event(InputEvent::Pointer(ctrl(Phase::Up)));
    assert_eq!(log(&mut app), "inner", "Control-click is a click off Apple platforms");
}

#[test]
fn long_press_fires_on_time_and_replaces_the_press() {
    let mut app = app();
    let at = center(&app, "hold");
    touch(&mut app, Phase::Down, at);
    assert_eq!(app.app().next_deadline(), Some(Duration::from_millis(500)));
    app.app().advance(Duration::from_millis(499));
    assert_eq!(log(&mut app), "");
    app.app().advance(Duration::from_millis(1));
    assert_eq!(log(&mut app), "long");
    touch(&mut app, Phase::Up, at);
    assert_eq!(log(&mut app), "", "the release presses nothing");
    // A short tap is a press. Without `on_long_press`, a long touch asks for a context menu.
    touch(&mut app, Phase::Down, at);
    touch(&mut app, Phase::Up, at);
    assert_eq!(log(&mut app), "hold");
    let at = center(&app, "inner");
    touch(&mut app, Phase::Down, at);
    app.app().advance(Duration::from_secs(1));
    touch(&mut app, Phase::Up, at);
    assert_eq!(log(&mut app), "menu");
}

#[test]
fn a_long_press_timer_does_not_use_a_target_removed_in_the_same_clock_step() {
    let mut app = Window::headless(|| {
        let alive = state("alive", || true);
        let count = state("count", || 0);
        spawn(async move {
            sleep(Duration::from_millis(150)).await;
            alive.set(false);
        });
        col()
            .size_full()
            .child(text(move || count.get().to_string()).id("count"))
            .child(show(move || alive.get(), move || el().id("hold").w(120).h(48).on_long_press(move || count.update(|n| *n += 1))))
    });
    let at = center(&app, "hold");
    touch(&mut app, Phase::Down, at);
    assert_eq!(app.held_pointer(), Some((1, PointerKind::Touch)));
    app.app().advance(Duration::from_millis(600));
    assert!(!app.kernel().exists("hold"));
    assert_eq!(app.kernel().text_of("count").as_deref(), Some("0"));
    assert!(!app.pointer_long_pressed(1));
    touch(&mut app, Phase::Cancel, at);
    assert_eq!(app.held_pointer(), None);
}

#[test]
fn becoming_disabled_during_a_hold_does_not_report_long_press_recognition() {
    let mut app = Window::headless(|| {
        let disabled = state("disabled", || false);
        let count = state("count", || 0);
        spawn(async move {
            sleep(Duration::from_millis(150)).await;
            disabled.set(true);
        });
        col()
            .size_full()
            .child(text(move || count.get().to_string()).id("count"))
            .child(el().id("hold").w(120).h(48).disabled(move || disabled.get()).on_long_press(move || count.update(|n| *n += 1)))
    });
    let at = center(&app, "hold");
    touch(&mut app, Phase::Down, at);
    app.app().advance(Duration::from_millis(500));
    assert_eq!(app.kernel().text_of("count").as_deref(), Some("0"));
    assert!(!app.pointer_long_pressed(1));
    touch(&mut app, Phase::Up, at);
    assert_eq!(app.held_pointer(), None);
}

#[test]
fn drag_starts_past_the_slop_and_is_not_a_press() {
    let mut app = app();
    let (x, y) = center(&app, "knob");
    app.pointer_down(x, y);
    app.pointer_move(x + 3.0, y);
    assert_eq!(log(&mut app), "", "within the 4 pt slop");
    app.pointer_move(x + 10.0, y + 2.0);
    app.pointer_move(x + 20.0, y + 2.0);
    app.pointer_up(x + 20.0, y + 2.0);
    assert_eq!(log(&mut app), "Start 10 2,Move 20 2,End 20 2");
    app.pointer_down(x, y);
    app.pointer_move(x + 10.0, y);
    app.event(InputEvent::Pointer(PointerEvent::mouse(Phase::Cancel, x + 10.0, y)));
    assert_eq!(log(&mut app), "Start 10 0,Cancel 10 0");
}

#[test]
fn a_drag_only_follows_the_captured_device_and_pointer_id() {
    let mut app = app();
    let (x, y) = center(&app, "knob");
    app.event(InputEvent::Pointer(PointerEvent::mouse(Phase::Down, x, y)));
    app.event(InputEvent::Pointer(PointerEvent::mouse(Phase::Move, x + 10.0, y)));
    assert_eq!(log(&mut app), "Start 10 0");

    for phase in [Phase::Move, Phase::Up, Phase::Cancel] {
        let mouse = PointerEvent::mouse(phase, x + 20.0, y);
        for foreign in [
            PointerEvent { kind: PointerKind::Touch, ..mouse },
            PointerEvent { kind: PointerKind::Pen, ..mouse },
            PointerEvent { id: 1, ..mouse },
        ] {
            app.event(InputEvent::Pointer(foreign));
            assert_eq!(app.held_pointer(), Some((0, PointerKind::Mouse)), "{foreign:?}");
            assert_eq!(log(&mut app), "", "{foreign:?}");
        }
    }

    app.event(InputEvent::Pointer(PointerEvent::mouse(Phase::Up, x + 30.0, y)));
    assert_eq!(app.held_pointer(), None);
    assert_eq!(log(&mut app), "End 30 0");
}

#[test]
fn deactivating_the_window_cancels_its_held_touch_drag() {
    let mut app = app();
    let (x, y) = center(&app, "knob");
    touch(&mut app, Phase::Down, (x, y));
    touch(&mut app, Phase::Move, (x + 10.0, y));
    app.event(InputEvent::Active(false));
    assert_eq!(app.held_pointer(), None);
    touch(&mut app, Phase::Up, (x + 10.0, y));
    assert_eq!(log(&mut app), "Start 10 0,Cancel 10 0");
}

fn offset(app: &Window) -> f32 {
    let k = app.kernel();
    k.scroll_offset(k.find("list").unwrap()).1
}

#[test]
fn a_touch_that_scrolls_cancels_the_press_and_glides() {
    let mut app = app();
    let (x, y) = center(&app, "item-2");
    touch(&mut app, Phase::Down, (x, y));
    assert!(app.kernel().state_of("item-2").unwrap().contains(State::PRESSED));
    // Drag the content up 60 pt over 60 ms.
    for i in 1..=6 {
        app.app().advance(Duration::from_millis(10));
        touch(&mut app, Phase::Move, (x, y - 10.0 * i as f32));
    }
    assert!(!app.kernel().state_of("item-2").unwrap().contains(State::PRESSED));
    assert_eq!(offset(&app), 60.0, "the content follows the finger");
    touch(&mut app, Phase::Up, (x, y - 60.0));
    assert_eq!(log(&mut app), "", "a scroll is not a press");
    // Momentum: 1 pt/ms decaying by 0.998 per ms.
    assert!(app.app().next_deadline().is_some());
    app.app().advance(Duration::from_millis(100));
    let glided = offset(&app) - 60.0;
    let expected = (0.998f64.powi(100) - 1.0) / 0.998f64.ln();
    assert!((glided as f64 - expected).abs() < 0.5, "{glided} vs {expected}");
    // A touch on gliding content stops it and presses nothing.
    let at = center(&app, "list");
    touch(&mut app, Phase::Down, at);
    assert_eq!(app.app().next_deadline(), None, "no momentum, and the touch is spent: no long press either");
    let stopped = offset(&app);
    touch(&mut app, Phase::Up, at);
    app.app().advance(Duration::from_millis(100));
    assert_eq!(offset(&app), stopped);
    assert_eq!(log(&mut app), "");
}

#[test]
fn momentum_runs_out() {
    for hz in [60, 120] {
        let mut window = app();
        let clock = window.app();
        let (x, y) = center(&window, "item-2");
        touch(&mut window, Phase::Down, (x, y));
        for i in 1..=4 {
            clock.advance(Duration::from_millis(10));
            touch(&mut window, Phase::Move, (x, y - 5.0 * i as f32));
        }
        touch(&mut window, Phase::Up, (x, y - 20.0));
        assert_eq!(clock.next_timer_deadline(), None);
        let start = clock.now();
        let release = offset(&window);
        for tick in 1..=3 * hz {
            let time = start + Duration::from_secs_f64(f64::from(tick) / f64::from(hz));
            clock.advance_to(time);
            if tick == 1 {
                assert!(offset(&window) > release);
                clock.advance_to(time);
                assert!(clock.next_deadline().is_some(), "a second observation at the same time keeps momentum");
            }
            if clock.next_deadline().is_none() {
                break;
            }
        }
        assert_eq!(clock.next_deadline(), None);
        let settled = offset(&window);
        clock.advance(Duration::from_secs(1));
        assert_eq!(offset(&window), settled);
    }
}

#[test]
fn wheel_under_a_held_press_cancels_it() {
    let mut app = app();
    let (x, y) = center(&app, "item-1");
    app.pointer_down(x, y);
    app.wheel(x, y, 0.0, 30.0);
    assert!(!app.kernel().state_of("item-1").unwrap().contains(State::PRESSED));
    app.pointer_up(x, y);
    assert_eq!(log(&mut app), "");
}

#[test]
fn hover_follows_content_moving_under_a_still_mouse() {
    let mut app = app();
    let (x, y) = center(&app, "item-1");
    app.pointer_move(x, y);
    assert!(app.kernel().state_of("item-1").unwrap().contains(State::HOVER));
    app.wheel(x, y, 0.0, 40.0);
    assert!(app.kernel().state_of("item-2").unwrap().contains(State::HOVER));
    assert!(!app.kernel().state_of("item-1").unwrap().contains(State::HOVER));
    // Touches do not hover.
    touch(&mut app, Phase::Down, (x, y));
    touch(&mut app, Phase::Up, (x, y));
    assert!(!app.kernel().state_of("item-2").unwrap().contains(State::HOVER));
}

#[test]
fn the_focus_ring_follows_the_keyboard() {
    let mut app = app();
    let (x, y) = center(&app, "button");
    app.click_at(x, y);
    let s = app.kernel().state_of("button").unwrap();
    assert!(s.contains(State::FOCUSED) && !s.contains(State::FOCUS_VISIBLE));
    app.press_key("tab").unwrap();
    app.press_key("shift+tab").unwrap();
    let s = app.kernel().state_of("button").unwrap();
    assert!(s.contains(State::FOCUSED) && s.contains(State::FOCUS_VISIBLE));
    app.pointer_down(x, y);
    assert!(!app.kernel().state_of("button").unwrap().contains(State::FOCUS_VISIBLE), "a press hides the ring");
}

#[test]
fn driving_by_element_checks_what_a_person_could_reach() {
    let mut app = Window::headless(|| {
        let n = state("n", || 0);
        let open = state("open", || false);
        col()
            .size_full()
            .child(text(move || n.get().to_string()).id("count"))
            .child(el().role(Role::Button).label("Save").focusable().on_press(move || n.update(|n| *n += 1)).child("💾"))
            .child(el().child("Add").on_press(move || n.update(|n| *n += 10)))
            .child(el().id("dup").child("Same").on_press(|| {}))
            .child(el().child("Same").on_press(|| {}))
            .child(el().id("open").child("Open").on_press(move || open.set(true)))
            .child(el().id("target").w(px(80.0)).h(px(30.0)).on_press(move || n.update(|n| *n += 100)))
            .child(el().id("far").absolute().top(px(2000.0)).w(px(10.0)).h(px(10.0)).on_press(|| {}))
            .child(show(move || open.get(), || el().id("cover").absolute().inset_0().bg(Token::Surface)))
    });
    app.tap("Save").unwrap();
    app.tap("Add").unwrap();
    assert_eq!(app.kernel().text_of("count").unwrap(), "11", "by label, and by a text inside the element");
    assert!(app.tap("Same").unwrap_err().contains("2 elements are named `Same`"));
    assert!(app.tap("nothing").unwrap_err().contains("no element"));
    assert!(app.tap("far").unwrap_err().contains("outside the window"));
    app.tap("open").unwrap();
    assert!(app.tap("target").unwrap_err().contains("covered at its centre"), "{}", app.tap("target").unwrap_err());
    assert!(app.tap("target").unwrap_err().contains("`cover`"));
}

#[test]
fn driving_gestures_by_element() {
    let mut app = app();
    app.double_tap("button").unwrap();
    assert_eq!(log(&mut app), "press,press,double");
    app.secondary_tap("inner").unwrap();
    assert_eq!(log(&mut app), "menu");
    app.long_press("hold").unwrap();
    assert_eq!(log(&mut app), "long");
    app.drag("knob", 20.0, 0.0).unwrap();
    assert_eq!(log(&mut app), "Start 5 0,Move 10 0,Move 15 0,Move 20 0,End 20 0");
    app.swipe("item-3", 0.0, -100.0, Duration::from_millis(100)).unwrap();
    assert!(offset(&app) >= 100.0);
    assert_eq!(log(&mut app), "");
}

#[test]
fn replacement_during_the_before_stage_drops_the_delta_and_its_contact() {
    let mut app = Window::headless(|| {
        let revision = state("revision", || 0_u32);
        let after_calls = state("after-calls", || 0_u32);
        col()
            .size_full()
            .child(keyed(
                move || revision.get(),
                move |&generation| {
                    col().id("list").h(100.0).overflow_y_scroll().child(el().h(200.0).child("Preview")).on_touch_scroll(move |stage, _| {
                        match stage {
                            TouchScrollStage::Before if generation == 0 => revision.set(1),
                            TouchScrollStage::Before => {}
                            TouchScrollStage::After => after_calls.update(|count| *count += 1),
                        }
                        [0.0; 2]
                    })
                },
            ))
            .child(text(move || after_calls.get().to_string()).id("after-calls"))
    });

    let (x, y) = center(&app, "list");
    let old = app.inspect().find("list").unwrap().target.clone();
    touch(&mut app, Phase::Down, (x, y));
    touch(&mut app, Phase::Move, (x, y - 30.0));
    assert_ne!(old, app.inspect().find("list").unwrap().target);
    assert!(app.held_pointer().is_none());
    assert!(app.inspect().touch_scroll.is_none());
    touch(&mut app, Phase::Move, (x, y - 42.0));
    touch(&mut app, Phase::Up, (x, y - 42.0));
    assert_eq!(offset(&app), 0.0);
    assert_eq!(app.kernel().text_of("after-calls").as_deref(), Some("0"));
    assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
}

fn nested_touch_window() -> Window {
    Window::headless(|| {
        let presses = state("presses", || 0);
        col()
            .size_full()
            .child(input().id("editor").h(40.0).value("Draft"))
            .child(
                col()
                    .id("outer")
                    .h(240.0)
                    .overflow_y_scroll()
                    .child(
                        col()
                            .id("list")
                            .w(160.0)
                            .h(100.0)
                            .flex_shrink_0()
                            .overflow_x_hidden()
                            .overflow_y_scroll()
                            .child(el().id("inner-item").w(220.0).h(400.0).focusable().on_press(move || presses.update(|n| *n += 1))),
                    )
                    .child(el().id("outer-item").h(480.0).focusable().on_press(move || presses.update(|n| *n += 1))),
            )
            .child(text(move || presses.get().to_string()).id("presses"))
    })
}

#[test]
fn a_nested_touch_reverses_its_glide_on_the_captured_axis_at_any_frame_cadence() {
    for hz in [1, 60, 120] {
        let mut app = nested_touch_window();
        let clock = app.app();
        let (x, y) = center(&app, "list");
        app.dispatch(Event::ScrollTo("list".into(), 0.0, 288.0)).unwrap();
        app.dispatch(Event::Focus("editor".into())).unwrap();
        touch(&mut app, Phase::Down, (x, y));
        clock.advance(Duration::from_millis(20));
        touch(&mut app, Phase::Move, (x - 5.0, y - 30.0));
        assert_eq!(offset(&app), 300.0);
        clock.advance(Duration::from_millis(20));
        touch(&mut app, Phase::Move, (x - 10.0, y - 75.0));
        assert_eq!(offset(&app), 300.0, "the contact stays with the inner viewport past its edge");
        clock.advance(Duration::from_millis(15));
        touch(&mut app, Phase::Move, (x - 15.0, y - 60.0));
        touch(&mut app, Phase::Up, (x - 15.0, y - 60.0));
        assert_eq!(offset(&app), 285.0);

        let release = clock.now();
        for frame in 1..hz / 20 {
            clock.advance_to(release + Duration::from_secs_f64(f64::from(frame) / f64::from(hz)));
        }
        clock.advance_to(release + Duration::from_millis(50));
        let k = app.kernel();
        let (inner_x, inner_y) = k.scroll_offset(k.find("list").unwrap());
        let decay = f64::from(app.tuning().momentum_decay);
        let expected = 285.0 - (decay.powi(50) - 1.0) / decay.ln();
        assert!((f64::from(inner_y) - expected).abs() < 0.01, "{hz} Hz: {inner_y} vs. {expected}");
        assert_eq!(inner_x, 0.0, "momentum stays on the enabled axis");
        assert_eq!(k.scroll_offset(k.find("outer").unwrap()), (0.0, 0.0));
        drop(k);
        clock.advance_to(release + Duration::from_secs(2));
        assert_eq!(offset(&app), 0.0);
        assert_eq!(app.inspect().focus.as_deref(), Some("editor"));
        assert_eq!(app.kernel().text_of("presses").as_deref(), Some("0"));
        assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
    }
}

#[test]
fn a_touch_grabs_a_nested_glide_and_cancels_without_disturbing_the_editor() {
    let mut app = nested_touch_window();
    let clock = app.app();
    let (x, y) = center(&app, "list");
    app.dispatch(Event::Focus("editor".into())).unwrap();
    app.set_selection(1, 4);
    app.event(InputEvent::Preedit { text: "nihao".into(), selection: Some((1, 3)) });
    let editor = app.text_field().unwrap();
    touch(&mut app, Phase::Down, (x, y));
    clock.advance(Duration::from_millis(20));
    touch(&mut app, Phase::Move, (x, y - 20.0));
    touch(&mut app, Phase::Up, (x, y - 20.0));
    clock.advance(Duration::from_millis(25));
    let gliding = offset(&app);
    assert!(gliding > 20.0);

    touch(&mut app, Phase::Down, (x, y));
    assert_eq!(app.text_field().unwrap(), editor);
    assert!(!app.kernel().state_of("inner-item").unwrap().contains(State::PRESSED));
    clock.advance(Duration::from_millis(16));
    assert_eq!(offset(&app), gliding);
    touch(&mut app, Phase::Move, (x, y + 4.0));
    assert_eq!(offset(&app), gliding - 4.0, "the moving viewport follows the new contact immediately");
    touch(&mut app, Phase::Cancel, (x, y + 4.0));
    touch(&mut app, Phase::Move, (x, y + 30.0));
    touch(&mut app, Phase::Up, (x, y + 30.0));
    clock.advance(Duration::from_millis(250));
    assert_eq!(app.held_pointer(), None);
    assert_eq!(offset(&app), gliding - 4.0);
    assert_eq!(app.text_field().unwrap(), editor);
    assert_eq!(app.kernel().text_of("presses").as_deref(), Some("0"));
    assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
}

#[test]
fn a_press_outside_a_nested_glide_keeps_its_normal_activation_and_focus() {
    let mut app = nested_touch_window();
    let clock = app.app();
    let (x, y) = center(&app, "list");
    touch(&mut app, Phase::Down, (x, y));
    clock.advance(Duration::from_millis(20));
    touch(&mut app, Phase::Move, (x, y - 20.0));
    touch(&mut app, Phase::Up, (x, y - 20.0));
    clock.advance(Duration::from_millis(25));
    let gliding = offset(&app);
    assert!(gliding > 20.0);

    touch(&mut app, Phase::Down, (x, y + 80.0));
    touch(&mut app, Phase::Up, (x, y + 80.0));
    clock.advance(Duration::from_millis(250));
    assert_eq!(offset(&app), gliding);
    assert_eq!(app.inspect().focus.as_deref(), Some("outer-item"));
    assert_eq!(app.kernel().text_of("presses").as_deref(), Some("1"));
}

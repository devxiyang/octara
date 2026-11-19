//! Activation is shared by semantic and accessibility input; retiring focus is a notification.
use octara::accesskit::{Action, ActionData};
use octara::app::{Event, InputEvent};
use octara::kernel::{NumericRange, a11y::a11y_id};
use octara::platform::{HeadlessPlatform, WindowOptions};
use octara::prelude::*;

#[test]
fn accessibility_value_without_an_input_handler_preserves_selection() {
    let mut window = Window::headless(|| input().id("field").value("initial"));
    window.dispatch(Event::Focus("field".into())).unwrap();
    window.set_selection(1, 3);
    let node = window.kernel().find("field").unwrap();
    let before = window.inspect().text_input;
    assert!(!window.event(InputEvent::Accessibility {
        target: a11y_id(node),
        action: Action::SetValue,
        data: Some(ActionData::Value("replacement".into())),
    }));
    assert_eq!(window.kernel().selection(node), (1, 3));
    assert_eq!(window.inspect().text_input, before);
}

#[test]
fn accessibility_value_runs_the_input_handler_and_updates_selection() {
    let mut window = Window::headless(|| {
        let value = state("value", || "initial".to_owned());
        input().id("field").value(value).on_input(move |text| value.set(text))
    });
    window.dispatch(Event::Focus("field".into())).unwrap();
    window.set_selection(1, 3);
    let node = window.kernel().find("field").unwrap();
    for replacement in ["你好，世界😀", "你好", ""] {
        assert!(window.event(InputEvent::Accessibility {
            target: a11y_id(node),
            action: Action::SetValue,
            data: Some(ActionData::Value(replacement.into())),
        }));
        assert_eq!(window.kernel().value(node), replacement);
        assert_eq!(window.kernel().selection(node), (replacement.len(), replacement.len()));
    }
    assert!(window.app().diagnostics().is_empty());
}

fn fixture() -> (App, Window, Signal<u8>, Signal<u32>) {
    let app = App::new(HeadlessPlatform::default());
    let (mode, hits) = app.run(|| (state("mode", || 0u8), state("hits", || 0u32)));
    let window = app
        .open_window(WindowOptions::default(), move || {
            col()
                .size_full()
                .child(
                    col()
                        .style_with(move |s| if mode.get() == 2 { s.hidden() } else { s.flex() })
                        .child(
                            el().id("button")
                                .size(60)
                                .focusable()
                                .disabled(move || mode.get() == 1)
                                .on_press(move || hits.update(|n| *n += 1))
                                .on_toggle(move || hits.update(|n| *n += 1))
                                .on_secondary_press(move || hits.update(|n| *n += 1)),
                        )
                        .child(
                            input()
                                .id("field")
                                .value("initial")
                                .disabled(move || mode.get() == 1)
                                .on_input(move |_| hits.update(|n| *n += 1)),
                        )
                        .child(
                            el().id("range")
                                .size(60)
                                .disabled(move || mode.get() == 1)
                                .numeric_range(NumericRange { value: Some(1.0), min: 0.0, max: 10.0, step: Some(1.0) })
                                .on_numeric_input(move |_| hits.update(|n| *n += 1)),
                        ),
                )
                .child(show(move || mode.get() == 3, || el().modal(false, || {}).size(60)))
        })
        .unwrap();
    (app, window, mode, hits)
}

#[test]
fn semantic_activation_rejects_disabled_hidden_and_modal_inert_targets() {
    let (app, mut w, mode, hits) = fixture();
    for blocked in [1, 2, 3] {
        app.run(|| mode.set(blocked));
        for event in [
            Event::Press("button".into()),
            Event::Toggle("button".into()),
            Event::Input("field".into(), "changed".into()),
            Event::NumericInput("range".into(), 2.0),
        ] {
            assert!(w.dispatch(event.clone()).is_err(), "mode {blocked}: {event:?} must be refused");
        }
        assert_eq!(app.run(|| hits.get()), 0);
    }
    app.run(|| mode.set(0));
    w.dispatch(Event::Press("button".into())).unwrap();
    assert_eq!(app.run(|| hits.get()), 1, "a retained target becomes usable again");
    assert!(app.diagnostics().is_empty());
}

#[test]
fn accessibility_activation_refuses_without_mutating_selection() {
    let (app, mut w, mode, hits) = fixture();
    w.dispatch(Event::Focus("field".into())).unwrap();
    w.set_selection(1, 3);
    let field = w.kernel().find("field").unwrap();
    for blocked in [1, 2, 3] {
        app.run(|| mode.set(blocked));
        for (id, action, data) in [
            ("button", Action::Click, None),
            ("button", Action::ShowContextMenu, None),
            ("field", Action::SetValue, Some(ActionData::Value("replacement".into()))),
            ("range", Action::SetValue, Some(ActionData::NumericValue(2.0))),
        ] {
            let target = a11y_id(w.kernel().find(id).unwrap());
            assert!(!w.event(InputEvent::Accessibility { target, action, data }), "mode {blocked}: {action:?} must be refused");
        }
        assert_eq!(w.kernel().selection(field), (1, 3), "refused input must not move the caret");
        assert_eq!(app.run(|| hits.get()), 0);
    }
    app.run(|| mode.set(0));
    let target = a11y_id(w.kernel().find("button").unwrap());
    assert!(w.event(InputEvent::Accessibility { target, action: Action::Click, data: None }));
    assert_eq!(app.run(|| hits.get()), 1);
    assert!(app.diagnostics().is_empty());
}

#[test]
fn retiring_focus_notifies_even_after_disable_hide_or_modal_open() {
    for blocked in [1, 2, 3] {
        let app = App::new(HeadlessPlatform::default());
        let (mode, blurs) = app.run(|| (state("mode", || 0u8), state("blurs", || 0u32)));
        let mut w = app
            .open_window(WindowOptions::default(), move || {
                col()
                    .child(
                        input()
                            .id("field")
                            .value("unchanged")
                            .disabled(move || mode.get() == 1)
                            .style_with(move |s| if mode.get() == 2 { s.hidden() } else { s.flex() })
                            .on_blur(move || blurs.update(|n| *n += 1)),
                    )
                    .child(show(move || mode.get() == 3, || el().modal(false, || {})))
            })
            .unwrap();
        w.dispatch(Event::Focus("field".into())).unwrap();
        w.event(InputEvent::Composition("nihao".into()));
        app.run(|| mode.set(blocked));
        assert_eq!(app.run(|| blurs.get()), 1, "mode {blocked}: focus retirement must notify once");
        assert_eq!(w.inspect().focus, None);
        assert_eq!(w.kernel().value_of("field").as_deref(), Some("unchanged"));
        assert!(w.kernel().preedit(w.kernel().find("field").unwrap()).is_empty());
        app.poll();
        assert_eq!(app.run(|| blurs.get()), 1);
        assert!(app.diagnostics().is_empty());
    }
}

#[test]
fn a_disabled_modal_can_still_be_dismissed() {
    use octara::app::{Phase, PointerEvent};
    for outside in [false, true] {
        let app = App::new(HeadlessPlatform::default());
        let closed = app.run(|| state("closed", || false));
        let mut w = app
            .open_window(WindowOptions::default(), move || {
                col().size_full().child(show(
                    move || !closed.get(),
                    move || el().id("modal").size(100).disabled(true).modal(true, move || closed.set(true)),
                ))
            })
            .unwrap();
        if outside {
            w.event(InputEvent::Pointer(PointerEvent::mouse(Phase::Down, 700.0, 500.0)));
        } else {
            w.press_key("escape").unwrap();
        }
        assert!(app.run(|| closed.get()), "outside={outside}");
        assert!(app.diagnostics().is_empty());
    }
}

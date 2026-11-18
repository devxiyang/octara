//! Numeric ranges are readable, actionable and guarded like every other retained control.

use octara::accesskit::{Action, ActionData};
use octara::app::{Event, InputEvent};
use octara::inspect::{self, Delivery};
use octara::kernel::{NumericRange, a11y::a11y_id};
use octara::prelude::*;
use octara::serde_json::json;

#[ui]
fn ranges() -> View {
    let value = state("value", || 2.0);
    let disabled = state("disabled", || false);
    let replaced = state("replaced", || false);
    let known = state("known", || true);
    on_command("range.disable", move || disabled.set(true));
    on_command("range.replace", move || replaced.set(true));
    on_command("range.unknown", move || known.set(false));
    let control = move || {
        el().id("level")
            .role(Role::Slider)
            .label("Level")
            .focusable()
            .size(200.0)
            .disabled(move || disabled.get())
            .numeric_range(move || NumericRange { value: Some(value.get()), min: 0.0, max: 10.0, step: Some(0.5) })
            .on_numeric_input(move |next| value.set(next.clamp(0.0, 10.0)))
    };
    col()
        .size_full()
        .child(keyed(move || replaced.get(), move |_| control()))
        .child(el().id("progress").role(Role::ProgressIndicator).label("Download").numeric_range(move || NumericRange {
            value: known.get().then(|| value.get()),
            min: 0.0,
            max: 10.0,
            step: None,
        }))
        .child(text(move || value.get().to_string()).id("current"))
        .child(text_input().id("plain").label("Ordinary text").w(200.0))
        .into()
}

fn request(window: &mut Window, value: serde_json::Value) -> inspect::Reply {
    inspect::handle_line(window, &value.to_string())
}

fn numeric(window: &Window) -> f64 {
    window.inspect().find("level").unwrap().numeric_value.unwrap()
}

#[test]
fn numeric_semantics_update_in_place_and_unknown_progress_omits_the_current_number() {
    let mut window = Window::headless(ranges);
    let first = window.inspect();
    let level = first.find("level").unwrap();
    assert_eq!(
        (level.numeric_value, level.min_numeric_value, level.max_numeric_value, level.numeric_value_step),
        (Some(2.0), Some(0.0), Some(10.0), Some(0.5))
    );
    for action in ["set_value", "increment", "decrement"] {
        assert!(level.actions.iter().any(|found| found == action));
    }
    assert!(first.find("progress").unwrap().actions.is_empty());
    let plain = serde_json::to_value(first.find("plain").unwrap()).unwrap();
    for property in ["numeric_value", "min_numeric_value", "max_numeric_value", "numeric_value_step"] {
        assert!(plain.get(property).is_none(), "ordinary nodes omit {property}");
    }
    let node = a11y_id(window.kernel().find("level").unwrap());
    let initial = window.accessibility_update();
    let ax = &initial.nodes.iter().find(|(id, _)| *id == node).unwrap().1;
    assert_eq!(
        (ax.numeric_value(), ax.min_numeric_value(), ax.max_numeric_value(), ax.numeric_value_step()),
        (Some(2.0), Some(0.0), Some(10.0), Some(0.5))
    );
    assert!(ax.supports_action(Action::SetValue) && ax.supports_action(Action::Increment) && ax.supports_action(Action::Decrement));
    window.dispatch(Event::NumericInput("level".into(), 4.0)).unwrap();
    assert_eq!(window.inspect().find("level").unwrap().target, level.target);
    let changed = window.accessibility_update();
    assert_eq!(changed.nodes.iter().find(|(id, _)| *id == node).unwrap().1.numeric_value(), Some(4.0));
    window.invoke("range.unknown").unwrap();
    let unknown = window.inspect();
    let progress = serde_json::to_value(unknown.find("progress").unwrap()).unwrap();
    assert!(progress.get("numeric_value").is_none());
    assert_eq!(progress["max_numeric_value"], json!(10.0));
}

#[test]
fn accessibility_sets_and_steps_numbers_but_disabled_and_nonfinite_changes_are_refused() {
    let mut window = Window::headless(ranges);
    let node = a11y_id(window.kernel().find("level").unwrap());
    let event = |action, data| InputEvent::Accessibility { target: node, action, data };
    assert!(window.event(event(Action::Increment, None)));
    assert_eq!(numeric(&window), 2.5);
    assert!(window.event(event(Action::Decrement, None)));
    assert_eq!(numeric(&window), 2.0);
    assert!(window.event(event(Action::SetValue, Some(ActionData::NumericValue(8.0)))));
    assert_eq!(numeric(&window), 8.0);
    assert!(window.event(event(Action::SetValue, Some(ActionData::Value("3.5".into())))));
    assert_eq!(numeric(&window), 3.5);
    assert!(!window.event(event(Action::SetValue, Some(ActionData::NumericValue(f64::NAN)))));
    assert!(!window.event(event(Action::SetValue, Some(ActionData::Value("NaN".into())))));
    assert!(window.dispatch(Event::NumericInput("level".into(), f64::INFINITY)).is_err());
    window.invoke("range.disable").unwrap();
    assert!(!window.event(event(Action::Increment, None)));
    assert!(!window.event(event(Action::SetValue, Some(ActionData::NumericValue(1.0)))));
    assert!(window.dispatch(Event::NumericInput("level".into(), 1.0)).is_err());
    assert_eq!(numeric(&window), 3.5);
}

#[test]
fn inspector_set_value_uses_fresh_identity_and_reports_delivery_without_retargeting() {
    let mut window = Window::headless(ranges);
    let token = window.inspect().find("level").unwrap().target.clone();
    let reply = request(&mut window, json!({"op":"set_value","id":"level","value":7,"target":token}));
    assert!(reply.ok, "{reply:?}");
    assert_eq!(reply.delivery, Some(Delivery::Delivered));
    assert_eq!(numeric(&window), 7.0);
    window.invoke("range.replace").unwrap();
    let stale = request(&mut window, json!({"op":"set_value","id":"level","value":1,"target":token}));
    assert_eq!(stale.code.as_deref(), Some("STALE_TARGET"));
    assert_eq!(stale.delivery, Some(Delivery::Refused));
    assert_eq!(numeric(&window), 7.0);
    let token = window.inspect().find("level").unwrap().target.clone();
    window.invoke("range.disable").unwrap();
    let disabled = request(&mut window, json!({"op":"set_value","id":"level","value":1,"target":token}));
    assert!(!disabled.ok);
    assert_eq!(disabled.delivery, Some(Delivery::Refused));
    assert_eq!(numeric(&window), 7.0);
    let malformed = inspect::handle_line(&mut window, r#"{"op":"set_value","id":"level","value":1e999}"#);
    assert_eq!(malformed.code.as_deref(), Some("BAD_REQUEST"));
}

#[test]
fn numeric_requests_do_not_replace_a_text_fields_active_composition() {
    let mut window = Window::headless(|| {
        let text = state("text", || "2".to_owned());
        col().size_full().child(
            text_input()
                .id("number")
                .label("Number")
                .w(200.0)
                .value(move || text.get())
                .on_input(move |value| text.set(value))
                .numeric_range(NumericRange { value: Some(2.0), min: 0.0, max: 10.0, step: Some(1.0) })
                .on_numeric_input(move |value| text.set(value.to_string())),
        )
    });
    window.dispatch(Event::Focus("number".into())).unwrap();
    window.set_selection(0, 1);
    window.event(InputEvent::Preedit { text: "候选".into(), selection: Some((0, 6)) });
    let before = window.inspect();
    let reply = request(&mut window, json!({"op":"set_value","id":"number","value":3,"target":before.find("number").unwrap().target}));
    assert!(!reply.ok);
    assert_eq!(reply.delivery, Some(Delivery::Refused));
    let node = a11y_id(window.kernel().find("number").unwrap());
    assert!(!window.event(InputEvent::Accessibility { target: node, action: Action::SetValue, data: Some(ActionData::NumericValue(4.0)) }));
    assert_eq!(window.text_field().unwrap().composition, "候选");
    assert_eq!(window.text_field().unwrap().selection, 0..1);
    assert_eq!(window.kernel().value_of("number").as_deref(), Some("2"));
}

#[test]
fn numeric_input_cannot_reach_hidden_leaving_or_modal_inert_controls() {
    let mut window = Window::headless(|| {
        let value = state("value", || 2.0);
        let hidden = state("hidden", || false);
        let present = state("present", || true);
        let modal = state("modal", || false);
        on_command("hide", move || hidden.set(true));
        on_command("show", move || hidden.set(false));
        on_command("remove", move || present.set(false));
        on_command("modal.open", move || modal.set(true));
        on_command("modal.close", move || modal.set(false));
        col()
            .size_full()
            .child(show(
                move || present.get(),
                move || {
                    el().id("level")
                        .label("Level")
                        .role(Role::Slider)
                        .size_8()
                        .motion(Spring::SNAP)
                        .style_with(move |style| if hidden.get() { style.hidden() } else { style.flex() })
                        .numeric_range(move || NumericRange { value: Some(value.get()), min: 0.0, max: 10.0, step: Some(1.0) })
                        .on_numeric_input(move |next| value.set(next))
                },
            ))
            .child(show(move || modal.get(), || el().role(Role::Dialog).modal(false, || {}).child(el().focusable().size_8())))
            .child(text(move || value.get().to_string()).id("current"))
    });
    let node = window.kernel().find("level").unwrap();
    let ax = a11y_id(node);
    for command in ["hide", "modal.open", "remove"] {
        window.invoke(command).unwrap();
        for action in [Action::SetValue, Action::Increment, Action::Decrement] {
            assert!(
                !window.event(InputEvent::Accessibility {
                    target: ax,
                    action,
                    data: (action == Action::SetValue).then_some(ActionData::NumericValue(8.0))
                }),
                "{command}: {action:?}"
            );
        }
        assert!(window.dispatch(Event::NumericInput("level".into(), 8.0)).is_err(), "{command}");
        assert_eq!(window.kernel().text_of("current").as_deref(), Some("2"));
        if command == "hide" {
            window.invoke("show").unwrap();
        } else if command == "modal.open" {
            window.invoke("modal.close").unwrap();
        }
    }
    assert!(window.kernel().is_alive(node) && window.kernel().is_exiting(node), "the refused AT target really is retained while leaving");
}

#[test]
fn inspector_drag_keeps_its_target_and_uses_four_timed_framework_moves() {
    let mut window = Window::headless(|| {
        let moved = state("moved", Vec::<f32>::new);
        let ended = state("ended", || false);
        col()
            .size_full()
            .child(el().id("target").w(200.0).h(60.0).on_drag(move |event| match event.phase {
                DragPhase::Move => moved.update(|moves| moves.push(event.dx)),
                DragPhase::End => ended.set(true),
                _ => {}
            }))
            .child(text(move || format!("{:?};{}", moved.get(), ended.get())).id("log"))
    });
    let target = window.inspect().find("target").unwrap().target.clone();
    let before = window.app().now();
    let reply = request(&mut window, json!({"op":"drag","id":"target","dx":80,"dy":0,"target":target}));
    assert!(reply.ok, "{reply:?}");
    assert_eq!(reply.delivery, Some(Delivery::Delivered));
    assert_eq!(window.app().now() - before, std::time::Duration::from_millis(64));
    assert_eq!(window.kernel().text_of("log").as_deref(), Some("[40.0, 60.0, 80.0];true"), "the first pointer move emits DragPhase::Start");
    assert!(window.held_pointer().is_none());
    let invalid = inspect::handle(&mut window, inspect::Request::Drag { id: "target".into(), dx: f32::NAN, dy: 0.0 });
    assert_eq!(invalid.code.as_deref(), Some("BAD_REQUEST"));
    assert_eq!(invalid.delivery, Some(Delivery::Refused));
}

#[test]
fn inspector_drag_cancels_a_replaced_allocation_and_never_moves_its_replacement() {
    let mut window = Window::headless(|| {
        let replaced = state("replaced", || false);
        let replacement_events = state("replacement_events", || 0);
        col()
            .size_full()
            .child(keyed(
                move || replaced.get(),
                move |replacement| {
                    let target = el().id("target").w(200.0).h(60.0);
                    if *replacement {
                        target.on_drag(move |_| replacement_events.update(|events| *events += 1))
                    } else {
                        target.on_drag(move |_| replaced.set(true))
                    }
                },
            ))
            .child(text(move || replacement_events.get().to_string()).id("log"))
    });
    let target = window.inspect().find("target").unwrap().target.clone();
    let reply = request(&mut window, json!({"op":"drag","id":"target","dx":80,"dy":0,"target":target}));
    assert_eq!(reply.code.as_deref(), Some("STALE_TARGET"));
    assert_eq!(reply.delivery, Some(Delivery::Delivered));
    assert_eq!(window.kernel().text_of("log").as_deref(), Some("0"));
    assert!(window.held_pointer().is_none());
}

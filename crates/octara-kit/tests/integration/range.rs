//! Numeric controls keep their model, editor and accessible range consistent.

use octara::accesskit::{Action, ActionData};
use octara::app::{Event, InputEvent, Phase, PointerEvent};
use octara::inspect;
use octara::kernel::a11y::a11y_id;
use octara::prelude::*;
use octara::serde_json::json;
use octara_kit::*;

fn clean(window: &Window) {
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
}

#[ui]
fn controls() -> View {
    let level = state("level", || 2.0);
    let raw = state("raw", || "2.00".to_owned());
    let disabled = state("disabled", || false);
    let progress_value = state("progress", || Some(25.0));
    let detailed = state("detailed", || false);
    on_command("range.disable", move || disabled.set(true));
    on_command("range.enable", move || disabled.set(false));
    on_command("range.unknown", move || progress_value.set(None));
    on_command("range.complete", move || progress_value.set(Some(500.0)));
    on_command("range.help", move || detailed.set(true));
    col()
        .size_full()
        .p_4()
        .gap_3()
        .child(
            slider("level", (move || level.get(), move |value| level.set(value)), 0.0, 10.0, 0.5)
                .label("Level")
                .disabled(move || disabled.get()),
        )
        .child(field("quantity", "Quantity", number_field(raw, 0.0, 10.0, 0.1).w(200.0).disabled(move || disabled.get()), move || {
            FieldInfo {
                description: if detailed.get() { "Use a number between zero and ten." } else { "A quantity." }.into(),
                error: parse_number(&raw.get())
                    .filter(|value| (0.0..=10.0).contains(value))
                    .is_none()
                    .then(|| "Enter a number from zero to ten.".into()),
                required: true,
            }
        }))
        .child(progress(move || progress_value.get(), 100.0).id("download").label("Download"))
        .child(button("Outside").id("outside"))
        .child(text(move || level.get().to_string()).id("chosen"))
        .into()
}

fn numeric(window: &Window, id: &str) -> Option<f64> {
    window.inspect().find(id).unwrap().numeric_value
}

fn value(window: &Window, id: &str) -> String {
    window.kernel().value_of(id).unwrap()
}

fn replace(window: &mut Window, text: &str) {
    window.dispatch(Event::Focus("quantity".into())).unwrap();
    window.press_key("mod+a").unwrap();
    window.type_text(text);
}

#[test]
fn finite_number_parsing_and_committing_preserve_invalid_drafts_and_decimal_steps() {
    for raw in ["", " ", "-", "1e", "12px", "候选", "NaN", "inf", "-inf", "1e999"] {
        assert_eq!(parse_number(raw), None, "{raw:?}");
        assert_eq!(normalize_number(raw, -10.0, 10.0, 0.1), None);
    }
    assert_eq!(parse_number("  -1.25e1 "), Some(-12.5));
    assert_eq!(normalize_number(" 0.3000 ", 0.0, 1.0, 0.1).as_deref(), Some("0.3"));
    assert_eq!(normalize_number("0.26", 0.0, 1.0, 0.1).as_deref(), Some("0.3"));
    assert_eq!(normalize_number("100", 0.0, 10.0, 0.1).as_deref(), Some("10"));
    assert_eq!(normalize_number("-100", -1.0, 10.0, 0.1).as_deref(), Some("-1"));
    assert_eq!(normalize_number("0.2", 0.05, 1.0, 0.1).as_deref(), Some("0.25"));
}

#[test]
fn slider_keyboard_and_semantic_values_share_bounds_steps_and_identity() {
    let mut window = Window::headless(controls);
    let initial = window.inspect();
    let level = initial.find("level").unwrap();
    assert_eq!(level.role, "slider");
    assert_eq!(
        (level.numeric_value, level.min_numeric_value, level.max_numeric_value, level.numeric_value_step),
        (Some(2.0), Some(0.0), Some(10.0), Some(0.5))
    );
    window.dispatch(Event::Focus("level".into())).unwrap();
    for (key, wanted) in [
        ("right", 2.5),
        ("up", 3.0),
        ("left", 2.5),
        ("down", 2.0),
        ("pageup", 7.0),
        ("pagedown", 2.0),
        ("end", 10.0),
        ("right", 10.0),
        ("home", 0.0),
        ("left", 0.0),
    ] {
        window.press_key(key).unwrap();
        assert_eq!(numeric(&window, "level"), Some(wanted), "{key}");
    }
    window.press_key("mod+right").unwrap();
    assert_eq!(numeric(&window, "level"), Some(0.0), "modified editing keys do not step");
    let reply = inspect::handle_line(&mut window, &json!({"op":"set_value","id":"level","value":6.3,"target":level.target}).to_string());
    assert!(reply.ok, "{reply:?}");
    assert_eq!(numeric(&window, "level"), Some(6.5));
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("6.5"), "the caller's tuple model owns the number");
    assert_eq!(window.inspect().find("level").unwrap().target, level.target);
    window.invoke("range.disable").unwrap();
    window.press_key("right").unwrap();
    assert!(window.dispatch(Event::NumericInput("level".into(), 0.0)).is_err());
    assert_eq!(numeric(&window, "level"), Some(6.5));
    clean(&window);
}

#[test]
fn slider_track_taps_work_for_mouse_and_touch_without_reusing_a_point_for_keyboard_activation() {
    let mut window = Window::headless(controls);
    let (x, y, width, height) = {
        let kernel = window.kernel();
        kernel.rect(kernel.find("level").unwrap())
    };
    window.click_at(x + width - 2.0, y + height / 2.0);
    assert_eq!(numeric(&window, "level"), Some(10.0));
    window.event(InputEvent::Pointer(PointerEvent::touch(7, Phase::Down, x + 2.0, y + height / 2.0)));
    window.event(InputEvent::Pointer(PointerEvent::touch(7, Phase::Up, x + 2.0, y + height / 2.0)));
    assert_eq!(numeric(&window, "level"), Some(0.0));
    window.dispatch(Event::NumericInput("level".into(), 4.0)).unwrap();
    window.press_key("enter").unwrap();
    assert_eq!(numeric(&window, "level"), Some(4.0), "keyboard activation has no pointer coordinate");
    let node = a11y_id(window.kernel().find("level").unwrap());
    window.event(InputEvent::Accessibility { target: node, action: Action::Click, data: None });
    assert_eq!(numeric(&window, "level"), Some(4.0), "AT activation has no stale pointer coordinate");
    clean(&window);
}

#[test]
fn slider_drag_cancel_restores_the_model_and_disabling_during_a_drag_stops_later_changes() {
    let mut window = Window::headless(controls);
    let (x, y, width, height) = {
        let kernel = window.kernel();
        kernel.rect(kernel.find("level").unwrap())
    };
    let at = (x + width / 2.0, y + height / 2.0);
    window.event(InputEvent::Pointer(PointerEvent::mouse(Phase::Down, at.0, at.1)));
    window.event(InputEvent::Pointer(PointerEvent::mouse(Phase::Move, at.0 + 30.0, at.1)));
    assert_ne!(numeric(&window, "level"), Some(2.0));
    window.event(InputEvent::Pointer(PointerEvent::mouse(Phase::Cancel, at.0 + 30.0, at.1)));
    assert_eq!(numeric(&window, "level"), Some(2.0));
    window.event(InputEvent::Pointer(PointerEvent::mouse(Phase::Down, at.0, at.1)));
    window.event(InputEvent::Pointer(PointerEvent::mouse(Phase::Move, at.0 + 20.0, at.1)));
    let before = numeric(&window, "level");
    window.invoke("range.disable").unwrap();
    window.event(InputEvent::Pointer(PointerEvent::mouse(Phase::Move, at.0 + 60.0, at.1)));
    window.event(InputEvent::Pointer(PointerEvent::mouse(Phase::Up, at.0 + 60.0, at.1)));
    assert_eq!(numeric(&window, "level"), before);
    assert!(window.held_pointer().is_none());
    clean(&window);
}

#[test]
fn number_field_keeps_raw_drafts_then_commits_only_complete_numbers_on_enter_or_blur() {
    let mut window = Window::headless(controls);
    let identity = window.inspect().find("quantity").unwrap().target.clone();
    assert_eq!(value(&window, "quantity"), "2.00", "construction does not normalize caller state");
    replace(&mut window, "1e");
    assert_eq!(value(&window, "quantity"), "1e");
    assert_eq!(numeric(&window, "quantity"), None);
    assert_eq!(window.inspect().find("quantity").unwrap().invalid, Some(true));
    window.press_key("enter").unwrap();
    assert_eq!(value(&window, "quantity"), "1e");
    window.tap("outside").unwrap();
    assert_eq!(value(&window, "quantity"), "1e", "invalid blur leaves the raw draft available to fix");
    replace(&mut window, " 0.3000 ");
    assert_eq!(value(&window, "quantity"), " 0.3000 ");
    window.press_key("enter").unwrap();
    assert_eq!(value(&window, "quantity"), "0.3");
    replace(&mut window, "99.00");
    assert_eq!(value(&window, "quantity"), "99.00");
    window.tap("outside").unwrap();
    assert_eq!(value(&window, "quantity"), "10");
    replace(&mut window, "0.2");
    window.press_key("up").unwrap();
    assert_eq!(value(&window, "quantity"), "0.3", "decimal stepping has no binary tail");
    window.press_key("down").unwrap();
    assert_eq!(value(&window, "quantity"), "0.2");
    assert_eq!(window.inspect().find("quantity").unwrap().target, identity);
    clean(&window);
}

#[test]
fn number_field_preserves_the_editor_and_ime_while_field_help_changes_or_navigation_is_pressed() {
    let mut window = Window::headless(controls);
    window.dispatch(Event::Focus("quantity".into())).unwrap();
    window.set_selection(0, 4);
    window.event(InputEvent::Preedit { text: "拼音".into(), selection: Some((0, 6)) });
    let before = window.text_field().unwrap();
    let target = window.inspect().find("quantity").unwrap().target.clone();
    window.invoke("range.help").unwrap();
    for key in ["up", "down", "home", "end", "pageup", "pagedown", "tab", "space", "enter"] {
        window.press_key(key).unwrap();
        let after = window.text_field().expect("IME navigation keeps the editor focused");
        assert_eq!(after.text, before.text, "{key}");
        assert_eq!(after.selection, before.selection, "{key}");
        assert_eq!(after.composition, before.composition, "{key}");
        assert_eq!(after.composition_selection, before.composition_selection, "{key}");
    }
    assert!(window.dispatch(Event::NumericInput("quantity".into(), 5.0)).is_err());
    window.event(InputEvent::Preedit { text: String::new(), selection: None });
    assert_eq!(window.text_field().unwrap().selection, 0..4);
    assert_eq!(window.inspect().find("quantity").unwrap().target, target);
    let snapshot = window.inspect();
    let field = snapshot.find("quantity").unwrap();
    assert_eq!(field.required, Some(true));
    assert_eq!(field.description.as_deref(), Some("Use a number between zero and ten."));
    window.type_text("三");
    assert_eq!(value(&window, "quantity"), "三", "Chinese committed text remains a correctable invalid draft");
    window.press_key("enter").unwrap();
    assert_eq!(value(&window, "quantity"), "三");
    clean(&window);
}

#[test]
fn number_field_at_and_agent_requests_use_the_same_finite_snapping_and_disabled_rule() {
    let mut window = Window::headless(controls);
    let node = a11y_id(window.kernel().find("quantity").unwrap());
    let event = |action, data| InputEvent::Accessibility { target: node, action, data };
    assert!(window.event(event(Action::SetValue, Some(ActionData::NumericValue(0.26)))));
    assert_eq!(value(&window, "quantity"), "0.3");
    assert!(window.event(event(Action::Increment, None)));
    assert_eq!(value(&window, "quantity"), "0.4");
    assert!(window.event(event(Action::Decrement, None)));
    assert_eq!(value(&window, "quantity"), "0.3");
    window.invoke("range.disable").unwrap();
    assert!(!window.event(event(Action::SetValue, Some(ActionData::NumericValue(2.0)))));
    assert_eq!(value(&window, "quantity"), "0.3");
    clean(&window);
}

#[test]
fn number_field_blur_cancels_composition_before_committing_the_prior_raw_number() {
    let mut window = Window::headless(controls);
    window.dispatch(Event::Focus("quantity".into())).unwrap();
    window.set_selection(0, 4);
    window.event(InputEvent::Preedit { text: "中文草稿".into(), selection: Some((0, 12)) });
    assert_eq!(value(&window, "quantity"), "2.00");
    window.tap("outside").unwrap();
    assert_eq!(value(&window, "quantity"), "2", "core cancels preedit first; blur normalizes only the committed number");
    window.dispatch(Event::Focus("quantity".into())).unwrap();
    assert!(window.text_field().unwrap().composition.is_empty());
    clean(&window);
}

#[test]
fn progress_switches_between_finite_and_unknown_without_changing_its_identity_or_gaining_actions() {
    let mut window = Window::headless(controls);
    let first = window.inspect();
    let progress = first.find("download").unwrap();
    assert_eq!(progress.role, "progress_indicator");
    assert_eq!((progress.numeric_value, progress.min_numeric_value, progress.max_numeric_value), (Some(25.0), Some(0.0), Some(100.0)));
    assert!(progress.actions.is_empty());
    window.invoke("range.unknown").unwrap();
    assert_eq!(numeric(&window, "download"), None);
    assert_eq!(window.inspect().find("download").unwrap().target, progress.target);
    assert_eq!(
        window.inspect().root.walk().into_iter().filter(|node| node.role == "progress_indicator").count(),
        1,
        "decorative spinner is hidden from accessibility"
    );
    let node = a11y_id(window.kernel().find("download").unwrap());
    let tree = window.accessibility_tree();
    let ax = &tree.nodes.iter().find(|(id, _)| *id == node).unwrap().1;
    assert_eq!(ax.numeric_value(), None);
    assert!(!ax.supports_action(Action::SetValue));
    window.invoke("range.complete").unwrap();
    assert_eq!(numeric(&window, "download"), Some(100.0));
    window.resize(390.0, 640.0);
    assert_eq!(window.inspect().find("download").unwrap().target, progress.target);
    clean(&window);
}

#[test]
#[should_panic(expected = "finite min < max")]
fn slider_rejects_invalid_static_bounds() {
    Window::headless(|| {
        let value = state("value", || 0.0);
        slider("bad", value, 1.0, 1.0, 0.1)
    });
}

#[test]
#[should_panic(expected = "positive finite step")]
fn number_field_rejects_a_nonpositive_step() {
    Window::headless(|| {
        let value = state("value", || "0".to_owned());
        number_field(value, 0.0, 1.0, 0.0)
    });
}

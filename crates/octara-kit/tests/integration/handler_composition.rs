use octara::prelude::*;
use octara_kit::{PopupItem, popup_list, text_field};
use std::cell::RefCell;
use std::rc::Rc;

#[test]
fn text_field_updates_its_model_before_the_user_input_callback() {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let saved = seen.clone();
    let mut window = Window::headless(move || {
        let value = state("value", || "initial".to_owned());
        let saved = saved.clone();
        text_field(value)
            .id("field")
            .on_input(|_| panic!("replaced user callback"))
            .on_input(move |text| saved.borrow_mut().push((value.get(), text)))
            .into()
    });
    window.dispatch(Event::Input("field".into(), "你好".into())).unwrap();
    assert_eq!(&*seen.borrow(), &[("你好".to_owned(), "你好".to_owned())]);
    assert_eq!(window.inspect().find("field").unwrap().value.as_deref(), Some("你好"));
    let field = window.kernel().find("field").unwrap();
    assert!(window.event(InputEvent::Accessibility {
        target: octara::kernel::a11y::a11y_id(field),
        action: octara::accesskit::Action::SetValue,
        data: Some(octara::accesskit::ActionData::Value("accessible".into())),
    }));
    assert_eq!(seen.borrow().last(), Some(&("accessible".to_owned(), "accessible".to_owned())));
    assert_eq!(window.kernel().value(field), "accessible");
    assert!(window.app().diagnostics().is_empty());
}

#[test]
fn popup_preserves_editor_callbacks_and_consumes_only_its_keys() {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let saved = seen.clone();
    let mut window = Window::headless(move || {
        let value = state("value", || "abc".to_owned());
        let open = state("open", || false);
        let (keys, blurs, picks) = (saved.clone(), saved.clone(), saved.clone());
        col()
            .size_full()
            .child(popup_list(
                "field",
                "Suggestions",
                open,
                text_field(value)
                    .on_key(move |event| {
                        keys.borrow_mut().push(format!("key:{:?}", event.key));
                        false
                    })
                    .on_blur(move || blurs.borrow_mut().push(format!("blur:{}", open.get()))),
                || vec![PopupItem::new("one", "One"), PopupItem::new("two", "Two")],
                move |id| picks.borrow_mut().push(format!("pick:{id}")),
            ))
            .child(text_input().id("other").value("other"))
            .into()
    });
    window.dispatch(Event::Focus("field".into())).unwrap();
    window.set_selection(3, 3);
    window.press_key("left").unwrap();
    assert_eq!(&*seen.borrow(), &["key:Left"]);
    let field = window.kernel().find("field").unwrap();
    assert_eq!(window.kernel().selection(field), (2, 2), "unconsumed keys still reach the editor");
    window.press_key("down").unwrap();
    window.press_key("down").unwrap();
    window.press_key("enter").unwrap();
    assert_eq!(&*seen.borrow(), &["key:Left", "pick:two"], "popup keys do not reach the user handler");
    window.press_key("down").unwrap();
    window.dispatch(Event::Focus("other".into())).unwrap();
    assert_eq!(&*seen.borrow(), &["key:Left", "pick:two", "blur:false"], "popup closes before notifying blur");
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
}

#[test]
fn toggle_callbacks_observe_the_bound_value_on_pointer_keyboard_and_accessibility_routes() {
    let controls: [fn(Signal<bool>) -> El; 2] = [octara_kit::checkbox, octara_kit::switch];
    for control in controls {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let saved = seen.clone();
        let mut window = Window::headless(move || {
            let value = state("value", || false);
            let saved = saved.clone();
            control(value)
                .id("toggle")
                .on_toggle(|| panic!("replaced callback"))
                .on_toggle(move || saved.borrow_mut().push(value.get()))
                .into()
        });
        window.tap("toggle").unwrap();
        assert_eq!(&*seen.borrow(), &[true]);
        window.press_key("space").unwrap();
        assert_eq!(&*seen.borrow(), &[true, false]);
        let node = octara::kernel::a11y::a11y_id(window.kernel().find("toggle").unwrap());
        assert!(window.event(InputEvent::Accessibility { target: node, action: octara::accesskit::Action::Click, data: None }));
        assert_eq!(&*seen.borrow(), &[true, false, true]);
        window.dispatch(Event::Toggle("toggle".into())).unwrap();
        assert_eq!(&*seen.borrow(), &[true, false, true, false]);
        assert!(window.app().diagnostics().is_empty());
    }
}

#[test]
fn slider_preserves_numeric_pointer_drag_and_key_behavior_with_application_callbacks() {
    use octara::app::{Phase, PointerEvent};
    let seen = Rc::new(RefCell::new(Vec::new()));
    let saved = seen.clone();
    let mut window = Window::headless(move || {
        let value = state("value", || 2.0);
        let (numeric, press, drag, keys) = (saved.clone(), saved.clone(), saved.clone(), saved.clone());
        octara_kit::slider("slider", value, 0.0, 10.0, 0.5)
            .on_numeric_input(|_| panic!("replaced callback"))
            .on_numeric_input(move |requested| numeric.borrow_mut().push(format!("numeric:{requested}:{}", value.get())))
            .on_press(move || press.borrow_mut().push(format!("press:{}", value.get())))
            .on_drag(move |event| drag.borrow_mut().push(format!("drag:{:?}:{}", event.phase, value.get())))
            .on_key(move |event| {
                keys.borrow_mut().push(format!("key:{:?}", event.key));
                false
            })
            .into()
    });
    window.dispatch(Event::NumericInput("slider".into(), 6.3)).unwrap();
    assert_eq!(&*seen.borrow(), &["numeric:6.3:6.5"]);
    let node = octara::kernel::a11y::a11y_id(window.kernel().find("slider").unwrap());
    assert!(window.event(InputEvent::Accessibility {
        target: node,
        action: octara::accesskit::Action::SetValue,
        data: Some(octara::accesskit::ActionData::NumericValue(3.2)),
    }));
    assert_eq!(seen.borrow().last().unwrap(), "numeric:3.2:3");
    window.dispatch(Event::Focus("slider".into())).unwrap();
    seen.borrow_mut().clear();
    window.press_key("right").unwrap();
    assert_eq!(window.inspect().find("slider").unwrap().numeric_value, Some(3.5));
    assert!(seen.borrow().is_empty(), "stepping consumes the key");
    window.press_key("a").unwrap();
    assert_eq!(&*seen.borrow(), &["key:Char('a')"]);
    let (x, y, w, h) = {
        let k = window.kernel();
        k.rect(k.find("slider").unwrap())
    };
    window.click_at(x + w - 2.0, y + h / 2.0);
    assert_eq!(seen.borrow().last().unwrap(), "press:10");
    seen.borrow_mut().clear();
    let (x, y) = (x + w - 10.0, y + h / 2.0);
    window.event(InputEvent::Pointer(PointerEvent::mouse(Phase::Down, x, y)));
    window.event(InputEvent::Pointer(PointerEvent::mouse(Phase::Move, x - 50.0, y)));
    assert!(window.inspect().find("slider").unwrap().numeric_value.unwrap() < 10.0);
    assert!(seen.borrow().first().unwrap().starts_with("drag:Start:"));
    window.event(InputEvent::Pointer(PointerEvent::mouse(Phase::Cancel, x - 50.0, y)));
    assert_eq!(seen.borrow().last().unwrap(), "drag:Cancel:10");
    assert_eq!(window.inspect().find("slider").unwrap().numeric_value, Some(10.0));
    assert!(window.app().diagnostics().is_empty());
}

#[test]
fn number_callbacks_observe_commits_without_rewriting_drafts_or_intercepting_ime() {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let saved = seen.clone();
    let mut window = Window::headless(move || {
        let value = state("value", || "2.00".to_owned());
        let (input, submit, blur, numeric, keys) = (saved.clone(), saved.clone(), saved.clone(), saved.clone(), saved.clone());
        col()
            .size_full()
            .child(
                octara_kit::number_field(value, 0.0, 10.0, 0.5)
                    .id("number")
                    .on_input(move |_| input.borrow_mut().push(format!("input:{}", value.get())))
                    .on_submit(|| panic!("replaced callback"))
                    .on_submit(move || submit.borrow_mut().push(format!("submit:{}", value.get())))
                    .on_blur(move || blur.borrow_mut().push(format!("blur:{}", value.get())))
                    .on_numeric_input(move |v| numeric.borrow_mut().push(format!("numeric:{v}:{}", value.get())))
                    .on_key(move |event| {
                        keys.borrow_mut().push(format!("key:{:?}", event.key));
                        false
                    }),
            )
            .child(text_input().id("other"))
            .into()
    });
    window.dispatch(Event::Focus("number".into())).unwrap();
    window.dispatch(Event::Input("number".into(), " 6.30 ".into())).unwrap();
    assert_eq!(&*seen.borrow(), &["input: 6.30 "]);
    window.press_key("enter").unwrap();
    assert_eq!(seen.borrow().last().unwrap(), "submit:6.5");
    window.dispatch(Event::Input("number".into(), "99".into())).unwrap();
    window.dispatch(Event::Focus("other".into())).unwrap();
    assert_eq!(seen.borrow().last().unwrap(), "blur:10");
    window.dispatch(Event::Focus("number".into())).unwrap();
    window.dispatch(Event::Input("number".into(), "1e".into())).unwrap();
    window.press_key("enter").unwrap();
    assert_eq!(seen.borrow().last().unwrap(), "submit:1e");
    window.dispatch(Event::NumericInput("number".into(), 3.2)).unwrap();
    assert_eq!(seen.borrow().last().unwrap(), "numeric:3.2:3");
    seen.borrow_mut().clear();
    window.press_key("up").unwrap();
    assert_eq!(window.kernel().value_of("number").as_deref(), Some("3.5"));
    assert!(seen.borrow().is_empty());
    window.event(InputEvent::Preedit { text: "候选".into(), selection: Some((0, 6)) });
    let before = window.inspect().text_input;
    for key in ["up", "down", "enter"] {
        window.press_key(key).unwrap();
    }
    assert!(seen.borrow().is_empty());
    assert_eq!(window.inspect().text_input, before);
    assert!(window.app().diagnostics().is_empty());
}

#[test]
fn custom_models_can_reject_or_transform_values_before_application_notification() {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let saved = seen.clone();
    let mut window = Window::headless(move || {
        let level = state("level", || 1.0f64);
        let raw = state("raw", || "2.00".to_owned());
        let (toggle, numeric, submit) = (saved.clone(), saved.clone(), saved.clone());
        col()
            .size_full()
            .child(
                octara_kit::checkbox((|| false, |_| {}))
                    .id("toggle")
                    .on_toggle(move || toggle.borrow_mut().push("rejected toggle".to_owned())),
            )
            .child(
                octara_kit::slider("slider", (move || level.get(), move |v: f64| level.set(v.min(5.0))), 0.0, 10.0, 0.5)
                    .on_numeric_input(move |v| numeric.borrow_mut().push(format!("{v}:{}", level.get()))),
            )
            .child(
                octara_kit::number_field((move || raw.get(), |_| {}), 0.0, 10.0, 0.5)
                    .id("number")
                    .on_submit(move || submit.borrow_mut().push(raw.get())),
            )
            .into()
    });
    window.dispatch(Event::Toggle("toggle".into())).unwrap();
    assert_eq!(window.inspect().find("toggle").unwrap().checked, Some(false));
    window.dispatch(Event::NumericInput("slider".into(), 6.3)).unwrap();
    window.dispatch(Event::Focus("number".into())).unwrap();
    window.press_key("enter").unwrap();
    assert_eq!(&*seen.borrow(), &["rejected toggle", "6.3:5", "2.00"]);
    assert!(window.app().diagnostics().is_empty());
}

#[test]
fn a_model_setter_closing_the_window_skips_application_notification() {
    for kind in ["toggle", "numeric", "submit"] {
        let mut window = Window::headless(move || {
            let close = || {
                App::current().close_window(current_window());
            };
            match kind {
                "toggle" => octara_kit::checkbox((|| false, move |_| close()))
                    .id("control")
                    .on_toggle(|| panic!("notification after setter retired scope")),
                "numeric" => octara_kit::slider("control", (|| 2.0, move |_| close()), 0.0, 10.0, 0.5)
                    .on_numeric_input(|_| panic!("notification after setter retired scope")),
                "submit" => octara_kit::number_field((|| "2.00".to_owned(), move |_| close()), 0.0, 10.0, 0.5)
                    .id("control")
                    .on_submit(|| panic!("notification after setter retired scope")),
                _ => unreachable!(),
            }
            .into()
        });
        match kind {
            "toggle" => {
                window.dispatch(Event::Toggle("control".into())).unwrap();
            }
            "numeric" => {
                window.dispatch(Event::NumericInput("control".into(), 3.0)).unwrap();
            }
            "submit" => {
                window.dispatch(Event::Focus("control".into())).unwrap();
                window.press_key("enter").unwrap();
            }
            _ => unreachable!(),
        }
        assert!(window.is_closed(), "{kind}");
        assert!(window.app().diagnostics().is_empty(), "{kind}: {:?}", window.app().diagnostics());
    }
}

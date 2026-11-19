use octara::app::{Event, InputEvent};
use octara::prelude::*;
use octara_kit::{PopupItem, popup_list, text_area, text_field};
use std::cell::RefCell;
use std::rc::Rc;

#[test]
fn text_editors_update_their_models_before_the_user_input_callback() {
    let editors: [fn(Signal<String>) -> TextInput; 2] = [text_field, text_area];
    for editor in editors {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let saved = seen.clone();
        let mut window = Window::headless(move || {
            let value = state("value", || "initial".to_owned());
            let saved = saved.clone();
            editor(value)
                .id("field")
                .on_input(|_| panic!("replaced user callback"))
                .on_input(move |text| saved.borrow_mut().push((value.get(), text)))
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
            .child(input().id("other").value("other"))
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
            control(value).id("toggle").on_toggle(|| panic!("replaced callback")).on_toggle(move || saved.borrow_mut().push(value.get()))
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
        let (inputs, submit, blur, numeric, keys) = (saved.clone(), saved.clone(), saved.clone(), saved.clone(), saved.clone());
        col()
            .size_full()
            .child(
                octara_kit::number_field(value, 0.0, 10.0, 0.5)
                    .id("number")
                    .on_input(move |_| inputs.borrow_mut().push(format!("input:{}", value.get())))
                    .on_submit(|| panic!("replaced callback"))
                    .on_submit(move || submit.borrow_mut().push(format!("submit:{}", value.get())))
                    .on_blur(move || blur.borrow_mut().push(format!("blur:{}", value.get())))
                    .on_numeric_input(move |v| numeric.borrow_mut().push(format!("numeric:{v}:{}", value.get())))
                    .on_key(move |event| {
                        keys.borrow_mut().push(format!("key:{:?}", event.key));
                        false
                    }),
            )
            .child(input().id("other"))
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
                    .on_submit(|| panic!("notification after setter retired scope"))
                    .into(),
                _ => unreachable!(),
            }
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

#[test]
fn popover_preserves_trigger_callbacks_and_later_replacement_keeps_open_behavior() {
    for replace in [false, true] {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let saved = seen.clone();
        let mut window = Window::headless(move || {
            let open = state("open", || false);
            let (original, replacement) = (saved.clone(), saved.clone());
            let trigger = octara_kit::button("Details").on_press(move || original.borrow_mut().push(format!("original:{}", open.get())));
            let popup = octara_kit::popover("details", open, Placement::BELOW, trigger, || input().id("inside").value("draft").autofocus());
            let popup =
                if replace { popup.on_press(move || replacement.borrow_mut().push(format!("replacement:{}", open.get()))) } else { popup };
            col().size_full().p_4().child(popup)
        });
        window.tap("details").unwrap();
        assert!(window.kernel().exists("details-popover"));
        assert_eq!(window.inspect().focus.as_deref(), Some("inside"));
        assert_eq!(&*seen.borrow(), &[if replace { "replacement:true" } else { "original:true" }]);
        window.press_key("escape").unwrap();
        assert!(!window.kernel().exists("details-popover"));
        assert_eq!(window.inspect().focus.as_deref(), Some("details"));
        window.press_key("enter").unwrap();
        assert_eq!(seen.borrow().len(), 2);
        assert!(window.kernel().exists("details-popover"));
        assert!(window.app().diagnostics().is_empty());
    }
}

#[test]
fn menu_button_keeps_opening_navigation_and_deferred_commands_with_application_callbacks() {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let saved = seen.clone();
    let mut window = Window::headless(move || {
        let open = state("open", || false);
        let (press, keys, command) = (saved.clone(), saved.clone(), saved.clone());
        on_command("test.choose", move || command.borrow_mut().push(format!("command:open={}", open.get())));
        col()
            .size_full()
            .p_4()
            .gap_4()
            .child(
                octara_kit::menu_button("menu", "Actions", open, || Menu::new("Actions").item(MenuItem::action("Choose", "test.choose")))
                    .on_press(move || press.borrow_mut().push(format!("press:{}", open.get())))
                    .on_key(move |event| {
                        keys.borrow_mut().push(format!("key:{:?}", event.key));
                        false
                    }),
            )
            .child(input().id("editor").value("draft"))
    });
    window.dispatch(Event::Focus("editor".into())).unwrap();
    window.set_selection(1, 4);
    window.tap("menu").unwrap();
    assert_eq!(&*seen.borrow(), &["press:true"]);
    assert!(window.kernel().exists("menu-menu"));
    window.tap("menu-action-test.choose").unwrap();
    assert_eq!(&*seen.borrow(), &["press:true", "command:open=false"]);
    assert_eq!(window.inspect().focus.as_deref(), Some("editor"));
    assert_eq!(window.text_field().unwrap().selection, 1..4);
    window.dispatch(Event::Focus("menu".into())).unwrap();
    seen.borrow_mut().clear();
    window.press_key("a").unwrap();
    assert_eq!(&*seen.borrow(), &["key:Char('a')"]);
    seen.borrow_mut().clear();
    for key in ["up", "down"] {
        window.press_key(key).unwrap();
        assert_eq!(window.inspect().focus.as_deref(), Some("menu-action-test.choose"));
        assert!(seen.borrow().is_empty(), "menu opening keys are consumed");
        window.press_key("escape").unwrap();
        assert_eq!(window.inspect().focus.as_deref(), Some("menu"));
    }
    assert!(window.app().diagnostics().is_empty());
}

#[test]
fn popup_trigger_notification_can_close_the_window_after_its_model_updates() {
    for menu in [false, true] {
        let mut window = Window::headless(move || {
            let open = state("open", || false);
            let trigger = if menu {
                octara_kit::menu_button("trigger", "Actions", open, || Menu::new("Actions"))
            } else {
                octara_kit::popover("trigger", open, Placement::BELOW, octara_kit::button("Details"), || "Details")
            };
            trigger.on_press(move || {
                assert!(open.get(), "the component updates before notifying");
                App::current().close_window(current_window());
            })
        });
        window.tap("trigger").unwrap();
        assert!(window.is_closed());
        assert!(window.app().diagnostics().is_empty());
    }
}

#[test]
fn retiring_from_a_kit_content_factory_skips_its_conversion() {
    use octara_kit::{DialogOptions, button, collapsible, dialog, popover};

    struct Unconverted;

    impl IntoView for Unconverted {
        fn into_view(self, _: &'static std::panic::Location<'static>) -> View {
            panic!("a retired content factory must not convert its result");
        }
    }

    fn retire() -> Unconverted {
        watch(|| (), |_, _| close_window(current_window()));
        Unconverted
    }

    for name in ["collapsible", "dialog", "popover"] {
        let window = Window::headless(move || {
            let open = state("open", || true);
            match name {
                "collapsible" => collapsible(name, open, "Details", retire).into(),
                "dialog" => dialog(name, "Details", open, DialogOptions::default(), retire),
                "popover" => popover(name, open, Placement::BELOW, button("Details"), retire).into(),
                _ => unreachable!(),
            }
        });
        assert!(window.is_closed(), "{name}");
        assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
        window.app().shutdown();
    }
}

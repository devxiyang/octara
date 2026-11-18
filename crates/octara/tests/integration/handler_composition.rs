use octara::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

#[test]
fn behavior_layers_keep_order_and_replace_callbacks_only_within_their_own_layer() {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let saved = seen.clone();
    let mut window = Window::headless(move || {
        let (first, second, user) = (saved.clone(), saved.clone(), saved.clone());
        text_input()
            .id("field")
            .value("initial")
            .on_input(|_| panic!("replaced callback"))
            .behavior(|b| {
                b.on_input(|_| panic!("replaced component callback")).on_input(move |s| first.borrow_mut().push(format!("first:{s}")))
            })
            .on_input(move |s| user.borrow_mut().push(format!("user:{s}")))
            .behavior(|b| b.on_input(move |s| second.borrow_mut().push(format!("second:{s}"))))
    });
    window.dispatch(Event::Input("field".into(), "text".into())).unwrap();
    assert_eq!(&*seen.borrow(), &["first:text", "second:text", "user:text"]);
}

#[test]
fn component_keys_consume_or_fall_through_and_ime_runs_first() {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let saved = seen.clone();
    let mut window = Window::headless(move || {
        let (first, second, user, parent) = (saved.clone(), saved.clone(), saved.clone(), saved.clone());
        col()
            .on_key(move |_| {
                parent.borrow_mut().push("parent");
                false
            })
            .child(
                text_input()
                    .id("field")
                    .value("abc")
                    .behavior(|b| {
                        b.on_key(move |_| {
                            first.borrow_mut().push("first");
                            false
                        })
                    })
                    .behavior(|b| {
                        b.on_key(move |e| {
                            second.borrow_mut().push("second");
                            e.key == Key::Left
                        })
                    })
                    .on_key(move |_| {
                        user.borrow_mut().push("user");
                        false
                    }),
            )
    });
    window.dispatch(Event::Focus("field".into())).unwrap();
    window.set_selection(1, 1);
    let field = window.kernel().find("field").unwrap();
    window.press_key("left").unwrap();
    assert_eq!(&*seen.borrow(), &["first", "second"]);
    assert_eq!(window.kernel().selection(field), (1, 1));
    seen.borrow_mut().clear();
    window.press_key("right").unwrap();
    assert_eq!(&*seen.borrow(), &["first", "second", "user", "parent"]);
    assert_eq!(window.kernel().selection(field), (2, 2));
    seen.borrow_mut().clear();
    window.event(InputEvent::Preedit { text: "候选".into(), selection: Some((0, 6)) });
    let before = window.inspect().text_input;
    for key in ["left", "right", "down", "up", "enter", "escape", "space", "tab"] {
        window.press_key(key).unwrap();
    }
    assert!(seen.borrow().is_empty());
    assert_eq!(window.inspect().text_input, before);
    assert!(window.app().diagnostics().is_empty());
}

fn close_current() {
    App::current().close_window(current_window());
}

#[test]
fn reloading_from_a_key_callback_does_not_continue_on_the_replacement_tree() {
    let mut window = Window::headless(|| {
        text_input().id("field").value("abc").behavior(|b| b.on_key(|_| false)).on_key(|_| {
            App::current().hot_reload();
            false
        })
    });
    window.dispatch(Event::Focus("field".into())).unwrap();
    let before = window.inspect().find("field").unwrap().target.clone();
    window.press_key("right").unwrap();
    assert!(!window.is_closed());
    assert_ne!(window.inspect().find("field").unwrap().target, before);
    let field = window.kernel().find("field").unwrap();
    assert_eq!(window.kernel().selection(field), (0, 0), "no default edit on the replacement input");
    assert!(window.app().diagnostics().is_empty());
}

#[test]
fn closing_from_component_behavior_skips_later_callbacks_and_key_defaults() {
    for kind in ["input", "key", "blur"] {
        let mut window = Window::headless(move || {
            let input = text_input().id("field").value("abc");
            let input = match kind {
                "input" => input
                    .behavior(|b| b.on_input(|_| close_current()))
                    .behavior(|b| b.on_input(|_| panic!("late component input")))
                    .on_input(|_| panic!("late user input")),
                "key" => input
                    .behavior(|b| {
                        b.on_key(|_| {
                            close_current();
                            false
                        })
                    })
                    .behavior(|b| b.on_key(|_| panic!("late component key")))
                    .on_key(|_| panic!("late user key")),
                "blur" => input
                    .behavior(|b| b.on_blur(close_current))
                    .behavior(|b| b.on_blur(|| panic!("late component blur")))
                    .on_blur(|| panic!("late user blur")),
                _ => unreachable!(),
            };
            col().on_key(|_| panic!("late ancestor key")).child(input).child(text_input().id("other").value("other"))
        });
        window.dispatch(Event::Focus("field".into())).unwrap();
        match kind {
            "input" => {
                window.dispatch(Event::Input("field".into(), "next".into())).unwrap();
            }
            "key" => {
                window.press_key("right").unwrap();
            }
            "blur" => {
                window.dispatch(Event::Focus("other".into())).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(window.is_closed(), "{kind}");
        assert!(window.app().diagnostics().is_empty(), "{kind}: {:?}", window.app().diagnostics());
    }
}

#[test]
fn every_behavior_signature_stops_when_a_component_closes_its_window() {
    use octara::app::{Phase, PointerEvent};
    use octara::kernel::NumericRange;
    for kind in ["press", "toggle", "submit", "numeric", "drag", "focus"] {
        let mut window = Window::headless(move || {
            let element = match kind {
                "press" => el().behavior(|b| b.on_press(close_current)).on_press(|| panic!("late press")),
                "toggle" => el().behavior(|b| b.on_toggle(close_current)).on_toggle(|| panic!("late toggle")),
                "submit" => text_input().behavior(|b| b.on_submit(close_current)).on_submit(|| panic!("late submit")),
                "numeric" => el()
                    .role(Role::Slider)
                    .numeric_range(NumericRange { value: Some(0.0), min: 0.0, max: 10.0, step: Some(1.0) })
                    .behavior(|b| b.on_numeric_input(|_| close_current()))
                    .on_numeric_input(|_| panic!("late numeric")),
                "drag" => el().behavior(|b| b.on_drag(|_| close_current())).on_drag(|_| panic!("late drag")),
                "focus" => el().behavior(|b| b.on_focus(close_current)).on_focus(|| panic!("late focus")),
                _ => unreachable!(),
            };
            element.id("target").focusable().w(200).h(50)
        });
        match kind {
            "press" => {
                window.tap("target").unwrap();
            }
            "toggle" => {
                window.dispatch(Event::Toggle("target".into())).unwrap();
            }
            "submit" => {
                window.dispatch(Event::Focus("target".into())).unwrap();
                window.press_key("enter").unwrap();
            }
            "numeric" => {
                window.dispatch(Event::NumericInput("target".into(), 5.0)).unwrap();
            }
            "drag" => {
                window.event(InputEvent::Pointer(PointerEvent::mouse(Phase::Down, 10.0, 10.0)));
                window.event(InputEvent::Pointer(PointerEvent::mouse(Phase::Move, 50.0, 10.0)));
            }
            "focus" => {
                window.dispatch(Event::Focus("target".into())).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(window.is_closed(), "{kind}");
        assert!(window.app().diagnostics().is_empty(), "{kind}: {:?}", window.app().diagnostics());
    }
}

#[test]
fn borrowed_link_and_file_payloads_reach_component_then_application_handlers() {
    use octara::app::FileDrag;
    use std::path::PathBuf;
    let seen = Rc::new(RefCell::new(Vec::new()));
    let saved = seen.clone();
    let mut window = Window::headless(move || {
        let (link, app_link, files, app_files) = (saved.clone(), saved.clone(), saved.clone(), saved.clone());
        col()
            .size_full()
            .gap_4()
            .behavior(|b| b.on_link(move |url| link.borrow_mut().push(format!("component:{url}"))))
            .on_link(move |url| app_link.borrow_mut().push(format!("application:{url}")))
            .child(text(rich().text("Read ").link("guide", "https://example.com/guide")))
            .child(
                el().id("drop")
                    .w(200)
                    .h(80)
                    .behavior(|b| b.on_drop(move |paths| files.borrow_mut().push(format!("component:{paths:?}"))))
                    .on_drop(move |paths| app_files.borrow_mut().push(format!("application:{paths:?}"))),
            )
    });
    window.tap("guide").unwrap();
    let (x, y, ..) = {
        let k = window.kernel();
        k.rect(k.find("drop").unwrap())
    };
    let paths = vec![PathBuf::from("/tmp/文档.txt")];
    window.event(InputEvent::FileDrag(FileDrag::Drop { x: x + 4.0, y: y + 4.0, paths: paths.clone() }));
    assert_eq!(
        &*seen.borrow(),
        &[
            "component:https://example.com/guide".to_owned(),
            "application:https://example.com/guide".to_owned(),
            format!("component:{paths:?}"),
            format!("application:{paths:?}"),
        ]
    );
    assert!(window.app().diagnostics().is_empty());
}

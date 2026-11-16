use octara::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

#[test]
fn before_input_keeps_registration_order_and_only_the_last_application_callback() {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let saved = seen.clone();
    let mut window = Window::headless(move || {
        let (first, second, user) = (saved.clone(), saved.clone(), saved.clone());
        text_input()
            .id("field")
            .value("initial")
            .on_input(|_| panic!("replaced callback"))
            .before_input(move |s| first.borrow_mut().push(format!("first:{s}")))
            .on_input(move |s| user.borrow_mut().push(format!("user:{s}")))
            .before_input(move |s| second.borrow_mut().push(format!("second:{s}")))
            .into()
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
                    .before_key(move |_| {
                        first.borrow_mut().push("first");
                        false
                    })
                    .before_key(move |e| {
                        second.borrow_mut().push("second");
                        e.key == Key::Left
                    })
                    .on_key(move |_| {
                        user.borrow_mut().push("user");
                        false
                    }),
            )
            .into()
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
        text_input()
            .id("field")
            .value("abc")
            .before_key(|_| false)
            .on_key(|_| {
                App::current().hot_reload();
                false
            })
            .into()
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
                    .before_input(|_| close_current())
                    .before_input(|_| panic!("late component input"))
                    .on_input(|_| panic!("late user input")),
                "key" => input
                    .before_key(|_| {
                        close_current();
                        false
                    })
                    .before_key(|_| panic!("late component key"))
                    .on_key(|_| panic!("late user key")),
                "blur" => {
                    input.before_blur(close_current).before_blur(|| panic!("late component blur")).on_blur(|| panic!("late user blur"))
                }
                _ => unreachable!(),
            };
            col().on_key(|_| panic!("late ancestor key")).child(input).child(text_input().id("other").value("other")).into()
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

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

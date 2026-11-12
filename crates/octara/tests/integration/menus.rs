//! Menus and prompts a window draws itself (where its platform shows none): a menu at its point,
//! a prompt over a scrim; a pick answers the task that asked, Escape dismisses.

use octara::ask::{Answer, Ask};
use octara::prelude::*;

fn view() -> View {
    let got = state("got", String::new);
    col()
        .p_4()
        .gap_4()
        .child(el().id("target").w(200.0).h(60.0).on_secondary_press(move || {
            spawn(async move {
                let menu = Menu::new("Edit")
                    .item(MenuItem::action("Copy", "copy").shortcut("mod+c"))
                    .item(MenuItem::action("Paste", "paste").disabled())
                    .item(MenuItem::separator())
                    .item(MenuItem::action("Wrap lines", "wrap").checked(true))
                    .item(MenuItem::submenu(Menu::new("More").item(MenuItem::action("Deeper", "deep"))));
                let picked = popup_menu(menu, None).await;
                got.set(format!("menu {picked:?}"));
            });
        }))
        .child(el().id("ask").size_8().on_press(move || {
            spawn(async move {
                let picked =
                    prompt(PromptLevel::Warning, "Discard changes?", Some("They cannot be recovered."), &["Cancel", "Discard"]).await;
                got.set(format!("prompt {picked:?}"));
            });
        }))
        .child(text(move || got.get()).id("got"))
        .into()
}

fn got(w: &Window) -> String {
    w.kernel().text(w.kernel().find("got").unwrap()).to_owned()
}

#[test]
fn a_drawn_menu_answers_the_item_picked() {
    let mut w = Window::headless(view);
    w.secondary_tap("target").unwrap();
    assert!(matches!(w.asks().as_slice(), [(_, Ask::Menu { .. })]));
    assert!(w.kernel().find("menu:copy").is_some(), "drawn in the window");
    assert_eq!(w.kernel().role_of("menu:wrap"), Some(Role::MenuItemCheckBox));
    // A disabled item does not answer.
    assert!(w.tap("menu:paste").is_err() || w.asks().len() == 1);
    w.tap("menu:copy").unwrap();
    assert_eq!(got(&w), r#"menu Some("copy")"#);
    assert!(w.asks().is_empty() && w.kernel().find("menu:copy").is_none(), "closed");
}

#[test]
fn escape_dismisses_a_menu_and_submenus_open() {
    let mut w = Window::headless(view);
    w.secondary_tap("target").unwrap();
    w.tap("menu:More").unwrap();
    assert!(w.kernel().find("menu:deep").is_some(), "the submenu opened");
    w.tap("menu:deep").unwrap();
    assert_eq!(got(&w), r#"menu Some("deep")"#);
    w.secondary_tap("target").unwrap();
    w.press_key("escape").unwrap();
    assert_eq!(got(&w), "menu None");
    assert!(w.asks().is_empty());
}

#[test]
fn an_answer_from_outside_closes_the_drawn_menu() {
    let mut w = Window::headless(view);
    w.secondary_tap("target").unwrap();
    let id = w.asks()[0].0;
    w.answer(id, Answer::Menu(Some("wrap".into()))).unwrap();
    assert_eq!(got(&w), r#"menu Some("wrap")"#);
    assert!(w.kernel().find("menu:copy").is_none());
    // An item that is disabled is no answer.
    w.secondary_tap("target").unwrap();
    let id = w.asks()[0].0;
    assert!(w.answer(id, Answer::Menu(Some("paste".into()))).is_err());
}

#[test]
fn a_prompt_answers_and_escape_dismisses_it() {
    let mut w = Window::headless(view);
    w.app().start_recording();
    w.tap("ask").unwrap();
    assert!(matches!(w.asks().as_slice(), [(_, Ask::Prompt { answers, .. })] if answers.len() == 2));
    assert_eq!(w.kernel().state_of("prompt:1").map(|s| s.contains(State::FOCUSED)), Some(true), "the default is focused");
    w.tap("prompt:1").unwrap();
    assert_eq!(got(&w), "prompt Some(1)");
    let recording = w.app().stop_recording();
    assert!(
        !recording.events.iter().any(|r| matches!(r.event, InputEvent::Answer { .. })),
        "a drawn prompt is answered by its press, recorded as one"
    );
    w.tap("ask").unwrap();
    w.press_key("escape").unwrap();
    assert_eq!(got(&w), "prompt None");
}

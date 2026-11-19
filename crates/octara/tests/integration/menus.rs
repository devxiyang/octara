//! Menus and prompts a window draws itself (where its platform shows none): a menu at its point,
//! a prompt over a scrim; a pick answers the task that asked, Escape dismisses.

use octara::app::{Event, InputEvent};
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

#[test]
fn a_drawn_prompt_opens_in_front_of_an_application_overlay() {
    let mut w = Window::headless(|| {
        let open = state("open", || false);
        let got = state("got", String::new);
        col()
            .size_full()
            .child(el().id("open").size_8().focusable().label("Open dialog").on_press(move || open.set(true)))
            .child(show(
                move || open.get(),
                move || {
                    el().absolute().size(0.0).a11y_hidden().overlay_window(
                        || true,
                        move || {
                            col()
                                .id("app-dialog")
                                .size_full()
                                .role(Role::Dialog)
                                .label("Application dialog")
                                .bg(Token::Surface)
                                .modal(true, move || open.set(false))
                                .child(el().id("ask-in-dialog").size_8().focusable().label("Ask").on_press(move || {
                                    spawn(async move {
                                        let picked = prompt(PromptLevel::Warning, "Discard changes?", None, &["Cancel", "Discard"]).await;
                                        got.set(format!("prompt {picked:?}"));
                                    });
                                }))
                        },
                    )
                },
            ))
            .child(text(got).id("got"))
    });

    w.tap("open").unwrap();
    w.tap("ask-in-dialog").unwrap();
    assert_eq!(w.inspect().focus.as_deref(), Some("prompt:1"));
    assert!(w.tap("ask-in-dialog").is_err());
    w.tap("prompt:1").unwrap();
    assert_eq!(got(&w), "prompt Some(1)");
    assert!(w.asks().is_empty());
    assert_eq!(w.inspect().focus.as_deref(), Some("ask-in-dialog"));

    w.tap("ask-in-dialog").unwrap();
    w.press_key("escape").unwrap();
    assert_eq!(got(&w), "prompt None");
    assert!(w.kernel().exists("app-dialog"));
    assert_eq!(w.inspect().focus.as_deref(), Some("ask-in-dialog"));
    w.press_key("escape").unwrap();
    assert!(!w.kernel().exists("app-dialog"));
    assert_eq!(w.inspect().focus.as_deref(), Some("open"));
}

fn menus_shown(w: &Window) -> Vec<Menu> {
    let app = w.app();
    let p = app.platform();
    p.as_any().downcast_ref::<octara::platform::HeadlessPlatform>().unwrap().menus.clone()
}

fn item<'a>(menus: &'a [Menu], id: &str) -> &'a MenuItem {
    menus.iter().find_map(|m| m.find(id)).unwrap_or_else(|| panic!("no item {id}"))
}

fn enabled(menus: &[Menu], id: &str) -> bool {
    matches!(item(menus, id), MenuItem::Action { enabled: true, .. })
}

#[test]
fn the_menu_bar_follows_what_the_active_window_handles() {
    let mut w = Window::headless(|| {
        let saved = state("saved", || 0);
        col()
            .on_command("file.save", move || saved.update(|n| *n += 1))
            .child(input().id("f").value("hello"))
            .child(text(move || saved.get().to_string()).id("saved"))
    });
    w.app().run(|| set_menus([Menu::new("File").item(MenuItem::action("Save", "file.save")), Menu::edit()]));
    w.tap("f").unwrap();
    let menus = menus_shown(&w);
    assert!(enabled(&menus, "file.save"), "the app handles it");
    assert!(!enabled(&menus, "edit.copy"), "nothing selected in the field");
    assert!(matches!(item(&menus, "edit.copy"), MenuItem::Action { shortcut: Some(s), .. } if s == "mod+c"), "keys from the keymap");
    w.press_key("mod+a").unwrap();
    assert!(enabled(&menus_shown(&w), "edit.copy"), "a selection: copy is enabled");
    // A pick is input: the command runs in the window.
    w.event(InputEvent::Command { name: "file.save".into(), chord: None });
    assert_eq!(w.kernel().text(w.kernel().find("saved").unwrap()), "1");
}

#[test]
fn a_menu_item_picked_by_its_keys_lets_the_keymap_decide() {
    let mut w = Window::headless(|| {
        let what = state("what", String::new);
        col()
            .on_command("app.close_window", move || what.set("closed the window".into()))
            .child(
                col()
                    .id("editor")
                    .size_8()
                    .key_context("editor")
                    .focusable()
                    .on_command("editor.close_tab", move || what.set("closed a tab".into())),
            )
            .child(text(move || what.get()).id("what"))
    });
    w.app().bind_keys([("mod+w", "editor.close_tab", Some("editor"))]).unwrap();
    w.tap("editor").unwrap();
    // The menu's item runs `app.close_window` on cmd+w, but in the editor the keymap binds it to
    // closing a tab: the keys win, as they would without the menu.
    w.event(InputEvent::Command { name: "app.close_window".into(), chord: Some("mod+w".into()) });
    assert_eq!(w.kernel().text(w.kernel().find("what").unwrap()), "closed a tab");
    w.event(InputEvent::Command { name: "app.close_window".into(), chord: None });
    assert_eq!(w.kernel().text(w.kernel().find("what").unwrap()), "closed the window", "a click on the item runs its command");
}

#[test]
fn undo_uses_the_window_history_for_menu_toolbar_and_inspector() {
    use octara::inspect::{self, Delivery};
    use octara::platform::HeadlessPlatform;

    fn document() -> El {
        let draft = state("draft", || "Draft".to_owned());
        let window = App::current().window(current_window()).unwrap();
        col().child(input().id("editor").label("Draft").value(draft).on_input(move |text| draft.set(text))).child(
            el().id("undo")
                .size_12()
                .label("Undo")
                .preserve_focus_on_press(true)
                .disabled(move || !window.command_enabled("edit.undo"))
                .on_press(|| invoke("edit.undo").unwrap()),
        )
    }
    fn toolbar_enabled(window: &Window) -> bool {
        !window.kernel().state_of("undo").unwrap().contains(State::DISABLED)
    }
    fn inspector_undo(window: &mut Window) -> inspect::Reply {
        let request = octara::serde_json::json!({"op": "invoke", "command": "edit.undo", "target": window.target()});
        inspect::handle_line(window, &request.to_string())
    }

    let app = App::new(HeadlessPlatform::default());
    let mut a = app.open_window(WindowOptions::new("Document"), document).unwrap();
    let mut b = app.open_window(WindowOptions::new("Document"), document).unwrap();
    app.run(|| set_menus([Menu::edit()]));
    for window in [&mut a, &mut b] {
        window.dispatch(Event::Focus("editor".into())).unwrap();
        assert!(!toolbar_enabled(window));
    }
    let refused = inspector_undo(&mut b);
    assert!(!refused.ok);
    assert_eq!(refused.delivery, Some(Delivery::Refused));
    a.event(InputEvent::Active(true));
    a.type_text("A");
    let a_value = a.inspect().text_input.unwrap().text;
    assert!(toolbar_enabled(&a));
    assert!(!toolbar_enabled(&b));
    assert!(enabled(&menus_shown(&a), "edit.undo"));
    b.event(InputEvent::Active(true));
    assert!(!enabled(&menus_shown(&b), "edit.undo"));
    b.type_text("B");
    let b_value = b.inspect().text_input.unwrap().text;
    assert!(enabled(&menus_shown(&b), "edit.undo"));
    assert!(toolbar_enabled(&b));

    b.app().bind_keys([("mod+z", None, Some("text_input"))]).unwrap();
    b.press_key("mod+z").unwrap();
    assert!(matches!(item(&menus_shown(&b), "edit.undo"), MenuItem::Action { shortcut: None, .. }));
    assert!(!b.event(InputEvent::Command { name: "edit.undo".into(), chord: Some("mod+z".into()) }));
    assert_eq!(b.inspect().text_input.unwrap().text, b_value);
    assert!(b.event(InputEvent::Command { name: "edit.undo".into(), chord: None }));
    assert_eq!(b.inspect().text_input.unwrap().text, "Draft");
    assert_eq!(a.inspect().text_input.unwrap().text, a_value);
    assert!(!toolbar_enabled(&b));

    a.tap("undo").unwrap();
    assert_eq!(a.inspect().text_input.unwrap().text, "Draft");
    assert!(!toolbar_enabled(&a));
    b.type_text("B2");
    a.event(InputEvent::Active(true));
    let delivered = inspector_undo(&mut b);
    assert!(delivered.ok);
    assert_eq!(delivered.delivery, Some(Delivery::Delivered));
    assert_eq!(b.inspect().text_input.unwrap().text, "Draft");
    assert_eq!(a.inspect().text_input.unwrap().text, "Draft");
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
    app.shutdown();
}

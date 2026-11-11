//! Components: semantics, controlled values, interaction looks, select with its overlay.

use octara::prelude::*;
use octara_kit::*;

fn tok(t: Token) -> Color {
    Theme::light().get(t)
}

fn view() -> View {
    let count = state("count", || 0);
    let locked = state("locked", || false);
    let wifi = state("wifi", || false);
    let name = state("name", || "Ada".to_string());
    let size = state("size", || 2u8);
    let starred = state("starred", || false);
    col()
        .child(primary_button("Save").id("save").disabled(move || locked.get()).on_press(move || count.update(|n| *n += 1)))
        .child(button("Lock").id("lock").on_press(move || locked.update(|l| *l = !*l)))
        .child(text(move || format!("saved {}", count.get())).id("count"))
        .child(switch(wifi).id("wifi").label("Wi-Fi"))
        .child(
            // A model that is not a Signal: getter and setter.
            checkbox((move || starred.get(), move |v| starred.set(v))).id("star"),
        )
        .child(text_field(name).id("name").label("Name"))
        .child(text(move || format!("hello {}", name.get())).id("greeting"))
        .child(select("size", size, [(1u8, "Small"), (2, "Medium"), (3, "Large")]))
        .child(text(move || format!("size {}", size.get())).id("size-out"))
        .into()
}

#[test]
fn buttons_have_roles_states_and_refuse_when_disabled() {
    let mut app = App::new(view);
    let k = app.kernel();
    assert_eq!(k.role_of("save"), Some(Role::Button));
    assert_eq!(k.visual_of("save").unwrap().background, Some(tok(Token::Accent)));
    drop(k);
    app.dispatch(Event::Hover("save".into(), true)).unwrap();
    assert_eq!(app.kernel().visual_of("save").unwrap().background, Some(tok(Token::AccentHover)));
    app.dispatch(Event::Press("save".into())).unwrap();
    app.dispatch(Event::Press("lock".into())).unwrap();
    assert_eq!(app.kernel().visual_of("save").unwrap().opacity, 0.5);
    assert!(app.dispatch(Event::Press("save".into())).is_err());
    assert_eq!(app.kernel().text_of("count").unwrap(), "saved 1");
}

#[test]
fn switch_and_checkbox_are_controlled() {
    let mut app = App::new(view);
    let thumb_x = |app: &App| {
        let k = app.kernel();
        let sw = k.find("wifi").unwrap();
        k.rect(k.children(sw)[0]).0
    };
    let off = thumb_x(&app);
    assert_eq!(app.kernel().role_of("wifi"), Some(Role::Switch));
    assert_eq!(app.kernel().checked_of("wifi"), Some(false));
    app.dispatch(Event::Toggle("wifi".into())).unwrap();
    assert_eq!(app.kernel().checked_of("wifi"), Some(true));
    assert_eq!(app.kernel().visual_of("wifi").unwrap().background, Some(tok(Token::Accent)));
    assert!(thumb_x(&app) > off, "the thumb moves to the end");
    app.dispatch(Event::Toggle("star".into())).unwrap();
    assert_eq!(app.kernel().checked_of("star"), Some(true));
}

#[test]
fn text_field_is_bound_both_ways() {
    let mut app = App::new(view);
    assert_eq!(app.kernel().value_of("name").unwrap(), "Ada");
    app.dispatch(Event::Input("name".into(), "Grace".into())).unwrap();
    assert_eq!(app.kernel().text_of("greeting").unwrap(), "hello Grace");
    app.dispatch(Event::Focus("name".into())).unwrap();
    assert_eq!(app.kernel().visual_of("name").unwrap().border, tok(Token::Focus));
}

#[test]
fn select_opens_a_listbox_and_picks() {
    let mut app = App::new(view);
    let k = app.kernel();
    assert_eq!(k.role_of("size"), Some(Role::ComboBox));
    assert_eq!(k.value_of("size").unwrap(), "Medium");
    assert!(!k.exists("size-list"));
    drop(k);
    app.dispatch(Event::Press("size".into())).unwrap();
    let k = app.kernel();
    assert!(k.tree_dump().contains("ComboBox#size \"Medium\" value=\"Medium\" expanded=true"));
    assert_eq!(k.role_of("size-list"), Some(Role::ListBox));
    assert!(k.state_of("size-option-1").unwrap().contains(State::SELECTED));
    let (_, ty, _, th) = k.rect(k.find("size").unwrap());
    let (_, ly, _, _) = k.rect(k.find("size-list").unwrap());
    assert_eq!(ly, ty + th + 4.0, "opens below the trigger");
    drop(k);
    app.hot_reload();
    assert!(app.kernel().exists("size-list"), "open state survives hot reload");
    app.dispatch(Event::Press("size-option-2".into())).unwrap();
    let k = app.kernel();
    assert_eq!(k.text_of("size-out").unwrap(), "size 3");
    assert_eq!(k.value_of("size").unwrap(), "Large");
    assert!(!k.exists("size-list"), "picking closes it");
    assert_eq!(k.expanded_of("size"), Some(false));
}

#[test]
fn no_diagnostics() {
    let mut app = App::new(view);
    app.dispatch(Event::Press("size".into())).unwrap();
    app.hot_reload();
    let d = app.diagnostics();
    assert!(d.is_empty(), "{d:?}");
}

#[test]
fn select_works_from_the_keyboard() {
    let mut app = App::new(view);
    app.dispatch(Event::Focus("size".into())).unwrap();
    app.press_key("enter").unwrap();
    assert!(app.kernel().exists("size-list"));
    let focused = |app: &App, id: &str| app.kernel().state_of(id).unwrap().contains(State::FOCUSED);
    assert!(focused(&app, "size-option-1"), "the list opens on the selected option");
    app.press_key("down").unwrap();
    assert!(focused(&app, "size-option-2"));
    app.press_key("enter").unwrap();
    assert_eq!(app.kernel().text_of("size-out").unwrap(), "size 3");
    assert!(!app.kernel().exists("size-list"));
    assert!(focused(&app, "size"), "focus goes back to the select");
    // Escape closes it without picking; a press outside does too.
    app.press_key("space").unwrap();
    app.press_key("up").unwrap();
    app.press_key("escape").unwrap();
    assert!(!app.kernel().exists("size-list"));
    assert_eq!(app.kernel().text_of("size-out").unwrap(), "size 3");
    app.press_key("enter").unwrap();
    app.click_at(790.0, 590.0);
    assert!(!app.kernel().exists("size-list"));
    // The list floats on a shadow.
    app.press_key("enter").unwrap();
    let list = app.kernel().find("size-list").unwrap();
    assert!(app.display_list().items.iter().any(|i| matches!(i, octara::paint::Item::Shadow(s) if s.source == list)));
}

//! The keyboard: chords as data, the keymap resolved along the focus path, commands, `on_key`,
//! other keyboard layouts, and input methods.

use octara::app::{Chord, Platform};
use octara::prelude::*;

fn view() -> View {
    let log = state("log", Vec::<String>::new);
    let push = move |s: &str| {
        let s = s.to_owned();
        log.update(|l| l.push(s))
    };
    on_command("save", move || push("app-save"));
    on_command("open", move || push("app-open"));
    channel::<String>().1.named("clear").on(move |_| log.set(Vec::new()));
    let field = state("field", String::new);
    col()
        .child(text(move || log.get().join(",")).id("log"))
        .child(
            el().id("editor")
                .key_context("editor")
                .on_command("save", move || push("editor-save"))
                .on_command("format", move || push("format"))
                .child(
                    el().id("in-editor")
                        .w(px(80.0))
                        .h(px(24.0))
                        .focusable()
                        .on_press(move || push("press"))
                        .on_secondary_press(move || push("menu")),
                )
                .child(
                    text_input().id("field").value(move || field.get()).on_input(move |v| field.set(v)).on_submit(move || push("submit")),
                ),
        )
        .child(el().id("outside").w(px(80.0)).h(px(24.0)).focusable().on_key(move |k| {
            if k.key == Key::Char('x') {
                push("x-key");
                true
            } else {
                false
            }
        }))
        .into()
}

fn log(app: &mut App) -> String {
    let s = app.kernel().text_of("log").unwrap();
    app.send("clear", "").unwrap();
    s
}

fn app() -> App {
    let mut app = App::new(view);
    app.set_platform(Platform::MacOs);
    app.bind("mod+s", Some("save"), None).unwrap();
    app.bind("mod+o", Some("open"), None).unwrap();
    app.bind("mod+shift+f", Some("format"), Some("editor")).unwrap();
    app
}

#[test]
fn chords_parse_and_print() {
    let c = Chord::parse("mod+shift+k").unwrap();
    assert_eq!(c.to_string(), "mod+shift+k");
    assert_eq!(c.modifiers_on(Platform::MacOs), Modifiers::META | Modifiers::SHIFT);
    assert_eq!(c.modifiers_on(Platform::Windows), Modifiers::CTRL | Modifiers::SHIFT);
    assert_eq!(Chord::parse("ctrl++").unwrap().key, Key::Char('+'));
    assert_eq!(Chord::parse("shift+f10").unwrap().key, Key::F(10));
    assert!(Chord::parse("hyper+k").unwrap_err().contains("unknown modifier"));
    assert!(Chord::parse("mod+banana").unwrap_err().contains("unknown key"));
}

#[test]
fn commands_resolve_along_the_focus_path() {
    let mut app = app();
    // Nothing focused: the app handles `save`; `format` is bound only inside the editor.
    app.press_key("cmd+s").unwrap();
    app.press_key("cmd+shift+f").unwrap();
    assert_eq!(log(&mut app), "app-save");
    app.dispatch(Event::Focus("in-editor".into())).unwrap();
    app.press_key("cmd+s").unwrap();
    app.press_key("cmd+shift+f").unwrap();
    app.press_key("cmd+o").unwrap();
    assert_eq!(log(&mut app), "editor-save,format,app-open", "the innermost handler wins; others fall through to the app");
    // Off Apple platforms `mod` is Control.
    app.set_platform(Platform::Linux);
    app.press_key("ctrl+s").unwrap();
    app.press_key("cmd+s").unwrap();
    assert_eq!(log(&mut app), "editor-save");
}

#[test]
fn later_and_deeper_bindings_win_and_none_unbinds() {
    let mut app = app();
    app.dispatch(Event::Focus("in-editor".into())).unwrap();
    app.bind("mod+s", Some("open"), None).unwrap();
    app.press_key("cmd+s").unwrap();
    assert_eq!(log(&mut app), "app-open", "a later binding at the same depth wins");
    app.bind("mod+s", Some("save"), Some("editor")).unwrap();
    app.press_key("cmd+s").unwrap();
    assert_eq!(log(&mut app), "editor-save", "a deeper context wins over a later one");
    app.bind("mod+s", None, Some("editor")).unwrap();
    app.press_key("cmd+s").unwrap();
    assert_eq!(log(&mut app), "", "unbound inside the editor");
}

#[test]
fn invoke_and_list_commands() {
    let mut app = app();
    app.dispatch(Event::Focus("in-editor".into())).unwrap();
    app.invoke("format").unwrap();
    assert_eq!(log(&mut app), "format");
    assert!(app.invoke("nope").unwrap_err().contains("nothing handles `nope`"));
    let commands = app.commands();
    let save = commands.iter().find(|(c, _)| c == "save").unwrap();
    assert_eq!(save.1, ["mod+s"]);
    assert!(commands.iter().any(|(c, chords)| c == "focus.next" && chords == &["tab"]));
    assert!(app.keymap().bindings.iter().any(|b| b.command.as_deref() == Some("format") && b.context.as_deref() == Some("editor")));
}

#[test]
fn on_key_comes_before_the_keymap() {
    let mut app = app();
    app.tap("outside").unwrap();
    app.press_key("x").unwrap();
    app.press_key("cmd+s").unwrap();
    assert_eq!(log(&mut app), "x-key,app-save");
}

#[test]
fn shortcuts_follow_the_physical_key_under_other_layouts() {
    let mut app = app();
    // Cmd + the key that types `ы` on a Russian layout, where US has `s`.
    let ev = KeyEvent { key: Key::Char('ы'), code: Some('s'), modifiers: Modifiers::META, repeat: false };
    assert!(app.key_event(ev));
    assert_eq!(log(&mut app), "app-save");
}

#[test]
fn enter_space_and_the_menu_key_act_on_focus() {
    let mut app = app();
    app.dispatch(Event::Focus("in-editor".into())).unwrap();
    app.press_key("enter").unwrap();
    app.press_key("space").unwrap();
    app.press_key("menu").unwrap();
    app.press_key("shift+f10").unwrap();
    assert_eq!(log(&mut app), "press,press,menu,menu");
}

#[test]
fn a_text_field_keeps_its_keys() {
    let mut app = app();
    app.tap("field").unwrap();
    app.press_key("h").unwrap();
    app.press_key("i").unwrap();
    app.press_key("space").unwrap();
    app.press_key("shift+a").unwrap();
    app.press_key("left").unwrap();
    assert_eq!(app.kernel().value_of("field").unwrap(), "hi A");
    assert!(app.kernel().state_of("field").unwrap().contains(State::FOCUSED), "arrows stay in the field");
    app.press_key("backspace").unwrap();
    assert_eq!(app.kernel().value_of("field").unwrap(), "hiA", "Backspace deletes before the caret, which Left moved");
    app.press_key("enter").unwrap();
    assert_eq!(log(&mut app), "submit");
}

#[test]
fn an_input_method_owns_enter_and_escape_while_composing() {
    let mut app = app();
    app.tap("field").unwrap();
    app.set_composition("ni");
    app.press_key("enter").unwrap();
    app.press_key("escape").unwrap();
    assert_eq!(log(&mut app), "", "Enter and Escape went to the input method");
    assert!(app.kernel().state_of("field").unwrap().contains(State::FOCUSED));
    // A shortcut commits the composition first, then runs.
    app.press_key("cmd+s").unwrap();
    assert_eq!(app.kernel().value_of("field").unwrap(), "ni");
    assert_eq!(log(&mut app), "editor-save");
}

#[test]
fn enter_submits_a_field_and_presses_one_without_on_submit() {
    let mut app = App::new(|| {
        let log = state("log", Vec::<String>::new);
        let push = move |s: &'static str| log.update(|l| l.push(s.into()));
        col()
            .child(text(move || log.get().join(",")).id("log"))
            .child(
                text_input()
                    .id("search")
                    .w(px(200.0))
                    .value("")
                    .on_input(|_| {})
                    .on_submit(move || push("submit"))
                    .on_press(move || push("press")),
            )
            .child(text_input().id("plain").w(px(200.0)).value("").on_input(|_| {}).on_press(move || push("plain-press")))
            .into()
    });
    // A click presses a field like any element; Enter submits it.
    app.tap("search").unwrap();
    app.press_key("enter").unwrap();
    assert_eq!(app.kernel().text_of("log").unwrap(), "press,submit");
    // Without `on_submit`, Enter falls through to `press`.
    app.dispatch(Event::Focus("plain".into())).unwrap();
    app.press_key("enter").unwrap();
    assert_eq!(app.kernel().text_of("log").unwrap(), "press,submit,plain-press");
    assert!(app.commands().iter().any(|(c, chords)| c == "submit" && chords == &["enter"]));
}

//! Text editing owned by the runtime: carets by grapheme and word, selections, clipboard,
//! undo, composition at the caret, pointer selection, and the platform text protocol.

use octara::accesskit;
use octara::app::{Phase, Platform, PointerEvent, byte_offset, utf16_offset};
use octara::prelude::*;

fn app(initial: &'static str) -> App {
    let mut app = App::new(move || {
        let v = state("v", move || initial.to_owned());
        col()
            .size_full()
            .child(text_input().id("field").w(px(200.0)).value(move || v.get()).on_input(move |s| v.set(s)))
            .child(text_input().id("narrow").w(px(60.0)).value("").on_input(|_| {}))
            .into()
    });
    app.set_platform(Platform::MacOs);
    app.tap("field").unwrap();
    app
}

fn value(app: &App) -> String {
    app.kernel().value_of("field").unwrap()
}

fn selection(app: &App) -> std::ops::Range<usize> {
    app.text_field().unwrap().selection
}

fn keys(app: &mut App, chords: &[&str]) {
    for c in chords {
        app.press_key(c).unwrap();
    }
}

#[test]
fn carets_move_by_grapheme_and_word() {
    let mut app = app("");
    app.type_text("cafe\u{301} 👍🏽 ok");
    keys(&mut app, &["left", "left", "left"]);
    let after = "cafe\u{301} 👍🏽".len();
    assert_eq!(selection(&app), after..after);
    keys(&mut app, &["left"]);
    let before = "cafe\u{301} ".len();
    assert_eq!(selection(&app), before..before, "the thumbs-up with its skin tone is one step");
    keys(&mut app, &["home", "right", "right", "right", "right"]);
    assert_eq!(selection(&app).start, "cafe\u{301}".len(), "e + combining acute is one step");
    keys(&mut app, &["alt+left"]);
    assert_eq!(selection(&app).start, 0);
    keys(&mut app, &["alt+right", "alt+right"]);
    assert_eq!(&value(&app)[..selection(&app).start], "cafe\u{301} 👍🏽 ok", "words skip the emoji and spaces");
    keys(&mut app, &["cmd+left"]);
    assert_eq!(selection(&app).start, 0);
}

#[test]
fn typing_replaces_the_selection_and_deletes_by_word() {
    let mut app = app("hello brave world");
    keys(&mut app, &["end", "alt+shift+left"]);
    assert_eq!(selection(&app), 12..17);
    app.type_text("there");
    assert_eq!(value(&app), "hello brave there");
    keys(&mut app, &["alt+backspace"]);
    assert_eq!(value(&app), "hello brave ");
    keys(&mut app, &["home", "delete"]);
    assert_eq!(value(&app), "ello brave ");
    // Off Apple platforms words move with Control.
    app.set_platform(Platform::Windows);
    keys(&mut app, &["ctrl+delete"]);
    assert_eq!(value(&app), " brave ");
}

#[test]
fn cut_copy_paste() {
    let mut app = app("one two");
    keys(&mut app, &["cmd+a", "cmd+c"]);
    assert_eq!(app.clipboard_text().as_deref(), Some("one two"));
    keys(&mut app, &["end", "alt+shift+left", "cmd+x"]);
    assert_eq!(value(&app), "one ");
    assert_eq!(app.clipboard_text().as_deref(), Some("two"));
    keys(&mut app, &["home", "cmd+v"]);
    assert_eq!(value(&app), "twoone ");
    assert_eq!(selection(&app), 3..3);
    app.set_clipboard(Box::new(octara::app::MemoryClipboard(Some("a\nb".into()))));
    keys(&mut app, &["cmd+v"]);
    assert_eq!(value(&app), "twoa bone ", "one line: breaks become spaces");
}

#[test]
fn undo_takes_back_a_run_of_typing_at_once() {
    let mut app = app("");
    for c in ["h", "e", "l", "l", "o"] {
        app.press_key(c).unwrap();
    }
    keys(&mut app, &["space"]);
    app.type_text("you");
    keys(&mut app, &["backspace", "backspace"]);
    assert_eq!(value(&app), "hello y");
    keys(&mut app, &["cmd+z"]);
    assert_eq!(value(&app), "hello you", "the deletions were one step");
    keys(&mut app, &["cmd+z"]);
    assert_eq!(value(&app), "", "the typing was one step");
    keys(&mut app, &["cmd+shift+z"]);
    assert_eq!(value(&app), "hello you");
    assert_eq!(selection(&app), 9..9);
    // Moving the caret ends a run.
    keys(&mut app, &["left"]);
    app.type_text("!");
    keys(&mut app, &["end"]);
    app.type_text("?");
    keys(&mut app, &["cmd+z"]);
    assert_eq!(value(&app), "hello yo!u");
}

#[test]
fn a_value_the_app_refuses_keeps_the_caret_in_range() {
    let mut app = App::new(|| col().child(text_input().id("fixed").w(px(200.0)).value("abc").on_input(|_| {})).into());
    app.tap("fixed").unwrap();
    app.press_key("end").unwrap();
    app.type_text("zzzz");
    assert_eq!(app.kernel().value_of("fixed").unwrap(), "abc");
    let f = app.text_field().unwrap();
    assert!(f.selection.end <= 3, "{:?}", f.selection);
}

#[test]
fn composition_shows_at_the_caret_and_replaces_the_selection() {
    let mut app = app("ab");
    keys(&mut app, &["home", "right"]);
    let before = app.text_field().unwrap().caret.0;
    app.set_composition("ni");
    let f = app.text_field().unwrap();
    assert_eq!((f.text.as_str(), f.composition.as_str()), ("ab", "ni"));
    assert!(f.caret.0 > before, "the caret follows the composition");
    app.type_text("你");
    assert_eq!(value(&app), "a你b");
    keys(&mut app, &["shift+right"]);
    app.set_composition("x");
    assert_eq!(value(&app), "a你", "composing over a selection replaces it");
    app.type_text("y");
    assert_eq!(value(&app), "a你y");
}

#[test]
fn pointer_places_the_caret_and_selects() {
    let mut app = app("alpha beta gamma");
    let (x, y, w, h) = {
        let k = app.kernel();
        k.rect(k.find("field").unwrap())
    };
    let cy = y + h / 2.0;
    // Far right: the end. Far left: the start.
    app.click_at(x + w - 2.0, cy);
    assert_eq!(selection(&app), 16..16);
    app.advance(std::time::Duration::from_secs(1));
    app.click_at(x + 1.0, cy);
    assert_eq!(selection(&app), 0..0);
    // Shift-click extends; double-click selects a word; triple-click everything.
    app.advance(std::time::Duration::from_secs(1));
    app.pointer(PointerEvent::mouse(Phase::Down, x + w - 2.0, cy).modifiers(Modifiers::SHIFT));
    app.pointer(PointerEvent::mouse(Phase::Up, x + w - 2.0, cy));
    assert_eq!(selection(&app), 0..16);
    app.advance(std::time::Duration::from_secs(1));
    let caret_after_beta = {
        app.press_key("home").unwrap();
        keys(&mut app, &["alt+right", "alt+right"]);
        app.text_field().unwrap().caret.0
    };
    app.click_at(caret_after_beta - 4.0, cy);
    app.click_at(caret_after_beta - 4.0, cy);
    assert_eq!(selection(&app), 6..10, "the word `beta`");
    app.click_at(caret_after_beta - 4.0, cy);
    assert_eq!(selection(&app), 0..16);
    // A drag selects from where it started.
    app.advance(std::time::Duration::from_secs(1));
    app.pointer_down(x + 1.0, cy);
    app.pointer_move(caret_after_beta, cy);
    app.pointer_up(caret_after_beta, cy);
    assert_eq!(selection(&app), 0..10);
}

#[test]
fn a_long_line_scrolls_to_keep_the_caret_in_view() {
    let mut app = app("");
    app.tap("narrow").unwrap();
    app.type_text("a fairly long line of text for a narrow field");
    let (x, _, w, _) = {
        let k = app.kernel();
        k.rect(k.find("narrow").unwrap())
    };
    let caret = app.text_field().unwrap().caret.0;
    assert!(caret >= x && caret <= x + w, "caret {caret} inside {x}..{}", x + w);
    app.press_key("home").unwrap();
    let caret = app.text_field().unwrap().caret.0;
    assert!(caret >= x && caret < x + 10.0, "back at the start: {caret}");
}

#[test]
fn the_platform_text_protocol() {
    let mut app = app("héllo 👋");
    let f = app.text_field().unwrap();
    assert_eq!(utf16_offset(&f.text, f.text.len()), 8, "👋 is two UTF-16 units");
    assert_eq!(byte_offset(&f.text, 2), "hé".len());
    assert_eq!(byte_offset(&f.text, 7), "héllo ".len(), "inside a surrogate pair: its start");
    app.set_selection(1, 3);
    assert_eq!(selection(&app), 1..3);
    app.replace_text(1..3, "E");
    assert_eq!(value(&app), "hEllo 👋");
    assert_eq!(selection(&app), 2..2);
}

#[test]
fn assistive_technology_sets_the_value() {
    let mut app = app("old");
    let id = octara::kernel::a11y::a11y_id(app.kernel().find("field").unwrap());
    let done = app.accessibility_action(id, accesskit::Action::SetValue, Some(accesskit::ActionData::Value("new".into())));
    assert!(done);
    assert_eq!(value(&app), "new");
    assert_eq!(selection(&app), 3..3);
}

/// The glyphs a field draws, as text: one glyph per character with the bundled Latin fonts.
fn drawn_glyphs(app: &App, id: &str) -> usize {
    let n = app.kernel().find(id).unwrap();
    app.display_list().glyph_runs().filter(|r| r.source == n).map(|r| r.glyphs.len()).sum()
}

#[test]
fn what_a_field_draws_follows_every_edit_at_once() {
    let mut app = app("hello");
    assert_eq!(drawn_glyphs(&app, "field"), 5);
    // Deleting shortens the text: the old shape must not be reused.
    keys(&mut app, &["end", "backspace", "backspace"]);
    assert_eq!(drawn_glyphs(&app, "field"), 3);
    // Replacing a character with another of the same length.
    keys(&mut app, &["shift+left"]);
    app.type_text("X");
    assert_eq!(value(&app), "heX");
    let glyphs_before = app.display_list().glyph_runs().flat_map(|r| r.glyphs.iter().map(|g| g.id)).collect::<Vec<_>>();
    keys(&mut app, &["shift+left"]);
    app.type_text("Y");
    let glyphs_after = app.display_list().glyph_runs().flat_map(|r| r.glyphs.iter().map(|g| g.id)).collect::<Vec<_>>();
    assert_ne!(glyphs_before, glyphs_after, "the new character is drawn, not the old one");
    // A composition moving with the caret.
    keys(&mut app, &["home"]);
    app.set_composition("ab");
    assert_eq!(drawn_glyphs(&app, "field"), 5);
    keys(&mut app, &["right"]);
    app.set_composition("a");
    assert_eq!(drawn_glyphs(&app, "field"), 4);
}

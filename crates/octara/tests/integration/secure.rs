//! Secure fields: bullets on screen and in every report (inspect, accessibility, dumps,
//! diagnostics), no copy, cut, undo, word motion or input method; revealing shows the text to
//! the screen and assistive technology only; touch platforms show the last typed character
//! for a moment.

use octara::accesskit::Role as AkRole;
use octara::app::{MemoryClipboard, Platform};
use octara::kernel::a11y::a11y_id;
use octara::prelude::*;

const SECRET: &str = "pässwörd🙂";

fn app(platform: Platform) -> App {
    let mut app = App::new(|| {
        let v = state("v", || SECRET.to_owned());
        let show = state("show", || false);
        col()
            .child(
                text_input()
                    .id("pw")
                    .label("Password")
                    .secure()
                    .revealed(move || show.get())
                    .w(px(200.0))
                    .value(move || v.get())
                    .on_input(move |s| v.set(s)),
            )
            .child(el().id("show").role(Role::Button).label("Show").on_press(move || show.update(|s| *s = !*s)).size_8())
            .into()
    });
    app.set_platform(platform);
    app.tap("pw").unwrap();
    app
}

fn value(app: &App) -> String {
    app.kernel().value_of("pw").unwrap()
}

#[test]
fn nothing_reports_the_value() {
    let app = app(Platform::MacOs);
    let bullets = "•".repeat(9);
    let snap = app.inspect();
    let pw = snap.find("pw").unwrap();
    assert_eq!((pw.role.as_str(), pw.value.as_deref()), ("password_input", Some(bullets.as_str())));
    let tree = app.accessibility_tree(1.0);
    let n = app.kernel().find("pw").unwrap();
    let node = &tree.nodes.iter().find(|(i, _)| *i == a11y_id(n)).unwrap().1;
    assert_eq!((node.role(), node.value()), (AkRole::PasswordInput, Some(bullets.as_str())));
    let everything = format!("{} {} {:?}", snap.to_json(), app.kernel().tree_dump(), app.diagnostics());
    for part in ["pässwörd", "wörd", "🙂"] {
        assert!(!everything.contains(part), "`{part}` leaked");
    }
    assert!(app.text_field().unwrap().secure);
}

#[test]
fn editing_keeps_it_in() {
    let mut app = app(Platform::MacOs);
    app.set_clipboard(Box::new(MemoryClipboard(Some("clip".into()))));
    app.press_key("mod+a").unwrap();
    app.press_key("mod+c").unwrap();
    app.press_key("mod+x").unwrap();
    assert_eq!(app.clipboard_text().as_deref(), Some("clip"), "nothing copied out");
    assert_eq!(value(&app), SECRET, "nor cut");
    // Moves by grapheme (the emoji is one), words are not revealed.
    app.press_key("right").unwrap();
    app.press_key("shift+left").unwrap();
    app.press_key("backspace").unwrap();
    assert_eq!(value(&app), "pässwörd");
    app.press_key("alt+left").unwrap();
    assert_eq!(app.text_field().unwrap().selection, 0..0, "a word move goes to the start");
    app.type_text("x");
    app.press_key("mod+z").unwrap();
    assert_eq!(value(&app), "xpässwörd", "no undo history");
    app.set_composition("ni");
    assert_eq!(app.text_field().unwrap().composition, "", "no input method");
}

#[test]
fn revealing_shows_the_text_to_the_screen_only_for_agents() {
    let mut app = app(Platform::MacOs);
    app.tap("show").unwrap();
    let tree = app.accessibility_tree(1.0);
    let n = app.kernel().find("pw").unwrap();
    let node = &tree.nodes.iter().find(|(i, _)| *i == a11y_id(n)).unwrap().1;
    assert_eq!((node.role(), node.value()), (AkRole::TextInput, Some(SECRET)), "assistive technology reads what shows");
    assert_eq!(app.inspect().find("pw").unwrap().value.as_deref(), Some("•".repeat(9).as_str()), "agents still see bullets");
    // Copy stays off while revealed.
    app.tap("pw").unwrap();
    app.set_clipboard(Box::new(MemoryClipboard(None)));
    app.press_key("mod+a").unwrap();
    app.press_key("mod+c").unwrap();
    assert_eq!(app.clipboard_text(), None);
}

/// The glyph ids the field draws.
fn glyphs(app: &App) -> Vec<u32> {
    let n = app.kernel().find("pw").unwrap();
    app.display_list().glyph_runs().filter(|r| r.source == n).flat_map(|r| r.glyphs.iter().map(|g| g.id)).collect()
}

#[test]
fn touch_shows_the_last_character_for_a_moment() {
    let mut app = app(Platform::Ios);
    app.press_key("mod+a").unwrap();
    app.press_key("backspace").unwrap();
    app.type_text("a");
    app.type_text("b");
    let shown = glyphs(&app);
    assert_eq!(shown.len(), 2);
    assert_ne!(shown[0], shown[1], "a bullet, then the b");
    let due = app.next_deadline().expect("it hides later");
    assert!(due.as_secs_f64() <= 1.5 + 1e-6);
    app.advance(std::time::Duration::from_millis(1600));
    let hidden = glyphs(&app);
    assert_eq!(hidden[0], hidden[1], "two bullets");
    assert_eq!(value(&app), "ab");
}

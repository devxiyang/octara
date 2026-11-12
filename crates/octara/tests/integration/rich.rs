//! Rich text ([text core](../../../../docs/design/text.md) step 1): runs over one string, drawn
//! in their styles; links as elements of their own, pressed by name, by point, by key and by
//! assistive technology; restyling reshapes only what changes shape; kept glyphs.

use octara::paint::Item;
use octara::prelude::*;
use std::time::Duration;

fn note(opened: Signal<Vec<String>>) -> impl Fn() -> View + 'static {
    move || {
        col()
            .w(320.0)
            .child(
                text(rich().text("Run ").code("cargo test").text(", then read ").link("the guide", "https://octara.dev/guide").text("."))
                    .id("note"),
            )
            .on_link(move |target| opened.update(|v| v.push(target.to_owned())))
            .into()
    }
}

fn app() -> (Window, Signal<Vec<String>>) {
    let app = App::new(octara::platform::HeadlessPlatform::default());
    let opened = app.run(|| Signal::new(Vec::new()));
    let w = app.open_window(octara::platform::WindowOptions::new("Rich"), note(opened)).unwrap();
    (w, opened)
}

#[test]
fn runs_draw_in_their_styles() {
    let (w, _) = app();
    let k = w.kernel();
    let note = k.find("note").unwrap();
    assert_eq!(k.text_of("note").as_deref(), Some("Run cargo test, then read the guide."));
    assert_eq!(k.runs(note).len(), 2);
    let list = w.display_list();
    let theme = Theme::light();
    let glyphs: Vec<_> = list.glyph_runs().filter(|g| g.source == note).collect();
    assert!(glyphs.iter().any(|g| g.color == theme.get(Token::Accent)), "the link in the accent");
    assert!(glyphs.iter().any(|g| g.color == theme.get(Token::Text)), "the rest in the text color");
    let quads: Vec<_> = list.quads().filter(|q| q.source == note).collect();
    assert!(quads.iter().any(|q| q.fill == Some(theme.get(Token::SurfaceHover)) && q.rect.h > 10.0), "inline code on its background");
    assert!(quads.iter().any(|q| q.fill == Some(theme.get(Token::Accent)) && q.rect.h <= 2.0), "the link underlined");
    // The background is drawn before the glyphs it is behind.
    let first_glyph = list.items.iter().position(|i| matches!(i, Item::Glyphs(g) if g.source == note)).unwrap();
    let background = list.items.iter().position(|i| matches!(i, Item::Quad(q) if q.source == note && q.rect.h > 10.0)).unwrap();
    assert!(background < first_glyph);
}

#[test]
fn a_link_is_an_element() {
    let (mut w, opened) = app();
    let snap = w.inspect();
    let link = snap.root.walk().into_iter().find(|n| n.role == "link").expect("a link node").clone();
    assert_eq!((link.name.as_deref(), link.value.as_deref()), (Some("the guide"), Some("https://octara.dev/guide")));
    assert!(link.actions.contains(&"press".to_string()) && link.actions.contains(&"focus".to_string()));
    let note = snap.find("note").unwrap();
    assert!(link.bounds[0] > note.bounds[0] && link.bounds[2] < note.bounds[2], "on its words, not the whole text: {link:?}");

    // By name, as agents drive.
    w.tap("the guide").unwrap();
    // By point: a click on the words.
    w.click_at(link.bounds[0] + 4.0, link.bounds[1] + link.bounds[3] / 2.0);
    // Not on the words: nothing.
    w.click_at(note.bounds[0] + 2.0, note.bounds[1] + 4.0);
    // By keyboard: Tab reaches it, Enter presses it.
    w.press_key("tab").unwrap();
    assert!(w.inspect().root.walk().into_iter().any(|n| n.role == "link" && n.states.contains(&"focused".to_string())));
    w.press_key("enter").unwrap();
    assert_eq!(w.app().run(|| opened.get()).len(), 3, "pressed three times");
    assert!(w.app().run(|| opened.get()).iter().all(|t| t == "https://octara.dev/guide"));
}

#[test]
fn a_link_that_wraps_is_pressed_on_either_line() {
    let app = App::new(octara::platform::HeadlessPlatform::default());
    let opened = app.run(|| Signal::new(0u32));
    let mut w = app
        .open_window(octara::platform::WindowOptions::new("Wrap"), move || {
            col()
                .w(140.0)
                .child(text(rich().text("Read ").link("the long and winding guide", "g")).id("t"))
                .on_link(move |_| opened.update(|n| *n += 1))
                .into()
        })
        .unwrap();
    let link = w.kernel().find_by_role(Role::Link).unwrap();
    let rects = w.kernel().link_rects(link);
    assert!(rects.len() >= 2, "it wraps: {rects:?}");
    let (tx, ty, _, _) = w.kernel().shown_rect(w.kernel().find("t").unwrap());
    let last = *rects.last().unwrap();
    w.click_at(tx + last.x + 2.0, ty + last.y + last.h / 2.0);
    assert_eq!(app.run(|| opened.get()), 1, "its second line takes the press");
}

#[test]
fn restyling_reshapes_only_what_changes_shape() {
    let app = App::new(octara::platform::HeadlessPlatform::default());
    let bold = app.run(|| Signal::new(false));
    let red = app.run(|| Signal::new(false));
    let w = app
        .open_window(octara::platform::WindowOptions::new("Style"), move || {
            col()
                .child(
                    text(move || {
                        rich().text("plain ").styled("styled", |s| {
                            let s = if bold.get() { s.bold() } else { s };
                            if red.get() { s.color(Token::Danger) } else { s }
                        })
                    })
                    .id("t"),
                )
                .into()
        })
        .unwrap();
    let shaped = || w.kernel().text_engine_shaped();
    let before = shaped();
    app.run(|| red.set(true));
    app.settle();
    assert_eq!(shaped(), before, "a color is not a shape");
    let glyphs = w.display_list();
    assert!(glyphs.glyph_runs().any(|g| g.color == Theme::light().get(Token::Danger)));
    app.run(|| bold.set(true));
    app.settle();
    assert_eq!(shaped(), before + 1, "a weight reshapes the paragraph once");
}

#[test]
fn a_stream_takes_only_its_own_glyphs_again() {
    let app = App::new(octara::platform::HeadlessPlatform::default());
    let live = app.run(|| LiveText::new("Streaming"));
    let stream = live.clone();
    let w = app
        .open_window(octara::platform::WindowOptions::new("Stream"), move || {
            col()
                .child(text(&stream).id("live"))
                .children((0..20).map(|i| text(format!("Message {i} that does not change")).into()).collect::<Vec<View>>())
                .into()
        })
        .unwrap();
    let _ = w.display_list();
    let before = w.kernel().glyph_extractions();
    for word in [" one", " two", " three"] {
        app.run(|| live.append(word));
        app.advance(Duration::from_millis(16));
        let _ = w.display_list();
    }
    assert_eq!(w.kernel().glyph_extractions() - before, 3, "one text per frame, not twenty-one");
}

#[test]
fn rows_scrolling_back_and_repeated_labels_shape_nothing_again() {
    let mut w = Window::headless(|| {
        let items = list("rows", || (0..1000u32).map(|i| (i, format!("Row {i}"))).collect::<Vec<_>>(), |r: &(u32, String)| r.0);
        col()
            .size_full()
            .child(col().id("list").flex_1().overflow_y_scroll().child(virtual_each(
                move || items.source(),
                |_| 24.0,
                |k, r| row().h(24.0).child(text(move || r.with(|r| r.1.clone())).id(format!("r{k}"))).child(text("Edit")).into(),
            )))
            .into()
    });
    let shaped = |w: &Window| w.kernel().text_engine_shaped();
    // Every row has the label "Edit": shaped once, however many rows show it.
    let first = shaped(&w);
    assert!(first < 2 * 40, "a row's own text, and one shared label: {first}");
    // Down and back: the rows that come back were shaped before.
    w.dispatch(Event::ScrollTo("list".into(), 0.0, 5000.0)).unwrap();
    let after_down = shaped(&w);
    w.dispatch(Event::ScrollTo("list".into(), 0.0, 0.0)).unwrap();
    assert_eq!(shaped(&w), after_down, "rows scrolling back shape nothing");
}

#[test]
fn hovering_a_link_redraws_its_text() {
    let (mut w, _) = app();
    w.app().set_check_consistency(true);
    let link = w.kernel().find_by_role(Role::Link).unwrap();
    let (x, y, _, h) = w.kernel().shown_rect(link);
    let before = w.display_list();
    w.pointer_move(x + 2.0, y + h / 2.0);
    let after = w.display_list();
    let accent = Theme::light().get(Token::AccentHover);
    assert!(after.glyph_runs().any(|g| g.color == accent), "the hovered link in its hover color");
    assert!(!std::rc::Rc::ptr_eq(&before, &after), "a new frame, not the last one reused");
}

#[test]
fn a_field_losing_focus_redraws_without_its_caret() {
    let mut w = Window::headless(|| {
        let v = state("v", String::new);
        col()
            .selectable()
            .child(text_input().id("f").label("F").value(move || v.get()).on_input(move |s| v.set(s)))
            .child(text("words").id("t"))
            .into()
    });
    w.tap("f").unwrap();
    let caret = |w: &Window| {
        let f = w.kernel().find("f").unwrap();
        w.display_list().quads().any(|q| q.source == f && q.rect.w <= 2.0)
    };
    assert!(caret(&w));
    let (x, y, _, h) = w.kernel().shown_rect(w.kernel().find("t").unwrap());
    w.click_at(x + 2.0, y + h / 2.0);
    assert!(!caret(&w), "the caret went with the focus");
}

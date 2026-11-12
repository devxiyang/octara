//! Text styles set on a container apply to the text inside (as in GPUI).

use octara::prelude::*;

fn tok(t: Token) -> Color {
    Theme::light().get(t)
}

fn view() -> View {
    let muted = state("muted", || true);
    let more = state("more", || false);
    col()
        .child(
            row()
                .id("note")
                .text_sm()
                .font_semibold()
                .text_color(move || if muted.get() { Token::TextMuted } else { Token::Accent })
                .child(text("plain").id("plain"))
                .child(text("own").id("own").text_color(Token::Danger).text_lg())
                .child(show(move || more.get(), || text("late").id("late").into())),
        )
        .child(text("outside").id("outside"))
        .child(el().id("toggle").on_press(move || muted.update(|m| *m = !*m)))
        .child(el().id("more").on_press(move || more.set(true)))
        .into()
}

#[test]
fn text_inherits_from_its_container_and_overrides_win() {
    let mut app = Window::headless(view);
    let k = app.kernel();
    let plain = k.visual_of("plain").unwrap();
    assert_eq!((plain.font_size, plain.font_weight, plain.text), (14.0, 600, tok(Token::TextMuted)));
    assert_eq!(plain.line_height, 14.0 * 1.25);
    let own = k.visual_of("own").unwrap();
    assert_eq!((own.font_size, own.font_weight, own.text), (18.0, 600, tok(Token::Danger)));
    let outside = k.visual_of("outside").unwrap();
    assert_eq!((outside.font_size, outside.text), (16.0, tok(Token::Text)), "siblings of the container are untouched");
    drop(k);

    // Text added later inherits too.
    app.dispatch(Event::Press("more".into())).unwrap();
    assert_eq!(app.kernel().visual_of("late").unwrap().font_size, 14.0);

    // A bound style on the container reaches its text; only that binding reruns.
    app.app().reset_work();
    app.dispatch(Event::Press("toggle".into())).unwrap();
    let k = app.kernel();
    assert_eq!(k.visual_of("plain").unwrap().text, tok(Token::Accent));
    assert_eq!(k.visual_of("late").unwrap().text, tok(Token::Accent));
    assert_eq!(k.visual_of("own").unwrap().text, tok(Token::Danger), "an explicit color stays");
    drop(k);
    assert_eq!(app.app().work().closures_run, 1);

    // Inherited tokens follow the theme.
    app.app().set_theme(Theme::dark());
    assert_eq!(app.kernel().visual_of("plain").unwrap().text, Theme::dark().get(Token::Accent));
}

#[test]
fn a_string_child_takes_the_container_style() {
    let app = Window::headless(|| row().id("r").text_xs().child("tiny").into());
    let k = app.kernel();
    let r = k.find("r").unwrap();
    let t = k.children(r)[0];
    assert_eq!(k.rect(t).3, 15.0, "12 px × 1.25 = 15 px line");
}

//! Customizing the theme (Tailwind v4 `@theme`) and writing arbitrary values.

use octara::kernel::MISSING;
use octara::prelude::*;

const BRAND: Token = Token::custom("brand");
const GHOST: Token = Token::custom("ghost");

/// A reusable style, Tailwind's `@apply` as a plain function.
fn card<S: Styled>(s: S) -> S {
    s.p_4().rounded_lg().border().bg_paint(Token::Surface)
}

fn view() -> View {
    col()
        .child(el().child(text("a").id("a")).id("card").map(card).gap_2())
        .child(text("brand").id("brand").text_color(BRAND))
        .child(text("ghost").id("ghost").text_color(GHOST))
        .child(text("code").id("code").font_mono())
        .child(text("sans").id("sans"))
        .child(
            // Arbitrary values: a number is pixels.
            el().id("box").w(123).h(45.5).mx(7.0).p(3),
        )
        .child(el().id("wide").hidden().when_medium(|p| p.flex()).size_4())
        .into()
}

fn rect(app: &App, id: &str) -> (f32, f32, f32, f32) {
    let k = app.kernel();
    k.rect(k.find(id).unwrap())
}

#[test]
fn arbitrary_values_and_apply() {
    let app = App::new(view);
    let (_, _, w, h) = rect(&app, "box");
    assert_eq!((w, h), (123.0, 46.0), "layout rounds to whole pixels");
    // card: p_4 = 16 px of padding around the text.
    let (cx, cy, ..) = rect(&app, "card");
    let (ax, ay, ..) = rect(&app, "a");
    assert_eq!((ax - cx, ay - cy), (17.0, 17.0), "16 px padding + 1 px border");
    assert_eq!(app.kernel().visual_of("card").unwrap().radius, 8.0);
}

#[test]
fn custom_tokens_and_missing_ones_are_reported() {
    let mut app = App::new(view);
    assert_eq!(app.kernel().visual_of("brand").unwrap().text, MISSING);
    assert_eq!(app.kernel().missing_tokens(), vec!["brand", "ghost"]);
    app.set_theme(Theme::light().color(BRAND, rgb(0xff6600)));
    assert_eq!(app.kernel().visual_of("brand").unwrap().text, rgb(0xff6600));
    assert_eq!(app.kernel().missing_tokens(), vec!["ghost"]);
}

#[test]
fn density_and_breakpoints_change_with_the_theme_without_running_closures() {
    let mut app = App::new(view);
    app.reset_work();
    app.set_theme(Theme::light().spacing(3.0));
    let (cx, cy, ..) = rect(&app, "card");
    let (ax, ay, ..) = rect(&app, "a");
    assert_eq!((ax - cx, ay - cy), (13.0, 13.0), "p_4 is now 12 px");
    assert_eq!(app.work().closures_run, 0);

    // 800 px is medium by default: `wide` shows. Moving medium to 900 px makes it compact.
    assert_eq!(rect(&app, "wide").2, 12.0);
    app.set_theme(Theme::light().breakpoint(Breakpoint::Medium, 900.0));
    assert_eq!(app.breakpoint(), Breakpoint::Compact);
    assert_eq!(rect(&app, "wide").2, 0.0, "hidden when compact");
}

#[test]
fn mono_text_uses_the_theme_mono_family() {
    let app = App::new(view);
    let k = app.kernel();
    assert_eq!(k.visual_of("code").unwrap().font_family, "IBM Plex Mono");
    assert_eq!(k.visual_of("sans").unwrap().font_family, "Inter");
    // Monospace text is wider than Inter for the same short word (both stretch to the column,
    // so compare the text itself).
    let code = k.text_metrics(k.find("code").unwrap()).width;
    let sans = k.text_metrics(k.find("sans").unwrap()).width;
    assert!(code > sans, "{code} vs {sans}");
}

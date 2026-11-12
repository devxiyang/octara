//! Renders icons to a PNG with the reference renderer:
//!
//! ```text
//! cargo run -p octara-icons --example gallery -- gallery.png [query]
//! ```
//!
//! With a query, the icons `icons::search` finds; without, a sample in several sizes, weights and
//! colors, inside buttons and on both themes.

use octara::prelude::*;
use octara_icons as icons;

fn tile(e: &'static icons::Entry) -> El {
    col()
        .w(88)
        .items_center()
        .gap_1()
        .child(icon(e.icon.clone()).size_6())
        .child(text(e.name).text_xs().text_color(Token::TextMuted).truncate())
}

fn sample() -> View {
    let names = ["search", "house", "settings", "user", "bell", "heart", "star", "trash", "pencil", "plus", "x", "check"];
    let row_of = |size: fn(El) -> El, weight: f32| {
        row().gap_4().children(names.iter().map(move |n| size(icon(icons::by_name(n).unwrap())).stroke_width(weight)))
    };
    let button = |label: &'static str, i: Icon, primary: bool| {
        let b = row().gap_2().items_center().px_4().py_2().rounded_md().child(icon(i)).child(text(label));
        if primary { b.bg(Token::Accent).text_color(Token::AccentText) } else { b.border().border_color(Token::Border) }
    };
    col()
        .size_full()
        .p_6()
        .gap_5()
        .bg(Token::Background)
        .child(text("16 px · 20 px · 24 px · 32 px, stroke 2").text_sm().text_color(Token::TextMuted))
        .child(row_of(|e| e.size_4(), 2.0))
        .child(row_of(|e| e.size_5(), 2.0))
        .child(row_of(|e| e.size_6(), 2.0))
        .child(row_of(|e| e.size_8(), 2.0))
        .child(text("24 px, stroke 1 · 1.5 · 2.5").text_sm().text_color(Token::TextMuted))
        .child(row_of(|e| e.size_6(), 1.0))
        .child(row_of(|e| e.size_6(), 1.5))
        .child(row_of(|e| e.size_6(), 2.5))
        .child(text("In buttons: the icon takes the text color").text_sm().text_color(Token::TextMuted))
        .child(
            row()
                .gap_3()
                .child(button("Search", icons::SEARCH, true))
                .child(button("Save", icons::SAVE, false))
                .child(button("Delete", icons::TRASH, false).text_color(Token::Danger))
                .child(
                    row().gap_3().children(
                        [rgb(0xef4444), rgb(0xf59e0b), rgb(0x10b981), rgb(0x3b82f6), rgb(0x8b5cf6)]
                            .map(|c| icon(icons::HEART).size_6().text_color(c)),
                    ),
                ),
        )
        .into()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "gallery.png".into());
    let query: Option<String> = args.next();
    let mut shots = Vec::new();
    for theme in [Theme::light(), Theme::dark()] {
        let q = query.clone();
        let mut app = Window::headless(move || match &q {
            Some(q) => {
                let found = icons::search(q);
                row()
                    .size_full()
                    .p_4()
                    .gap_2()
                    .flex_wrap()
                    .items_start()
                    .bg(Token::Background)
                    .children(found.into_iter().take(120).map(tile))
                    .into()
            }
            None => sample(),
        });
        app.resize(720.0, 520.0);
        app.app().set_theme(theme);
        shots.push(octara_render::screenshot(&app, 2.0));
    }
    // Light above dark.
    let (w, h) = (shots[0].width, shots[0].height);
    let mut rgba = shots[0].rgba.clone();
    rgba.extend_from_slice(&shots[1].rgba);
    octara_render::Bitmap { width: w, height: h * 2, rgba }.save_png(&out).unwrap();
    eprintln!("wrote {out}");
}

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
use octara_model::{
    style::{AlignItems, FlexWrap},
    text::Wrap,
    theme::{RadiusToken, rgb},
};
use octara_pipeline::{NativeEngine, NativeEngineConfig};

fn tile(e: &'static icons::Entry) -> El {
    col()
        .w(88)
        .align(AlignItems::Center)
        .gap(sp(1.0))
        .child(icon(e.icon.into()).size(sp(6.0)))
        .child(text(e.name).text_size(FontSizeToken::Xs).text_color(ColorToken::TextMuted).text_wrap(Wrap::Truncate))
}

fn sample() -> View {
    let sample = [
        icons::SEARCH,
        icons::HOUSE,
        icons::SETTINGS,
        icons::USER,
        icons::BELL,
        icons::HEART,
        icons::STAR,
        icons::TRASH,
        icons::PENCIL,
        icons::PLUS,
        icons::X,
        icons::CHECK,
    ];
    let row_of =
        |size: fn(El) -> El, weight: f32| row().gap(sp(4.0)).children(sample.map(|entry| size(icon(entry.with_stroke_width(weight)))));
    let button = |label: &'static str, i: icons::LucideIcon, primary: bool| {
        let b = row()
            .gap(sp(2.0))
            .align(AlignItems::Center)
            .px(sp(4.0))
            .py(sp(2.0))
            .rounded(RadiusToken::Md)
            .child(icon(i.into()))
            .child(text(label));
        if primary { b.bg(ColorToken::Accent).text_color(ColorToken::AccentText) } else { b.border(1.0).border_color(ColorToken::Border) }
    };
    col()
        .size(rel(1.0))
        .p(sp(6.0))
        .gap(sp(5.0))
        .bg(ColorToken::Background)
        .child(text("16 px · 20 px · 24 px · 32 px, stroke 2").text_size(FontSizeToken::Sm).text_color(ColorToken::TextMuted))
        .child(row_of(|e| e.size(sp(4.0)), 2.0))
        .child(row_of(|e| e.size(sp(5.0)), 2.0))
        .child(row_of(|e| e.size(sp(6.0)), 2.0))
        .child(row_of(|e| e.size(sp(8.0)), 2.0))
        .child(text("24 px, stroke 1 · 1.5 · 2.5").text_size(FontSizeToken::Sm).text_color(ColorToken::TextMuted))
        .child(row_of(|e| e.size(sp(6.0)), 1.0))
        .child(row_of(|e| e.size(sp(6.0)), 1.5))
        .child(row_of(|e| e.size(sp(6.0)), 2.5))
        .child(text("In buttons: the icon takes the text color").text_size(FontSizeToken::Sm).text_color(ColorToken::TextMuted))
        .child(
            row()
                .gap(sp(3.0))
                .child(button("Search", icons::SEARCH, true))
                .child(button("Save", icons::SAVE, false))
                .child(button("Delete", icons::TRASH, false).text_color(ColorToken::Danger))
                .child(
                    row().gap(sp(3.0)).children(
                        [rgb(0xef4444), rgb(0xf59e0b), rgb(0x10b981), rgb(0x3b82f6), rgb(0x8b5cf6)]
                            .map(|c| icon(icons::HEART.into()).size(sp(6.0)).text_color(c)),
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
        let mut app = App::new(|_, _| Box::new(NativeEngine::new(NativeEngineConfig::default())));
        let options = WindowOptions {
            viewport: WindowViewport { size: Size::new(720.0, 520.0), scale: 2.0, safe_area: None },
            theme,
            ..Default::default()
        };
        let window = app
            .open_window(options, move |_| match &q {
                Some(q) => row()
                    .size(rel(1.0))
                    .p(sp(4.0))
                    .gap(sp(2.0))
                    .wrap(FlexWrap::Wrap)
                    .align(AlignItems::Start)
                    .bg(ColorToken::Background)
                    .children(icons::search(q).into_iter().take(120).map(tile))
                    .into(),
                None => sample(),
            })
            .unwrap();
        let draw = app.runtime().window(window).unwrap().last_commit().unwrap().draw.as_deref().unwrap();
        shots.push(octara_render::CpuRenderer::new().paint(draw, 2.0).unwrap());
    }
    // Light above dark.
    let mut writer = std::io::BufWriter::new(std::fs::File::create(&out).unwrap());
    let mut encoder = png::Encoder::new(&mut writer, shots[0].width(), shots[0].height() * 2);
    encoder.set_color(png::ColorType::Rgba);
    let rgba = shots.into_iter().flat_map(|picture| picture.into_rgba()).collect::<Vec<_>>();
    encoder.write_header().unwrap().write_image_data(&rgba).unwrap();
    eprintln!("wrote {out}");
}

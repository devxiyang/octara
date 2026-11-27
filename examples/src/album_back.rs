//! A retained album list and a photo detail that retires when Back completes.

use octara::prelude::*;
use octara_kit::{ButtonVariant, Size as KitSize, button};

pub const LIST: &str = "album-list";
pub const DETAIL: &str = "album-detail";
pub const DETAIL_SCROLL: &str = "album-detail-scroll";
pub const BACK: &str = "album-back";
pub const COUNTERS: &str = "album-counters";
pub const DETAIL_COUNTERS: &str = "album-detail-counters";

const BACK_COMMAND: &str = "navigation.back";
const PHOTOS: [(&str, u32); 12] = [
    ("Salt Marsh", 0x426f64),
    ("High Tide", 0x49858a),
    ("Daybreak", 0xbb7650),
    ("Tide Pools", 0x6d8e84),
    ("Grey Dunes", 0xb58d69),
    ("Breakwater", 0x466579),
    ("Sea Glass", 0x567d68),
    ("Driftwood", 0x987954),
    ("Gull Island", 0x557a85),
    ("Headland", 0x6b7d6f),
    ("Low Fog", 0x7f8f8e),
    ("Last Light", 0xad692e),
];

const NOTES: [(&str, &str); 6] = [
    ("Location", "The shoreline trail follows the dunes to the water."),
    ("Weather", "Low clouds softened the horizon and muted the afternoon light."),
    ("Framing", "Leave enough space above the horizon for the clouds to read."),
    ("Exposure", "Keep detail in the brightest patches of water and sky."),
    ("Color", "Keep the cool colors in the surf and the warmer tones on the shore."),
    ("Print", "Use a matte finish to keep the detail in the shadows visible."),
];

#[derive(Default)]
struct Counts {
    back: u32,
    released: u32,
}

#[ui]
pub fn view(cx: &mut BuildCx<'_>) -> El {
    let selected = cx.signal(None::<usize>);
    let counts = cx.signal(Counts::default());
    let close = move |w: &mut WriteCx<'_>| {
        w.update(counts, |counts| {
            counts.back += 1;
            counts.released += 1;
        });
        w.set(selected, None);
    };

    col()
        .size(rel(1.0))
        .bg(ColorToken::Background)
        .text_color(ColorToken::Text)
        .child(
            text("Coastal Album")
                .role(Role::Heading)
                .level(1)
                .text_size(FontSizeToken::Xl)
                .text_weight(600)
                .shrink(0.0)
                .p(sp(4.0))
                .when(When::Touch, |p| p.pt(60.0)),
        )
        .child(
            col().test_id(LIST).label("Album photos").grow(1.0).min_h(0.0).overflow_y(Overflow::Scroll).gap(sp(2.0)).px(sp(4.0)).children(
                PHOTOS.iter().copied().enumerate().map(move |(index, (title, color))| {
                    row()
                        .test_id(format!("album-photo-{}", index + 1))
                        .role(Role::Button)
                        .label(title)
                        .focusable()
                        .h(80.0)
                        .shrink(0.0)
                        .align(AlignItems::Center)
                        .gap(sp(4.0))
                        .p(sp(3.0))
                        .rounded(RadiusToken::Lg)
                        .bg(ColorToken::Surface)
                        .when(State::PRESSED, |p| p.bg(ColorToken::SurfacePressed))
                        .child(el().size(52.0).shrink(0.0).rounded(RadiusToken::Md).bg(rgb(color)))
                        .child(text(title).text_weight(500))
                        .on_press(move |w| w.set(selected, Some(index)))
                }),
            ),
        )
        .child(counters(COUNTERS, counts))
        .portal(
            move |r| r.get(selected).is_some(),
            move |b| {
                let index = b.sample(selected).expect("an open detail has a selected photo");
                detail(PHOTOS[index], counts, close)
            },
        )
}

#[ui]
fn detail((title, color): (&str, u32), counts: Signal<Counts>, close: impl Fn(&mut WriteCx<'_>) + Copy + 'static) -> El {
    col()
        .test_id(DETAIL)
        .role(Role::Dialog)
        .label(title)
        .size(rel(1.0))
        .bg(ColorToken::Surface)
        .text_color(ColorToken::Text)
        .layer(LayerPolicy { dismiss_on_outside_press: false, ..LayerPolicy::modal() }, close)
        .on_command(BACK_COMMAND, close)
        .preserve_focus_on_press()
        .child(
            row()
                .shrink(0.0)
                .align(AlignItems::Center)
                .gap(sp(3.0))
                .p(sp(4.0))
                .when(When::Touch, |p| p.pt(60.0))
                .child(button("Back", KitSize::Md, ButtonVariant::Secondary).test_id(BACK).on_press(close))
                .child(text(title).role(Role::Heading).level(1).text_size(FontSizeToken::Xl).text_weight(600)),
        )
        .child(
            col()
                .test_id(DETAIL_SCROLL)
                .label("Photo notes")
                .focusable()
                .autofocus()
                .grow(1.0)
                .min_h(0.0)
                .overflow_y(Overflow::Scroll)
                .gap(sp(3.0))
                .px(sp(4.0))
                .child(el().h(320.0).shrink(0.0).rounded(RadiusToken::Lg).bg(rgb(color)))
                .children(NOTES.map(|(title, note)| {
                    col()
                        .shrink(0.0)
                        .gap(sp(1.0))
                        .rounded(RadiusToken::Md)
                        .bg(ColorToken::Background)
                        .p(sp(4.0))
                        .child(text(title).text_weight(600))
                        .child(text(note).text_size(FontSizeToken::Sm))
                })),
        )
        .child(counters(DETAIL_COUNTERS, counts))
}

fn counters(id: &str, counts: Signal<Counts>) -> El {
    text(move |r: &mut ReadCx<'_>| r.with(counts, |counts| format!("Back {} · Released {}", counts.back, counts.released)))
        .test_id(id)
        .shrink(0.0)
        .px(sp(4.0))
        .py(sp(3.0))
        .when(When::Touch, |p| p.pb(34.0))
        .text_size(FontSizeToken::Sm)
        .text_color(ColorToken::TextMuted)
}

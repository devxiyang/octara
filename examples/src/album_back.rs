//! A retained album list and a photo detail that retires when Back completes.

use octara::prelude::*;
use octara::reactive::untrack;
use octara_kit::button;

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
pub fn view() -> El {
    let selected = state("selected-photo", || None);
    let counts = state("counts", Counts::default);
    let close = move || {
        counts.update(|counts| counts.back += 1);
        selected.set(None);
    };

    col()
        .size_full()
        .bg(Token::Background)
        .text_color(Token::Text)
        .child(
            text("Coastal Album").role(Role::Heading).level(1).text_xl().font_semibold().flex_shrink_0().p_4().when_touch(|p| p.pt(60.0)),
        )
        .child(col().id(LIST).label("Album photos").flex_1().min_h(0.0).overflow_y_scroll().gap_2().px_4().children(
            PHOTOS.iter().copied().enumerate().map(move |(index, (title, color))| {
                row()
                    .id(format!("album-photo-{}", index + 1))
                    .role(Role::Button)
                    .label(title)
                    .focusable()
                    .h(80.0)
                    .flex_shrink_0()
                    .items_center()
                    .gap_4()
                    .p_3()
                    .rounded_lg()
                    .bg(Token::Surface)
                    .active(|p| p.bg(Token::SurfacePressed))
                    .child(el().size(52.0).flex_shrink_0().rounded_md().bg(rgb(color)))
                    .child(text(title).font_medium())
                    .on_press(move || selected.set(Some(index)))
            }),
        ))
        .child(counters(COUNTERS, counts))
        .overlay_window(
            move || selected.get().is_some(),
            move || {
                let index = untrack(|| selected.get()).expect("an open detail has a selected photo");
                detail(PHOTOS[index], counts, close)
            },
        )
}

#[ui]
fn detail((title, color): (&str, u32), counts: Signal<Counts>, close: impl Fn() + Copy + 'static) -> El {
    on_cleanup(move || counts.update(|counts| counts.released += 1));

    col()
        .id(DETAIL)
        .role(Role::Dialog)
        .label(title)
        .size_full()
        .bg(Token::Surface)
        .text_color(Token::Text)
        .modal(false, close)
        .on_command(BACK_COMMAND, close)
        .preserve_focus_on_press(true)
        .child(
            row()
                .flex_shrink_0()
                .items_center()
                .gap_3()
                .p_4()
                .when_touch(|p| p.pt(60.0))
                .child(button("Back").id(BACK).on_press(close))
                .child(text(title).role(Role::Heading).level(1).text_xl().font_semibold()),
        )
        .child(
            col()
                .id(DETAIL_SCROLL)
                .label("Photo notes")
                .focusable()
                .autofocus()
                .flex_1()
                .min_h(0.0)
                .overflow_y_scroll()
                .gap_3()
                .px_4()
                .child(el().h(320.0).flex_shrink_0().rounded_lg().bg(rgb(color)))
                .children(NOTES.map(|(title, note)| {
                    col()
                        .flex_shrink_0()
                        .gap_1()
                        .rounded_md()
                        .bg(Token::Background)
                        .p_4()
                        .child(text(title).font_semibold())
                        .child(text(note).text_sm())
                })),
        )
        .child(counters(DETAIL_COUNTERS, counts))
}

fn counters(id: &str, counts: Signal<Counts>) -> El {
    text(move || counts.with(|counts| format!("Back {} · Released {}", counts.back, counts.released)))
        .id(id)
        .flex_shrink_0()
        .px_4()
        .py_3()
        .when_touch(|p| p.pb(34.0))
        .text_sm()
        .text_color(Token::TextMuted)
}

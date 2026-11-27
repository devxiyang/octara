//! Kit components and core primitives in one native window.

use octara::prelude::*;
use octara_kit::{ButtonVariant, Size as KitSize, button};

mod choices;
mod controls;
mod drawing;
mod forms;
mod images;
mod interaction;
mod layers;
mod map;
mod markdown;
mod menus;
mod search;
mod text;
mod web;
mod workspace;

type Card = (&'static str, fn(&mut BuildCx<'_>) -> El);

pub const CARDS: &[Card] = &[
    ("workspace", workspace::view),
    ("forms", forms::view),
    ("choices", choices::view),
    ("controls", controls::view),
    ("interaction", interaction::view),
    ("markdown", markdown::view),
    ("text", text::view),
    ("drawing", drawing::view),
    ("images", images::view),
    ("layers", layers::view),
    ("map", map::view),
    ("menus", menus::view),
    ("search", search::view),
    ("web", web::view),
];

pub fn page(cx: &mut BuildCx<'_>, name: &str) -> Option<El> {
    CARDS.iter().find(|(card, _)| *card == name).map(|(_, build)| col().size(rel(1.0)).p(sp(4.0)).child(build(cx)))
}

#[ui]
pub fn showcase(cx: &mut BuildCx<'_>) -> El {
    let current = cx.signal("forms");
    col()
        .test_id("showcase")
        .size(rel(1.0))
        .p(sp(4.0))
        .gap(sp(3.0))
        .child(text::heading("Octara Showcase"))
        .child(row().wrap(FlexWrap::Wrap).gap(sp(2.0)).children(CARDS.iter().map(move |(name, _)| {
            let name = *name;
            button(name, KitSize::Md, ButtonVariant::Secondary).test_id(format!("show-{name}")).on_press(move |w| w.set(current, name))
        })))
        .child(
            col()
                .grow(1.0)
                .min_h(0)
                .overflow_y(Overflow::Scroll)
                .child(keyed(move |r| r.get(current), |b, name| page(b, name).expect("a known showcase card"))),
        )
}

fn section(title: &str) -> El {
    col().test_id(format!("showcase-{}", title.to_lowercase())).gap(sp(3.0)).p(sp(2.0)).child(text::heading(title))
}

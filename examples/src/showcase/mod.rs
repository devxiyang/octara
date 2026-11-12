//! A tour of what Octara draws and does: shadows, gradients, images, a canvas chart, icons, group
//! opacity, cut text, text editing, a select, focus, timers, an async resource, a web view, a
//! map, and motion. One file per card; [`page`] shows one card alone (`window -- showcase web`).

use octara::prelude::*;

mod cut;
mod drawing;
mod editing;
mod fields;
mod hero;
mod images;
mod layers;
mod map;
mod markdown;
mod search;
mod text;
mod web;

/// A card: its name and how to build it.
pub type Card = (&'static str, fn() -> El);

/// The cards by name, in the tour's order.
pub const CARDS: [Card; 12] = [
    ("hero", hero::card),
    ("images", images::card),
    ("layers", layers::card),
    ("drawing", drawing::card),
    ("editing", editing::card),
    ("search", search::card),
    ("text", text::card),
    ("markdown", markdown::card),
    ("cut", cut::card),
    ("fields", fields::card),
    ("web", web::card),
    ("map", map::card),
];

/// The frame every card shares.
#[component]
pub(crate) fn section(title: &'static str) -> El {
    // Lifts while the mouse is anywhere over it (the card has no handlers of its own). Boxes do
    // not shrink by default; a card does, so on a phone it narrows and its rows wrap.
    col()
        .flex_shrink()
        .hover(|p| p.shadow_lg())
        .gap_3()
        .p_5()
        .rounded_xl()
        .bg(Token::Background)
        .border()
        .border_color(Token::Border)
        .shadow_sm()
        .child(text(title).text_sm().font_semibold().text_color(Token::TextMuted))
}

/// The page around the cards: it scrolls, and its layout moves.
#[component]
fn frame(content: El) -> View {
    col()
        .size_full()
        .bg(Token::Surface)
        .overflow_y_scroll()
        .child(
            content
                // Everything here moves: theme and state colors, opacity, rows entering and
                // leaving, layout making room.
                .motion(Spring::SNAP)
                .p_8()
                .gap_6()
                .max_w(880),
        )
        .into()
}

/// Every card.
#[component]
pub fn showcase() -> View {
    let wrap = || row().gap_6().flex_wrap().items_start();
    frame(
        col()
            .child(hero::card())
            .child(wrap().child(images::card()).child(layers::card()))
            .child(wrap().child(drawing::card()))
            .child(wrap().child(editing::card()).child(col().gap_6().child(search::card()).child(cut::card())))
            .child(wrap().child(text::card()).child(markdown::card()))
            .child(wrap().child(fields::card()).child(web::card()).child(map::card())),
    )
}

/// One card alone (a name from [`CARDS`]); `None` for an unknown name.
pub fn page(name: &str) -> Option<View> {
    let (_, card) = CARDS.iter().find(|(n, _)| *n == name)?;
    Some(frame(col().child(card())))
}

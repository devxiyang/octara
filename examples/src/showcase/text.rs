//! Rich text: one paragraph of styled runs, inline code and links that are elements of their
//! own (pressable, focusable, named by their words); the card's texts select as one document.

use octara::prelude::*;

#[component]
pub fn card() -> El {
    let opened = state("opened", String::new);
    super::section("Rich text")
        .w(380.0)
        .selectable()
        .child(
            text(
                rich()
                    .text("Octara draws ")
                    .bold("bold")
                    .text(", ")
                    .italic("italic")
                    .text(", ")
                    .styled("struck", |s| s.strike())
                    .text(" and ")
                    .styled("colored", |s| s.color(Token::Danger))
                    .text(" runs in one paragraph, with ")
                    .code("inline code")
                    .text(" and links such as ")
                    .link("the text core's design", "docs/design/text.md")
                    .text(" or ")
                    .link("Parley", "https://github.com/linebender/parley")
                    .text("."),
            )
            .id("rich")
            .line_height(relative(1.5)),
        )
        .child(
            text(move || if opened.get().is_empty() { "Press a link.".to_owned() } else { format!("Opened {}", opened.get()) })
                .id("opened")
                .text_sm()
                .text_color(Token::TextMuted),
        )
        .on_link(move |target| opened.set(target.to_owned()))
}

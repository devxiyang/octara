//! Selectable rich text and inline links.

use octara::prelude::*;

pub fn heading(value: &str) -> El {
    text(value).role(Role::Heading).level(1).text_size(FontSizeToken::Xl).text_weight(600)
}

pub fn view(_: &mut BuildCx<'_>) -> El {
    super::section("Rich text").w(380).selectable(true).child(
        text(
            rich()
                .text("Octara draws ")
                .span("bold", RichSpan { weight: Some(700), ..Default::default() })
                .text(", ")
                .span("italic", RichSpan::default().font_style(FontStyle::Italic))
                .text(" and ")
                .span("struck", RichSpan { strike: true, ..Default::default() })
                .text(" runs in one paragraph, with a link to ")
                .link("Parley", "https://github.com/linebender/parley")
                .text("."),
        )
        .test_id("rich"),
    )
}

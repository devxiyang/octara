//! Group opacity: overlapping children fade as one layer.

use super::section;
use octara::prelude::*;
use octara_kit::switch;

#[ui]
pub fn view(cx: &mut BuildCx<'_>) -> El {
    let faded = cx.signal(true);
    section("Group opacity")
        .child(
            row().gap(sp(6.0)).align(AlignItems::Center).child(
                el().w(150)
                    .h(90)
                    .opacity(move |r: &mut ReadCx<'_>| if r.get(faded) { 0.5_f32 } else { 1.0 })
                    .child(
                        el().position(Position::Absolute)
                            .top(sp(0.0))
                            .left(sp(0.0))
                            .size(sp(16.0))
                            .rounded(RadiusToken::Lg)
                            .bg(rgb(0xef4444)),
                    )
                    .child(el().position(Position::Absolute).top(26.0).left(40.0).size(sp(16.0)).rounded(Radius::Full).bg(rgb(0x3b82f6)))
                    .child(text("Layer").position(Position::Absolute).bottom(sp(0.0)).right(sp(0.0)).text_weight(600)),
            ),
        )
        .child(
            row()
                .gap(sp(2.0))
                .align(AlignItems::Center)
                .child(switch(faded.into()).label("Half opacity"))
                .child(text("Half opacity").text_size(FontSizeToken::Sm)),
        )
}

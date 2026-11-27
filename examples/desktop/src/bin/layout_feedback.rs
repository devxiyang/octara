//! Native-window qualification of geometry feedback.

use octara::prelude::*;
use octara_examples::window_options;
use octara_kit::{ButtonVariant, Size as KitSize, button};

#[ui]
fn layout_feedback(cx: &mut BuildCx<'_>) -> El {
    let wide = cx.signal(false);
    let geometry = cx.signal(None::<LayoutBox>);
    col()
        .size(rel(1.0))
        .p(sp(6.0))
        .gap(sp(4.0))
        .child(text("Layout geometry feedback").text_size(FontSizeToken::Xl).text_weight(600))
        .child(
            button("Change width", KitSize::Md, ButtonVariant::Secondary)
                .test_id("feedback-change")
                .on_press(move |w| w.update(wide, |value| *value = !*value)),
        )
        .child(
            text(move |r: &mut ReadCx<'_>| format!("{:.0} pt wide", r.get(geometry).unwrap_or_default().rect.w)).test_id("feedback-status"),
        )
        .child(
            el().test_id("feedback-box")
                .h(80)
                .bg(ColorToken::Accent)
                .observe_geometry(geometry)
                .style_with(move |r, p| p.w(if r.get(wide) { 80 } else { 40 })),
        )
}

fn main() -> Result<(), octara_host_native::ShellError> {
    octara_examples_desktop::native::run(window_options("Octara layout feedback", 800.0, 500.0), layout_feedback)
}

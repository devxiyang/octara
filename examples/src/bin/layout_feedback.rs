//! Native-window qualification for bounded layout feedback and later recovery.
use octara::prelude::*;
use octara_kit::button;

#[ui]
fn layout_feedback() -> View {
    let running = state("running", || false);
    let size = size_of("feedback-box");
    on_command("feedback.start", move || running.set(true));
    on_command("feedback.stop", move || running.set(false));
    col()
        .size_full()
        .p_6()
        .gap_4()
        .bg(Token::Surface)
        .text_color(Token::Text)
        .child(text("Layout completion and recovery").text_xl().font_semibold())
        .child(text("Start creates bounded size feedback. Stop lets the pending window finish."))
        .child(
            row()
                .gap_3()
                .child(button("Start feedback").id("feedback-start").on_press(move || running.set(true)))
                .child(button("Stop feedback").id("feedback-stop").on_press(move || running.set(false))),
        )
        .child(text(move || if running.get() { "Feedback enabled" } else { "Stable" }).id("feedback-status"))
        .child(
            el().id("feedback-box")
                .h(80)
                .bg(Token::Accent)
                .style_with(move |p| p.w(if running.get() && size.get().0 == 40.0 { 80.0 } else { 40.0 })),
        )
        .into()
}

fn main() {
    octara_shell::run_with(
        octara_shell::Options { window: WindowOptions::new("Octara layout feedback").size(800.0, 500.0), ..Default::default() },
        layout_feedback,
    );
}

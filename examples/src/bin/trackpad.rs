//! A nested viewport fixture for the AppKit wheel trace.

use octara::prelude::*;
use octara_kit::button;

const INNER: &str = "trackpad-inner";
const OUTER: &str = "trackpad-outer";
const INNER_HEIGHT: f32 = 180.0;
const ROW_HEIGHT: f32 = 44.0;
const INNER_ROWS: u32 = 12;

#[ui]
fn fixture() -> El {
    let visible = state("inner-visible", || true);
    on_command("trackpad.edge", || {
        scroll_to(OUTER, 0.0, 0.0);
        scroll_to(INNER, 0.0, ROW_HEIGHT * INNER_ROWS as f32 - INNER_HEIGHT - 12.0);
    });
    on_command("trackpad.reset", move || {
        visible.set(true);
        scroll_to(OUTER, 0.0, 0.0);
        scroll_to(INNER, 0.0, 0.0);
    });
    on_command("trackpad.remove", move || visible.set(false));

    col()
        .size_full()
        .p_4()
        .gap_3()
        .bg(Token::Surface)
        .text_color(Token::Text)
        .child(
            row()
                .gap_2()
                .child(button("Near inner edge").id("trackpad-edge").on_press(|| _ = invoke("trackpad.edge")))
                .child(button("Reset").id("trackpad-reset").on_press(|| _ = invoke("trackpad.reset"))),
        )
        .child(
            col()
                .id(OUTER)
                .flex_1()
                .overflow_y_scroll()
                .child(show(
                    move || visible.get(),
                    || {
                        col().id(INNER).h(INNER_HEIGHT).overflow_y_scroll().bg(Token::SurfaceHover).children((1..=INNER_ROWS).map(
                            |index| {
                                el().id(format!("trackpad-inner-{index}"))
                                    .h(ROW_HEIGHT)
                                    .px_3()
                                    .justify_center()
                                    .child(text(format!("Inner row {index}")))
                            },
                        ))
                    },
                ))
                .children((1..=20).map(|index| el().h(ROW_HEIGHT).px_3().justify_center().child(text(format!("Outer row {index}"))))),
        )
}

fn main() {
    octara_shell::run_with(
        octara_shell::Options { title: "Octara Trackpad Validation".into(), size: (480.0, 500.0), ..Default::default() },
        fixture,
    );
}

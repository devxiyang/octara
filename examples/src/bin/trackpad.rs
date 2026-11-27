//! Nested scrolling for testing a native trackpad's direct and momentum phases.

use octara::prelude::*;
use octara_examples::window_options;
use octara_kit::{ButtonVariant, Size as KitSize, button};

fn view(cx: &mut BuildCx<'_>) -> El {
    let revision = cx.signal(0_u32);
    let visible = cx.signal(true);
    col()
        .size(rel(1.0))
        .gap(sp(3.0))
        .p(sp(4.0))
        .on_command("trackpad.remove", move |w| w.set(visible, false))
        .child(text("Nested trackpad scrolling").role(Role::Heading).text_size(FontSizeToken::Xl))
        .child(button("Reset", KitSize::Md, ButtonVariant::Secondary).test_id("trackpad-reset").on_press(move |w| {
            w.set(visible, true);
            w.update(revision, |value| *value += 1);
        }))
        .child(keyed(
            move |r| r.get(revision),
            move |_, _| {
                col()
                    .test_id("trackpad-outer")
                    .min_h(0)
                    .grow(1.0)
                    .overflow_y(Overflow::Scroll)
                    .gap(sp(2.0))
                    .child(text("Scroll here to move the outer viewport").h(64).shrink(0.0))
                    .child(show(
                        move |r| r.get(visible),
                        |_| {
                            col()
                                .test_id("trackpad-inner")
                                .label("Inner viewport")
                                .h(180)
                                .shrink(0.0)
                                .overflow_y(Overflow::Scroll)
                                .children((1..=30).map(|n| text(format!("Inner row {n}")).h(27).shrink(0.0).p(sp(1.0))))
                        },
                    ))
                    .children((1..=30).map(|n| text(format!("Outer row {n}")).h(31).shrink(0.0).p(sp(1.0))))
            },
        ))
}

fn main() -> Result<(), octara_host_native::ShellError> {
    octara_examples::native::run(window_options("Octara Trackpad Validation", 480.0, 500.0), view)
}

//! A native fixture for guarded targets with duplicated labels and allocation reuse.

use octara::prelude::*;
use octara_examples::window_options;
use octara_kit::{ButtonVariant, Size as KitSize, button};

fn view(cx: &mut BuildCx<'_>) -> El {
    let count = cx.signal(0_u32);
    let version = cx.signal(0_u32);
    let enabled = cx.signal(true);
    col()
        .size(rel(1.0))
        .p(sp(4.0))
        .gap(sp(3.0))
        .child(text(move |r: &mut ReadCx<'_>| format!("Actions: {}", r.get(count))).test_id("count"))
        .child(
            row()
                .gap(sp(3.0))
                .child(
                    button("Rebuild the first target", KitSize::Md, ButtonVariant::Secondary)
                        .test_id("replace")
                        .on_press(move |w| w.update(version, |v| *v += 1)),
                )
                .child(
                    button("Hide or show second target", KitSize::Md, ButtonVariant::Secondary)
                        .test_id("toggle")
                        .on_press(move |w| w.update(enabled, |v| *v = !*v)),
                ),
        )
        .child(keyed(
            move |r| r.get(version),
            move |_, _| {
                button("Run", KitSize::Md, ButtonVariant::Secondary).test_id("first").on_press(move |w| w.update(count, |n| *n += 1))
            },
        ))
        .child(show(
            move |r| r.get(enabled),
            move |_| {
                button("Run", KitSize::Md, ButtonVariant::Secondary).test_id("second").on_press(move |w| w.update(count, |n| *n += 10))
            },
        ))
}

fn main() -> Result<(), octara_host_native::ShellError> {
    octara_examples_desktop::native::run(window_options("Octara Target Validation", 840.0, 620.0), view)
}

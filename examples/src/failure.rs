//! Fault injection for the native runtime failure boundary.

use octara::prelude::*;
use octara_kit::{ButtonVariant, Size as KitSize, button};

#[ui]
pub fn view(cx: &mut BuildCx<'_>) -> El {
    let count = cx.signal(0_u32);
    let fail_binding = cx.signal(false);
    col()
        .size(rel(1.0))
        .p(sp(4.0))
        .gap(sp(3.0))
        .child(text("Runtime failure validation"))
        .child(
            text(move |r: &mut ReadCx<'_>| {
                assert!(!r.get(fail_binding), "injected binding panic");
                format!("Accepted: {}", r.get(count))
            })
            .test_id("status"),
        )
        .child(button("Accept", KitSize::Md, ButtonVariant::Secondary).test_id("accept").on_press(move |w| w.update(count, |n| *n += 1)))
        .child(
            button("Panic in handler", KitSize::Md, ButtonVariant::Secondary)
                .test_id("fail_event")
                .on_press(|_| panic!("injected event panic")),
        )
        .child(button("Panic in task", KitSize::Md, ButtonVariant::Secondary).test_id("fail_task").on_press(|w| {
            w.spawn("inject fault", |_| async { panic!("injected task panic") });
        }))
        .child(
            button("Panic in binding", KitSize::Md, ButtonVariant::Secondary)
                .test_id("fail_binding")
                .on_press(move |w| w.set(fail_binding, true)),
        )
}

//! Reveal an offscreen editor and replace its row during a focused session.

use octara::prelude::*;
use octara_examples::{native, window_options};
use octara_kit::{ButtonVariant, Size as KitSize, button, text_field};

fn view(cx: &mut BuildCx<'_>) -> El {
    let rows = cx.list::<u32, String>();
    cx.spawn("seed editor rows", move |task| async move {
        task.write(|w| w.reconcile_list(rows, (1..=1000).map(|key| (key, format!("Editable row {key}"))).collect()));
    });
    col()
        .size(rel(1.0))
        .p(sp(4.0))
        .gap(sp(2.0))
        .child(text("Virtual row editors").role(Role::Heading).text_size(FontSizeToken::Xl))
        .child(
            row()
                .gap(sp(2.0))
                .child(
                    button("Reveal row 900", KitSize::Md, ButtonVariant::Secondary)
                        .test_id("virtual-reveal")
                        .preserve_focus_on_press()
                        .on_press(|w| w.reveal_row("virtual-list", 900)),
                )
                .child(
                    button("Remove row 900", KitSize::Md, ButtonVariant::Secondary)
                        .test_id("virtual-remove")
                        .preserve_focus_on_press()
                        .on_press(move |w| {
                            w.remove_row(rows, &900);
                        }),
                ),
        )
        .child(col().test_id("virtual-list").grow(1.0).min_h(0).overflow_y(Overflow::Scroll).child(virtual_each(
            rows,
            VirtualOptions { estimated_row_height: 44.0, ..Default::default() },
            move |b, &key, row| {
                col().shrink(0.0).p(sp(1.0)).child(
                    text_field(b.binding(
                        move |r| r.try_get(row).unwrap_or_default(),
                        move |w, value| w.update_row(rows, &key, |text| *text = value),
                    ))
                    .test_id(format!("virtual-row-{key}"))
                    .label(format!("Row {key}")),
                )
            },
        )))
}

fn main() -> Result<(), octara_host_native::ShellError> {
    native::run(window_options("Octara Virtual Interaction", 960.0, 760.0), view)
}

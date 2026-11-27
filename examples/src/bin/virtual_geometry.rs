//! Variable-height keyed rows retain a reading position across list and width changes.

use octara::prelude::*;
use octara_examples::{native, window_options};
use octara_kit::{ButtonVariant, Size as KitSize, button};

const ROWS: u32 = 10_000;
const WORDS: &str = "Variable-height text wraps with the window width while this row keeps its business key. ";

fn view(cx: &mut BuildCx<'_>) -> El {
    let rows = cx.list::<u32, String>();
    let width = cx.signal(640);
    let next = cx.signal(ROWS + 1);
    cx.spawn("seed geometry rows", move |task| async move {
        task.write(|w| {
            w.reconcile_list(rows, (1..=ROWS).map(|key| (key, format!("Row {key}. {}", WORDS.repeat((1 + key % 7) as usize)))).collect());
        });
    });
    col()
        .size(rel(1.0))
        .p(sp(4.0))
        .gap(sp(2.0))
        .child(text("Virtual geometry").role(Role::Heading).text_size(FontSizeToken::Xl))
        .child(
            row()
                .gap(sp(2.0))
                .child(
                    button("Narrow or widen rows", KitSize::Md, ButtonVariant::Secondary)
                        .test_id("geometry-width")
                        .on_press(move |w| w.update(width, |wide| *wide = if *wide == 640 { 400 } else { 640 })),
                )
                .child(button("Insert before current rows", KitSize::Md, ButtonVariant::Secondary).test_id("geometry-insert").on_press(
                    move |w| {
                        let key = w.get(next);
                        w.set(next, key + 1);
                        w.insert_row(rows, 0, key, format!("New row {key}. {WORDS}"));
                    },
                ))
                .child(
                    button("Reveal row 5,000", KitSize::Md, ButtonVariant::Secondary)
                        .test_id("geometry-reveal")
                        .on_press(|w| w.reveal_row("geometry-list", 5000)),
                ),
        )
        .child(
            col().test_id("geometry-list").grow(1.0).min_h(0).overflow_y(Overflow::Scroll).style_with(move |r, p| p.w(r.get(width))).child(
                virtual_each(rows, VirtualOptions { estimated_row_height: 88.0, ..Default::default() }, |_, &key, row| {
                    text(move |r: &mut ReadCx<'_>| r.try_get(row).unwrap_or_default())
                        .test_id(format!("geometry-row-{key}"))
                        .shrink(0.0)
                        .p(sp(2.0))
                }),
            ),
        )
}

fn main() -> Result<(), octara_host_native::ShellError> {
    native::run(window_options("Octara Virtual Geometry", 960.0, 760.0), view)
}

//! Native input while the Shader Lab updates custom paint at a 120 Hz application cadence.

use super::{BACKGROUND, DEFAULT_PHASE, TEXT, flow, materials, preview};
use octara::prelude::*;
use octara_kit::{ButtonVariant, Size as KitSize, button, slider};
use std::time::Duration;

pub const TITLE: &str = "Octara Shader Lab Input Pressure";
const FRAMES: u32 = 27 * 120;

#[ui]
pub fn view(cx: &mut BuildCx<'_>) -> El {
    let phase = cx.signal(DEFAULT_PHASE);
    let selected = cx.signal(0_usize);
    let grid = cx.signal(true);
    let passes = cx.signal(false);
    let count = cx.signal(0_u64);
    let draft = cx.signal("Z".to_owned());
    let range = cx.signal(0.5);
    let running = cx.signal(false);
    let start = move |w: &mut WriteCx<'_>| {
        if w.get(running) {
            return;
        }
        w.set(running, true);
        w.spawn("input pressure", move |task| async move {
            let origin = task.now();
            for frame in 1..=FRAMES {
                task.sleep_until(origin + Duration::from_secs_f64(f64::from(frame) / 120.0)).await;
                task.write(|w| w.set(phase, 0.2 + f64::from(frame % 240) / 400.0));
            }
            task.write(|w| w.set(running, false));
        });
    };
    row()
        .size(rel(1.0))
        .p(18)
        .gap(14)
        .bg(BACKGROUND)
        .text_color(TEXT)
        .on_command(super::replay::START_COMMAND, start)
        .child(
            col()
                .w(280)
                .gap(10)
                .shrink(0.0)
                .child(button("Start workload", KitSize::Md, ButtonVariant::Secondary).test_id("pressure-start").on_press(start))
                .child(
                    input()
                        .test_id("pressure-editor")
                        .label("Input under pressure")
                        .value(draft)
                        .on_input(move |w, value| {
                            w.set(draft, value.to_owned());
                        })
                        .h(36),
                )
                .child(button("Toggle the grid", KitSize::Md, ButtonVariant::Secondary).test_id("pressure-toggle").on_press(move |w| {
                    w.update(count, |n| *n += 1);
                    w.update(grid, |v| *v = !*v);
                }))
                .child(text(move |r: &mut ReadCx<'_>| r.get(count).to_string()).test_id("pressure-count"))
                .child(slider(cx, "pressure-slider", range.into(), 0.0, 1.0, 0.01).label("Drag position").w(rel(1.0)))
                .child(materials(cx, selected, phase).w(rel(1.0)).h(390)),
        )
        .child(preview(cx, selected, phase, grid, passes, flow::program(false)))
}

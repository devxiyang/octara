//! A stable set of native input targets beside Shader Lab's continuously changing author paint.

use super::{BACKGROUND, DEFAULT_PHASE, TEXT, flow, materials, preview};
use octara::app::App;
use octara::prelude::*;
use octara::serde_json::json;
use octara::task::sleep_until;
use octara_kit::{button, slider};
use std::time::Duration;

pub const TITLE: &str = "Octara Shader Lab Input Pressure";
const FRAMES: u32 = 27 * 120;

#[ui]
pub fn view() -> El {
    let phase = state("pressure-phase", || DEFAULT_PHASE);
    let selected = state("pressure-material", || 0_usize);
    let grid = state("pressure-grid", || true);
    let passes = state("pressure-passes", || false);
    let count = state("pressure-clicks", || 0_u64);
    let draft = state("pressure-draft", || "Z".to_owned());
    let range = state("pressure-range", || 0.5);
    let window = current_window();
    on_command(super::replay::START_COMMAND, move || {
        spawn(async move {
            let origin = App::current().now();
            eprintln!(
                "{}",
                json!({"schema": "octara.shader_lab_input/1", "event": "start", "window": window.number(), "app_clock_ms": origin.as_millis()})
            );
            for frame in 1..=FRAMES {
                sleep_until(origin + Duration::from_secs_f64(f64::from(frame) / 120.0)).await;
                phase.set(0.2 + f64::from(frame % 240) / 400.0);
            }
            eprintln!(
                "{}",
                json!({"schema": "octara.shader_lab_input/1", "event": "finish", "window": window.number(),
                       "app_clock_ms": App::current().now().as_millis()})
            );
        });
    });

    row()
        .size_full()
        .p(18)
        .gap(14)
        .bg(BACKGROUND)
        .text_color(TEXT)
        .child(
            col()
                .w(280)
                .gap(10)
                .flex_shrink_0()
                .child(
                    input().id("pressure-editor").label("Input under pressure").value(draft).on_input(move |value| draft.set(value)).h(36),
                )
                .child(button("Toggle the grid").id("pressure-toggle").on_press(move || {
                    count.update(|n| *n += 1);
                    grid.update(bool::toggle);
                }))
                .child(text(move || count.get().to_string()).id("pressure-count"))
                .child(slider("pressure-slider", range, 0.0, 1.0, 0.01).label("Drag position").w_full())
                .child(materials(selected, phase).w_full().h(390)),
        )
        .child(preview(selected, phase, grid, passes, flow::program(false)))
}

//! Release benchmark for retained scroll, Markdown flings and bulk list switches.

use octara::prelude::*;
use octara::{Input, InteractionEvent, WheelPhase};
use octara_examples::window_options;
use octara_inspect::HeadlessDriver;
use std::time::{Duration, Instant};

const DOCUMENT: &str = "## Render timeline\n\nRead the **layout review** before running the fixture.\n\n- Invalidate geometry\n- Retain paint\n\n```rust\nlet next = rows.iter().filter(|row| row.is_live());\n```\n\nThe next paragraph contains inline math $x^2+y^2$ and a [reference](https://example.com).";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let hz: u64 = std::env::args().nth(1).as_deref().unwrap_or("120").parse()?;
    assert!(hz > 0);
    for mode in ["micro-scroll", "markdown-fling", "bulk-switch"] {
        let mut driver = HeadlessDriver::default();
        let selected = driver.app_mut().build(|b| b.signal(false))?;
        let window = driver.app_mut().open_window(window_options("Scroll benchmark", 960.0, 680.0), move |b| match mode {
            "micro-scroll" => scrollport().children((0..160).map(|row| text(format!("Retained item {row}")).h(28).shrink(0.0).px(8))),
            "markdown-fling" => {
                let rows = b.list_from((0..320).map(|row| (row, format!("# Row {row}\n\n{DOCUMENT}"))));
                scrollport().child(virtual_each(rows, VirtualOptions { estimated_row_height: 190.0, ..Default::default() }, |b, _, row| {
                    let document = b.sample(row);
                    octara_markdown::markdown(b, document).p(8)
                }))
            }
            _ => scrollport().child(keyed(
                move |r| r.get(selected),
                |_, right| {
                    col().shrink(0.0).children((0..160).map(|row| {
                        col()
                            .shrink(0.0)
                            .p(4)
                            .child((text(if *right { "Second dataset" } else { "First dataset" }), text(format!("Row {row}"))))
                    }))
                },
            )),
        })?;
        let count: usize = if mode == "bulk-switch" { 100 } else { 360 };
        let mut samples = Vec::with_capacity(count);
        let mut handled = 0;
        for index in 0..(count + 16) {
            driver.advance_to(Duration::from_nanos((index as u64 + 1) * (1_000_000_000 / hz)))?;
            let direction = if index / 12 % 2 == 0 { 1.0 } else { -1.0 };
            let start = Instant::now();
            if mode == "bulk-switch" {
                driver.app_mut().update(|w| w.set(selected, index % 2 == 0))?;
            } else {
                let distance = if mode == "markdown-fling" { [220.0, 870.0, 360.0, 1130.0][index % 4] } else { 4.0 };
                let event = InteractionEvent::Wheel {
                    pos: Point::new(100.0, 160.0),
                    delta: Vec2::new(0.0, distance * direction),
                    phase: WheelPhase::Discrete,
                    mods: Modifiers::empty(),
                };
                let receipt = driver.dispatch(window, Input::Interaction(event))?;
                handled += usize::from(receipt.handled);
            }
            if index >= 16 {
                samples.push(start.elapsed().as_secs_f64() * 1e3);
            }
        }
        assert!(mode == "bulk-switch" || handled >= count * 9 / 10, "{mode} handled {handled} inputs");
        samples.sort_by(f64::total_cmp);
        let quantile = |p: usize| samples[(p * count).div_ceil(100) - 1];
        println!("{mode},{hz},{count},{:.6},{:.6},{:.6}", quantile(50), quantile(95), quantile(99));
    }
    Ok(())
}

fn scrollport() -> El {
    col().size(rel(1.0)).min_h(0).overflow_y(Overflow::Scroll)
}

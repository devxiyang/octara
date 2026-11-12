//! A canvas chart picked by dragging, and icons.

use super::section;
use octara::prelude::*;
use octara_icons as icons;

/// Monthly figures for the chart.
const SALES: [f32; 12] = [42.0, 51.0, 47.0, 63.0, 58.0, 72.0, 69.0, 81.0, 77.0, 88.0, 84.0, 96.0];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// A bar chart drawn on a canvas: dragging across it picks a month.
#[component]
fn chart(picked: Signal<usize>) -> El {
    let pick = move |x: f32, w: f32| picked.set(((x / w * 12.0).floor().max(0.0) as usize).min(11));
    let width = std::rc::Rc::new(std::cell::Cell::new(1.0f32));
    let seen = width.clone();
    canvas(move |c| {
        let (w, full) = c.size();
        seen.set(w);
        // Room for the month labels under the plot.
        let h = full - 18.0;
        // Headroom above the tallest bar for its value label.
        let max = 115.0;
        let slot = w / SALES.len() as f32;
        // The trend under the bars: an area fading down, and its line.
        let pts: Vec<_> = SALES.iter().enumerate().map(|(i, v)| point(slot * (i as f32 + 0.5), h - v / max * h)).collect();
        let line = Path::new().add_polygon(&pts, false);
        let area = line.clone().line_to((pts[11].x, h)).line_to((pts[0].x, h)).close();
        c.fill(
            &area,
            linear_gradient(
                180.0,
                linear_color_stop(rgb(0x6366f1).with_alpha(0.25), 0.0),
                linear_color_stop(rgb(0x6366f1).with_alpha(0.0), 1.0),
            ),
        );
        for (i, v) in SALES.iter().enumerate() {
            let bh = v / max * h;
            let bar = Path::rounded_rect(Rect::new(slot * i as f32 + slot * 0.2, h - bh, slot * 0.6, bh), 3.0);
            c.fill(&bar, if i == picked.get() { Token::Accent } else { Token::Border });
        }
        c.stroke(&line, &Stroke::new(2.0).cap(LineCap::Round).join(LineJoin::Round), rgb(0x6366f1));
        for (i, m) in MONTHS.iter().enumerate() {
            let label = c.text(*m, (slot * (i as f32 + 0.5), full)).align(Align::Center).valign(Align::End).size(10.0);
            if i == picked.get() {
                label.color(Token::Text).weight(600);
            }
        }
        let p = picked.get();
        let top = h - SALES[p] / max * h;
        c.text(format!("{}", SALES[p]), (slot * (p as f32 + 0.5), (top - 4.0).max(12.0)))
            .align(Align::Center)
            .valign(Align::End)
            .size(12.0)
            .weight(600)
            .color(Token::Accent);
    })
    .text_color(Token::TextMuted)
    .id("chart")
    .label(move || format!("Monthly sales, {} selected: {}", MONTHS[picked.get()], SALES[picked.get()]))
    .w(320)
    .h(160)
    .on_drag(move |d| pick(d.x, width.get()))
}

#[component]
pub fn card() -> El {
    let picked = state("picked", || 11usize);
    section("Canvas and icons")
        .child(chart(picked))
        .child(
            text(move || format!("{}: {} — drag across the chart", MONTHS[picked.get()], SALES[picked.get()]))
                .text_xs()
                .text_color(Token::TextMuted),
        )
        .child(
            row()
                .gap_3()
                .items_center()
                .children([icons::HOUSE, icons::SEARCH, icons::BELL, icons::SETTINGS, icons::HEART, icons::STAR].map(|i| icon(i).size_5()))
                .child(
                    row()
                        .gap_1()
                        .items_center()
                        .text_color(Token::Accent)
                        .child(icon(icons::SPARKLES))
                        .child(text("Lucide, in the text color").text_sm()),
                ),
        )
}

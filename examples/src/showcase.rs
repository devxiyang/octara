//! A tour of what Octara draws and does: shadows, gradients, images, a canvas chart, icons, group
//! opacity, cut text, text editing, a select, focus, timers, an async resource, and motion.

use octara::prelude::*;
use octara::task::now;
use octara_icons as icons;
use octara_kit::{button, ghost_button, primary_button, select, switch, text_field};
use std::time::Duration;

/// A generated landscape (no image files needed).
fn landscape() -> Image {
    let (w, h) = (240u32, 150u32);
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = (x as f32 - 170.0, y as f32 - 50.0);
            let sun = (dx * dx + dy * dy).sqrt() < 24.0;
            let hill = (y as f32) > 105.0 + 14.0 * ((x as f32) / 38.0).sin();
            let c = if sun {
                [255, 204, 77]
            } else if hill {
                [52, 132 - (y.saturating_sub(100).min(60) as u8), 82]
            } else {
                [64 + (y / 2) as u8, 120 + (y / 3) as u8, 235]
            };
            px.extend([c[0], c[1], c[2], 255]);
        }
    }
    Image::rgba(w, h, px)
}

fn section(title: &'static str) -> El {
    // Lifts while the mouse is anywhere over it (the card has no handlers of its own). Boxes do
    // not shrink by default; a card does, so on a phone it narrows and its rows wrap.
    col()
        .flex_shrink()
        .hover(|p| p.shadow_lg())
        .gap_3()
        .p_5()
        .rounded_xl()
        .bg(Token::Background)
        .border()
        .border_color(Token::Border)
        .shadow_sm()
        .child(text(title).text_sm().font_semibold().text_color(Token::TextMuted))
}

/// Monthly figures for the chart.
const SALES: [f32; 12] = [42.0, 51.0, 47.0, 63.0, 58.0, 72.0, 69.0, 81.0, 77.0, 88.0, 84.0, 96.0];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// A bar chart drawn on a canvas: dragging across it picks a month.
fn chart(picked: Signal<usize>) -> El {
    let pick = move |x: f32, w: f32| picked.set(((x / w * 12.0).floor().max(0.0) as usize).min(11));
    let width = std::rc::Rc::new(std::cell::Cell::new(1.0f32));
    let seen = width.clone();
    canvas(move |c| {
        let (w, h) = c.size();
        seen.set(w);
        let max = 100.0;
        let slot = w / SALES.len() as f32;
        // The trend under the bars: an area fading down, and its line.
        let pts: Vec<_> = SALES.iter().enumerate().map(|(i, v)| point(slot * (i as f32 + 0.5), h - v / max * h)).collect();
        let line = Path::new().add_polygon(&pts, false);
        let area = line.clone().line_to((pts[11].x, h)).line_to((pts[0].x, h)).close();
        c.fill(&area, linear_gradient(180.0, linear_color_stop(rgb(0x6366f1).with_alpha(0.25), 0.0), linear_color_stop(rgb(0x6366f1).with_alpha(0.0), 1.0)));
        for (i, v) in SALES.iter().enumerate() {
            let bh = v / max * h;
            let bar = Path::rounded_rect(Rect::new(slot * i as f32 + slot * 0.2, h - bh, slot * 0.6, bh), 3.0);
            c.fill(&bar, if i == picked.get() { Token::Accent } else { Token::Border });
        }
        c.stroke(&line, &Stroke::new(2.0).cap(LineCap::Round).join(LineJoin::Round), rgb(0x6366f1));
    })
    .id("chart")
    .label(move || format!("Monthly sales, {} selected: {}", MONTHS[picked.get()], SALES[picked.get()]))
    .w(320)
    .h(140)
    .on_drag(move |d| pick(d.x, width.get()))
}

pub fn showcase() -> View {
    let picture = landscape();
    let notes = list("notes", || vec![(1u32, "Try Tab, arrows and Escape".to_owned())], |n: &(u32, String)| n.0);
    let next = state("next", || 2u32);
    let draft = state("draft", String::new);
    let theme = state("theme", || "light");
    let faded = state("faded", || true);
    let query = state("query", || "octara".to_string());
    // A pretend search: answers 600 ms later on the app's clock.
    let results = resource(
        move || query.get(),
        |q: String| async move {
            sleep(Duration::from_millis(600)).await;
            if q.trim().is_empty() {
                Err("type something".to_string())
            } else {
                Ok(format!("{} results for “{}”", q.len() * 7, q.trim()))
            }
        },
    );
    let clock = state("clock", || 0u64);
    interval(Duration::from_secs(1), move || clock.set(now().as_secs()));
    let add = move || {
        let t = draft.get().trim().to_owned();
        if t.is_empty() {
            return;
        }
        let id = next.get();
        next.set(id + 1);
        notes.push((id, t));
        draft.set(String::new());
    };

    watch(move || theme.get(), |t, _| set_theme(if *t == "dark" { Theme::dark() } else { Theme::light() }));
    let hero = col()
        .gap_1()
        .p_6()
        .rounded_2xl()
        .shadow_xl()
        .bg(linear_gradient(135.0, linear_color_stop(rgb(0x6366f1), 0.0), linear_color_stop(rgb(0xec4899), 1.0)))
        .child(text("Octara").text_3xl().font_semibold().text_color(rgb(0xffffff)))
        .child(text("Shadows, gradients, images, layers, editing and async — drawn by the GPU renderer.").text_color(rgb(0xfce7f3)))
        .child(text(move || format!("Running for {} s on the app's clock", clock.get())).text_sm().text_color(rgb(0xe0e7ff)))
        .child(row().pt_2().child(select("theme", theme, [("light", "Light theme"), ("dark", "Dark theme")])));

    let images = section("Images and fit")
        .child(
            row()
                .gap_4()
                .flex_wrap()
                .items_end()
                .child(el().w(200).img(&picture).rounded_lg().shadow_md())
                .child(el().size_24().img(&picture).object_fit(ObjectFit::Cover).rounded_full().shadow_md())
                .child(el().w(120).h(96).img(&picture).bg(Token::Surface).rounded_md().object_fit(ObjectFit::Contain)),
        )
        .child(text("200 wide (height from the image) · cover in a circle · contain").text_xs().text_color(Token::TextMuted));

    let picked = state("picked", || 11usize);
    let drawing = section("Canvas and icons")
        .child(chart(picked))
        .child(text(move || format!("{}: {} — drag across the chart", MONTHS[picked.get()], SALES[picked.get()])).text_xs().text_color(Token::TextMuted))
        .child(
            row()
                .gap_3()
                .items_center()
                .children([icons::HOUSE, icons::SEARCH, icons::BELL, icons::SETTINGS, icons::HEART, icons::STAR].map(|i| icon(i).size_5()))
                .child(row().gap_1().items_center().text_color(Token::Accent).child(icon(icons::SPARKLES)).child(text("Lucide, in the text color").text_sm())),
        );

    let layers = section("Group opacity")
        .child(
            row().gap_6().items_center().child(
                el().w(150)
                    .h(90)
                    .opacity(move || if faded.get() { 0.5 } else { 1.0 })
                    .child(el().absolute().top_0().left_0().size_16().rounded_lg().bg(rgb(0xef4444)))
                    .child(el().absolute().top(26.0).left(40.0).size_16().rounded_full().bg(rgb(0x3b82f6)))
                    .child(text("Layer").absolute().bottom_0().right_0().font_semibold()),
            ),
        )
        .child(row().gap_2().items_center().child(switch(faded).label("Half opacity")).child(text("Half opacity").text_sm()));

    let editing = section("Text editing")
        .child(
            row()
                .gap_2()
                .child(text_field(draft).id("note").label("New note").w(260).flex_shrink().on_submit(add))
                .child(primary_button("Add").id("add").on_press(add)),
        )
        .child(col().gap_1().child(each(
            move || notes.source(),
            move |id, n| {
                let id = *id;
                // The Remove button shows while the row is hovered.
                row()
                    .group("note")
                    .gap_2()
                    .items_center()
                    .child(text(move || n.with(|n| n.1.clone())).flex_shrink().truncate().w(300))
                    .child(ghost_button("Remove").opacity(0.0).group_hover("note", |p| p.opacity(1.0)).on_press(move || notes.remove(&id)))
                    .into()
            },
        )))
        .child(text("Select text with the mouse, double-click a word, ⌘Z to undo, ⌘C / ⌘V.").text_xs().text_color(Token::TextMuted));

    let search = section("Async resource")
        .child(text_field(query).id("query").label("Search").w(260).flex_shrink())
        .child(text(move || match (results.loading(), results.value(), results.error()) {
            (true, Some(v), _) => format!("{v} (updating…)"),
            (true, None, _) => "Searching…".to_string(),
            (false, _, Some(e)) => e,
            (false, v, None) => v.unwrap_or_default(),
        }))
        .child(row().gap_2().child(button("Search again").on_press(move || results.refetch())));

    let cut = section("Cut text")
        .child(text("A sentence that is far too long to fit in this narrow box, cut with an ellipsis").w(240).truncate())
        .child(
            text("Two lines at most: the rest of this paragraph is hidden behind an ellipsis at the end of the second line.")
                .w(240)
                .line_clamp(2),
        );

    col()
        .size_full()
        .bg(Token::Surface)
        .overflow_y_scroll()
        .child(
            col()
                // Everything here moves: theme and state colors, opacity, rows entering and
                // leaving, layout making room.
                .motion(Spring::SNAP)
                .p_8()
                .gap_6()
                .max_w(880)
                .child(hero)
                .child(row().gap_6().flex_wrap().items_start().child(images).child(layers))
                .child(row().gap_6().flex_wrap().items_start().child(drawing))
                .child(row().gap_6().flex_wrap().items_start().child(editing).child(col().gap_6().child(search).child(cut))),
        )
        .into()
}

//! A real-window fixture for keyed virtual-list geometry and reading anchors.
//! Start with `OCTARA_INSPECT=/tmp/octara-geometry.sock cargo run -p octara-examples --bin virtual_geometry`.
//! Inspector `invoke` commands use the same handlers as the visible buttons.

use octara::prelude::*;
use octara_kit::button;

const ROWS: u32 = 10_000;
const ESTIMATE: f32 = 88.0;
const WORDS: &str = "Variable-height text wraps with the window width while this row keeps its business key. ";

#[derive(Clone)]
struct GeometryRow {
    text: String,
}

#[derive(Clone, Copy)]
struct Model {
    rows: Signal<Vec<(u32, GeometryRow)>>,
    next: Signal<u32>,
    font: Signal<f32>,
    report: Signal<String>,
    action: Signal<String>,
}

#[derive(Clone, Copy)]
struct Anchor {
    key: u32,
    offset: f32,
}

fn seed_row(key: u32) -> (u32, GeometryRow) {
    (key, GeometryRow { text: format!("Row {key}. {}", WORDS.repeat(1 + (key % 7) as usize)) })
}

fn mounted_rows(window: &Window) -> Vec<(u32, f32, f32)> {
    let kernel = window.kernel();
    kernel
        .child_ids("geometry-list")
        .into_iter()
        .filter_map(|id| {
            let key = id.strip_prefix("geometry-row-")?.parse().ok()?;
            let (_, y, _, height) = kernel.rect(kernel.find(&id)?);
            Some((key, y, height))
        })
        .collect()
}

fn anchor(window: &Window) -> Option<Anchor> {
    let top = {
        let kernel = window.kernel();
        kernel.content_origin_y(kernel.find("geometry-list")?)
    };
    mounted_rows(window)
        .into_iter()
        .find(|(_, y, height)| *y <= top + 0.1 && y + height > top)
        .map(|(key, y, _)| Anchor { key, offset: y - top })
}

fn update_report(window: &Window, model: Model) {
    let reading = anchor(window).map(|anchor| format!("{} @ {:.2}", anchor.key, anchor.offset)).unwrap_or_else(|| "none".into());
    let resident = mounted_rows(window).len();
    let kernel = window.kernel();
    let list = kernel.find("geometry-list").expect("the fixture list is mounted");
    let scroll = kernel.scroll_offset(list).1;
    let extent = kernel.scroll_size(list).1;
    let viewport = kernel.client_size(list).1;
    let following = kernel.is_following_tail(list);
    let nodes = kernel.live_nodes();
    drop(kernel);
    model.report.set(format!(
        "Rows: {}; resident: {resident}; nodes: {nodes}; anchor: {reading}; scroll: {scroll:.2}; extent: {extent:.2}; viewport: {viewport:.2}; following: {following}; font: {:.0}",
        model.rows.with(Vec::len),
        model.font.get(),
    ));
}

fn command(name: &'static str, model: Model, action: impl Fn(&Window, Model) + 'static) {
    let id = current_window();
    on_command(name, move || {
        if let Some(window) = App::current().window(id) {
            action(&window, model);
            // Mutate first, then let the outer event settle. Nested dispatch would settle a
            // partially edited snapshot and can run the current command a second time.
            invoke_after_update("geometry.report");
        }
    });
}

#[ui]
fn action(label: &'static str, id: &'static str, command: &'static str) -> View {
    button(label).id(id).preserve_focus_on_press(true).on_press(move || invoke_after_update(command)).into()
}

#[ui]
fn geometry_row(key: u32, row: ReadSignal<GeometryRow>) -> View {
    text(move || row.with(|row| row.text.clone()))
        .id(format!("geometry-row-{key}"))
        .w_full()
        .min_w_0()
        .flex_shrink_0()
        .p_3()
        .bg(if key.is_multiple_of(2) { Token::Surface } else { Token::SurfaceHover })
        .into()
}

#[ui]
fn virtual_geometry() -> View {
    let model = Model {
        rows: state("rows", || (0..ROWS).map(seed_row).collect()),
        next: state("next", || ROWS),
        font: state("font", || 14.0),
        report: state("report", || "Use Middle to leave the tail, then mutate rows or resize the window.".into()),
        action: state("action", || "Ready".into()),
    };
    let id = current_window();
    on_command("geometry.report", move || {
        if let Some(window) = App::current().window(id) {
            update_report(&window, model);
        }
    });
    command("geometry.middle", model, |window, model| {
        let offset = {
            let kernel = window.kernel();
            let list = kernel.find("geometry-list").unwrap();
            (kernel.scroll_size(list).1 - kernel.client_size(list).1).max(0.0) / 2.0
        };
        scroll_to("geometry-list", 0.0, offset);
        model.action.set("Scrolled to the middle".into());
    });
    command("geometry.prepend", model, |_, model| {
        let first = model.next.get();
        model.next.set(first + 100);
        model.rows.update(|rows| drop(rows.splice(0..0, (first..first + 100).map(seed_row))));
        model.action.set(format!("Prepended keys {first} through {}", first + 99));
    });
    command("geometry.remove", model, |window, model| {
        if let Some(anchor) = anchor(window) {
            model.rows.update(|rows| {
                let index = rows.iter().position(|(key, _)| *key == anchor.key).unwrap();
                let start = index.saturating_sub(32);
                rows.drain(start..index);
                model.action.set(format!("Removed {} rows before key {}", index - start, anchor.key));
            });
        }
    });
    command("geometry.move", model, |window, model| {
        if let Some(anchor) = anchor(window) {
            model.rows.update(|rows| {
                let index = rows.iter().position(|(key, _)| *key == anchor.key).unwrap();
                if index > 0 {
                    let moved = rows.remove(0);
                    model.action.set(format!("Moved key {} from the start to after key {}", moved.0, anchor.key));
                    rows.insert(index, moved);
                }
            });
        }
    });
    command("geometry.grow", model, |window, model| {
        let top = {
            let kernel = window.kernel();
            kernel.content_origin_y(kernel.find("geometry-list").unwrap())
        };
        // Grow a measured overscan row above the reader, or the anchor at the very start.
        let key = mounted_rows(window)
            .into_iter()
            .rev()
            .find(|(_, y, height)| y + height <= top + 0.1)
            .map(|(key, _, _)| key)
            .or_else(|| anchor(window).map(|anchor| anchor.key));
        if let Some(key) = key {
            model.rows.update(|rows| {
                let (_, row) = rows.iter_mut().find(|(id, _)| *id == key).unwrap();
                row.text.push_str(&WORDS.repeat(8));
            });
            model.action.set(format!("Grew measured key {key}"));
        }
    });
    command("geometry.font", model, |_, model| {
        model.font.update(|font| *font = if *font == 14.0 { 20.0 } else { 14.0 });
        model.action.set(format!("Inherited font size is {:.0}", model.font.get()));
    });
    command("geometry.tail", model, |_, model| {
        scroll_to_end("geometry-list");
        model.action.set("Following the tail".into());
    });

    col()
        .id("geometry-root")
        .size_full()
        .p_3()
        .gap_2()
        .bg(Token::Surface)
        .text_color(Token::Text)
        .child(text("Virtual list geometry").text_xl().font_semibold())
        .child(text("10,000 keyed rows with wrapping text. Read the anchor, then mutate, resize, or change the font.").text_sm())
        .child(
            row()
                .flex_wrap()
                .gap_2()
                .child(action("Middle", "geometry-middle", "geometry.middle"))
                .child(action("Prepend 100", "geometry-prepend", "geometry.prepend"))
                .child(action("Remove 32", "geometry-remove", "geometry.remove"))
                .child(action("Move first", "geometry-move", "geometry.move"))
                .child(action("Grow above", "geometry-grow", "geometry.grow"))
                .child(action("Font", "geometry-font", "geometry.font"))
                .child(action("Tail", "geometry-tail", "geometry.tail"))
                .child(action("Report", "geometry-report-action", "geometry.report")),
        )
        .child(text(model.report).id("geometry-report").text_sm().h(88.0).flex_shrink_0())
        .child(text(model.action).id("geometry-action").text_sm().h(36.0).flex_shrink_0())
        .child(
            col()
                .id("geometry-list")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .follow_tail()
                .style_with(move |style| style.text_size(model.font.get()))
                .child(virtual_each(move || model.rows.get(), |_| ESTIMATE, |key, row| geometry_row(*key, row))),
        )
        .into()
}

fn main() {
    octara_shell::run_with(
        octara_shell::Options { title: "Octara Virtual Geometry".into(), size: (960.0, 760.0), ..Default::default() },
        virtual_geometry,
    );
}

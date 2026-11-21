//! A real-window fixture for editor and pointer residency in a sparse virtual list.
//! Inspector commands inject framework events; `virtual.compose` does not exercise an OS IME.
//! Set `OCTARA_VIRTUAL_EXIT=1` to show the deletion and reinsertion of row zero.

use octara::app::{Event, InputEvent, Phase, PointerEvent};
use octara::prelude::*;
use octara::reactive::untrack;
use octara_kit::button;

const ROW_HEIGHT: f32 = 64.0;
const ROWS: u32 = 10_000;
const FAR: f32 = 5_000.0 * ROW_HEIGHT;
const SEED: &str = "A😀bcZ";

#[derive(Clone, Copy)]
struct Model {
    items: StateList<u32, u32>,
    shown: Signal<bool>,
    report: Signal<String>,
    drags: Signal<Vec<String>>,
    drag_at: Signal<(f32, f32)>,
    value: Signal<String>,
    edits: Signal<u32>,
    exit_reinserted: Option<Signal<bool>>,
}

fn update_report(window: &Window, model: Model) {
    let kernel = window.kernel();
    let rows = kernel.child_ids("virtual-list").iter().filter(|id| id.starts_with("virtual-row-")).count();
    let offset = kernel.find("virtual-list").map(|list| kernel.scroll_offset(list).1).unwrap_or(0.0);
    let nodes = kernel.live_nodes();
    drop(kernel);
    let field = window.text_field();
    let focus = window.inspect().focus.unwrap_or_else(|| "none".into());
    model.report.set(format!(
        "Resident rows: {rows}; nodes: {nodes}; scroll: {offset:.0}; focus: {focus}; held: {}; editor: {}",
        window.held_pointer().is_some(),
        field
            .map(|field| format!(
                "selection {:?}, reversed {}, composition {:?}, marked selection {:?}",
                field.selection, field.reversed, field.composition, field.composition_selection
            ))
            .unwrap_or_else(|| "none".into())
    ));
}

fn command(name: &str, model: Model, action: impl Fn(&mut Window, Model) + 'static) {
    let id = current_window();
    on_command(name, move || {
        if let Some(mut window) = App::current().window(id) {
            action(&mut window, model);
            update_report(&window, model);
        }
    });
}

fn scroll(window: &mut Window, offset: f32) {
    window.dispatch(Event::ScrollTo("virtual-list".into(), 0.0, offset)).unwrap();
}

#[ui]
fn action(label: &'static str, id: &'static str, name: &'static str) -> View {
    button(label).id(id).preserve_focus_on_press(true).on_press(move || invoke(name).unwrap()).into()
}

#[ui]
fn virtual_row(key: u32, model: Model) -> El {
    let draft = state("draft", || SEED.to_owned());
    let row = row()
        .id(format!("virtual-row-{key}"))
        .h(ROW_HEIGHT)
        .flex_shrink_0()
        .items_center()
        .gap_3()
        .px_3()
        .child(text(format!("Row {key}")).w(100.0))
        .child(
            input()
                .id(format!("virtual-editor-{key}"))
                .label(format!("Virtual editor {key}"))
                .w(300.0)
                .h(40.0)
                .px_3()
                .bg(Token::Surface)
                .border()
                .border_color(Token::Border)
                .rounded_md()
                .value(draft)
                .behavior(|b| b.on_input(move |value| draft.set(value)))
                .on_input(move |_| {
                    if key == 0 {
                        model.value.set(draft.get());
                        model.edits.update(|count| *count += 1);
                    }
                }),
        )
        .child(
            el().id(format!("virtual-drag-{key}"))
                .label(format!("Drag row {key}"))
                .w(140.0)
                .h(44.0)
                .items_center()
                .justify_center()
                .rounded_md()
                .bg(Token::Accent)
                .text_color(Token::AccentText)
                .preserve_focus_on_press(true)
                .child("Drag this row")
                .on_drag(move |drag| model.drags.update(|events| events.push(format!("{key}:{:?}", drag.phase)))),
        );
    if let Some(reinserted) = model.exit_reinserted
        && key == 0
    {
        let color = if untrack(|| reinserted.get()) { rgb(0x285fe6) } else { rgb(0xe12d42) };
        row.opacity_motion(Curve::Fade(2.0)).child(el().id("virtual-exit-swatch").absolute().right(12).top(0).w(16).h(ROW_HEIGHT).bg(color))
    } else {
        row
    }
}

#[ui]
fn virtual_list(model: Model) -> View {
    col()
        .id("virtual-list")
        .flex_1()
        .overflow_y_scroll()
        .bg(Token::SurfaceHover)
        .child(virtual_each(move || model.items.read_only(), |_| ROW_HEIGHT, move |key, _| virtual_row(*key, model)))
        .into()
}

#[ui]
fn virtual_interaction(exit_probe: bool) -> View {
    let model = Model {
        items: state_list("items", || (0..ROWS).collect(), |key| *key),
        shown: state("shown", || true),
        report: state("report", || "Use an action, then Report to read residency and editing state.".to_owned()),
        drags: state("drags", Vec::new),
        drag_at: state("drag_at", || (0.0, 0.0)),
        value: state("value", || SEED.to_owned()),
        edits: state("edits", || 0),
        exit_reinserted: exit_probe.then(|| state("exit-reinserted", || false)),
    };
    command("virtual.report", model, |_, _| {});
    command("virtual.home", model, |window, _| scroll(window, 0.0));
    command("virtual.far", model, |window, _| scroll(window, FAR));
    command("virtual.focus", model, |window, _| {
        scroll(window, 0.0);
        window.dispatch(Event::Focus("virtual-editor-0".into())).unwrap();
    });
    command("virtual.compose", model, |window, _| {
        scroll(window, 0.0);
        window.dispatch(Event::Focus("virtual-editor-0".into())).unwrap();
        window.set_selection(7, 5);
        window.event(InputEvent::Preedit { text: "nihao".into(), selection: Some((1, 4)) });
    });
    command("virtual.blur", model, |window, _| {
        window.dispatch(Event::Blur).unwrap();
    });
    command("virtual.drag.start", model, |window, model| {
        scroll(window, 0.0);
        let (x, y) = {
            let kernel = window.kernel();
            let (x, y, width, height) = kernel.rect(kernel.find("virtual-drag-1").unwrap());
            (x + width / 2.0, y + height / 2.0)
        };
        window.pointer_down(x, y);
        window.pointer_move(x + 12.0, y);
        model.drag_at.set((x + 12.0, y));
    });
    command("virtual.drag.move", model, |window, model| {
        let (x, y) = model.drag_at.get();
        window.pointer_move(x + 24.0, y);
        model.drag_at.set((x + 24.0, y));
    });
    command("virtual.drag.end", model, |window, model| {
        let (x, y) = model.drag_at.get();
        window.pointer_up(x, y);
    });
    command("virtual.drag.cancel", model, |window, model| {
        let (x, y) = model.drag_at.get();
        window.event(InputEvent::Pointer(PointerEvent::mouse(Phase::Cancel, x, y)));
    });
    command("virtual.remove", model, |_, model| {
        model.items.remove(&0);
        model.items.remove(&1);
    });
    command("virtual.reset", model, |_, model| {
        model.items.remove(&0);
        model.items.remove(&1);
        if let Some(reinserted) = model.exit_reinserted {
            reinserted.set(true);
        }
        model.items.insert(0, 1);
        model.items.insert(0, 0);
        model.shown.set(true);
        model.value.set(SEED.to_owned());
        model.edits.set(0);
        model.drags.set(Vec::new());
    });
    command("virtual.branch.hide", model, |_, model| model.shown.set(false));
    command("virtual.branch.show", model, |_, model| model.shown.set(true));

    col()
        .id("virtual-root")
        .size_full()
        .p_4()
        .gap_3()
        .bg(Token::Surface)
        .child(text("Virtual list interaction residency").text_xl().font_semibold())
        .child(text("10,000 rows. Compose injects framework preedit; use your keyboard to test the OS IME.").text_sm())
        .child(
            row()
                .gap_2()
                .child(action("Compose", "virtual-compose", "virtual.compose"))
                .child(action("Scroll far", "virtual-scroll-far", "virtual.far"))
                .child(action("Home", "virtual-scroll-home", "virtual.home"))
                .child(action("Blur", "virtual-blur", "virtual.blur"))
                .child(action("Report", "virtual-report-action", "virtual.report")),
        )
        .child(
            row()
                .gap_2()
                .child(action("Drag start", "virtual-drag-start", "virtual.drag.start"))
                .child(action("Drag move", "virtual-drag-move", "virtual.drag.move"))
                .child(action("Drag end", "virtual-drag-end", "virtual.drag.end"))
                .child(action("Cancel", "virtual-drag-cancel", "virtual.drag.cancel"))
                .child(action("Remove active rows", "virtual-remove", "virtual.remove"))
                .child(action("Reset", "virtual-reset", "virtual.reset")),
        )
        .child(text(model.report).id("virtual-report").text_sm().w_full())
        .child(text(move || format!("Drag events: {}", model.drags.with(|events| events.join(", ")))).id("virtual-drag-report").text_sm())
        .child(text(move || format!("Row 0: {:?}; edits: {}", model.value.get(), model.edits.get())).id("virtual-model-report").text_sm())
        .child(show(move || model.shown.get(), move || virtual_list(model)))
        .into()
}

fn main() {
    let exit_probe = std::env::var_os("OCTARA_VIRTUAL_EXIT").is_some_and(|value| value == "1");
    octara_shell::run_with(
        octara_shell::Options { window: WindowOptions::new("Octara Virtual Interaction").size(960.0, 760.0), ..Default::default() },
        move || virtual_interaction(exit_probe),
    );
}

//! A real-window fixture for retained targets, replacement and window generations.

use octara::prelude::*;
use octara_kit::button;

#[derive(Clone, Copy)]
struct Targets {
    hits: Signal<u32>,
    generation: Signal<u32>,
    updates: Signal<u32>,
    child: Signal<Option<WindowId>>,
    child_openings: Signal<u32>,
    error: Signal<String>,
}

#[ui]
fn target(model: Targets) -> View {
    button(move || format!("Hit target — value {}", model.updates.get()))
        .id("target-action")
        .self_start()
        .min_h(44.0)
        .style_with(move |s| s.ml(if model.updates.get().is_multiple_of(2) { 0.0 } else { 120.0 }))
        .on_press(move || model.hits.update(|n| *n += 1))
        .into()
}

#[ui]
fn stage(model: Targets) -> View {
    col()
        .id("target-stage")
        .h(100.0)
        .p_4()
        .border()
        .border_color(Token::Border)
        .rounded_md()
        .child(show_else(move || model.generation.get().is_multiple_of(2), move || target(model), move || target(model)))
        .into()
}

#[ui]
fn counters(model: Targets) -> View {
    row()
        .gap_6()
        .child(text(move || format!("Total hits: {}", model.hits.get())).id("targets-hits").font_semibold())
        .child(text(move || format!("Generation: {}", model.generation.get())).id("targets-generation"))
        .child(text(move || format!("Value updates: {}", model.updates.get())).id("targets-updates"))
        .into()
}

#[ui]
fn child(model: Targets, main: WindowId) -> View {
    let id = current_window();
    // A reload runs cleanup too; the initial watch publishes this same live window again.
    watch(|| (), move |_, _| model.child.set(Some(id)));
    on_cleanup(move || {
        if windows().contains(&main) && untrack(|| model.child.get()) == Some(id) {
            model.child.set(None);
        }
    });

    col()
        .id("targets-child-root")
        .size_full()
        .p_6()
        .gap_4()
        .bg(Token::Surface)
        .child(text("Target child window").text_xl().font_semibold())
        .child(counters(model))
        .child(stage(model))
        .child(text("This window shares the main window's hit counter.").text_sm())
        .child(button("Close target window").id("targets-child-close").self_start().min_h(44.0).on_press(move || close_window(id)))
        .into()
}

#[ui]
fn targets() -> View {
    let model = Targets {
        hits: state("hits", || 0),
        generation: state("generation", || 0),
        updates: state("updates", || 0),
        child: state("child", || None),
        child_openings: state("child_openings", || 0),
        error: state("error", String::new),
    };
    let main = current_window();

    col()
        .id("targets-root")
        .size_full()
        .p_6()
        .gap_4()
        .bg(Token::Surface)
        .child(text("Target validation").text_xl().font_semibold())
        .child(text("Inspect target-action, then change its value, replace it, or reopen its window.").text_sm())
        .child(counters(model))
        .child(stage(model))
        .child(
            row()
                .gap_3()
                .child(button("Replace target").id("targets-replace").min_h(44.0).on_press(move || model.generation.update(|n| *n += 1)))
                .child(
                    button("Update value and move").id("targets-update").min_h(44.0).on_press(move || model.updates.update(|n| *n += 1)),
                ),
        )
        .child(text("Replace creates a new node. Update keeps the node and changes its text and position.").text_sm())
        .child(
            row()
                .gap_3()
                .child(button("Open target window").id("targets-open").min_h(44.0).on_press(move || {
                    if model.child.get().is_some_and(|id| windows().contains(&id)) {
                        return;
                    }
                    let options = WindowOptions::new("Octara Target Child").size(640.0, 360.0);
                    match open_window(options, move || child(model, main)) {
                        Ok(id) => {
                            model.child.set(Some(id));
                            model.child_openings.update(|n| *n += 1);
                            model.error.set(String::new());
                        }
                        Err(error) => model.error.set(error),
                    }
                }))
                .child(
                    button("Close target window").id("targets-close").min_h(44.0).disabled(move || model.child.get().is_none()).on_press(
                        move || {
                            if let Some(id) = model.child.get() {
                                close_window(id);
                            }
                        },
                    ),
                ),
        )
        .child(
            text(move || match model.child.get() {
                Some(id) => format!("Child: open (window {})", id.number()),
                None => "Child: closed".into(),
            })
            .id("targets-child"),
        )
        .child(text(move || format!("Child openings: {}", model.child_openings.get())).id("targets-child-openings"))
        .child(text("Use Inspector reload to rebuild all windows; named state and total hits survive.").text_sm())
        .child(text(move || model.error.get()).id("targets-error").text_color(Token::Danger))
        .into()
}

fn main() {
    octara_shell::run_with(
        octara_shell::Options { title: "Octara Target Validation".into(), size: (840.0, 620.0), ..Default::default() },
        targets,
    );
}

//! Declaration order is stable across incremental updates, patches and state overrides.

use octara::prelude::*;

fn opacity(window: &Window, id: &str) -> f32 {
    window.kernel().visual_of(id).unwrap().opacity
}

fn rect(window: &Window, id: &str) -> (f32, f32, f32, f32) {
    let k = window.kernel();
    k.rect(k.find(id).unwrap())
}

fn clean(window: &Window) {
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
}

#[test]
fn declaration_order_survives_updates_in_either_order() {
    let mut window = Window::headless(|| {
        let early = state("early", || 0.2f32);
        let late = state("late", || 0.6f32);
        on_command("early", move || early.set(0.4));
        on_command("late", move || late.set(0.8));
        col()
            .size_full()
            .child(el().id("static-last").size_8().opacity(early).opacity(0.7))
            .child(el().id("dynamic-last").size_8().opacity(0.7).opacity(early))
            .child(el().id("two-bindings").size_8().opacity(early).opacity(late))
            .into()
    });
    assert_eq!(opacity(&window, "static-last"), 0.7);
    assert_eq!(opacity(&window, "dynamic-last"), 0.2);
    assert_eq!(opacity(&window, "two-bindings"), 0.6);
    let node = window.kernel().find("two-bindings").unwrap();

    window.app().reset_work();
    window.invoke("early").unwrap();
    assert_eq!(window.app().work().closures_run, 3, "only early's three bindings run");
    assert_eq!(opacity(&window, "static-last"), 0.7);
    assert_eq!(opacity(&window, "dynamic-last"), 0.4);
    assert_eq!(opacity(&window, "two-bindings"), 0.6, "late keeps precedence without rerunning");
    window.app().reset_work();
    window.invoke("late").unwrap();
    assert_eq!(window.app().work().closures_run, 1);
    assert_eq!(opacity(&window, "two-bindings"), 0.8);
    assert_eq!(window.kernel().find("two-bindings"), Some(node));
    window.app().hot_reload();
    assert_eq!(opacity(&window, "static-last"), 0.7);
    assert_eq!(opacity(&window, "dynamic-last"), 0.4);
    assert_eq!(opacity(&window, "two-bindings"), 0.8);
    clean(&window);
}

#[test]
fn replacing_a_patch_restores_earlier_properties_and_defaults() {
    let mut window = Window::headless(|| {
        let active = state("active", || true);
        let width = state("width", || 70);
        on_command("toggle", move || active.update(|v| *v = !*v));
        on_command("width", move || width.set(90));
        col()
            .size_full()
            .child(
                el().id("panel")
                    .h(20)
                    .style_with(move |p| p.w(width.get()))
                    .style_with(move |p| if active.get() { p.w(120).opacity(0.3) } else { p }),
            )
            .into()
    });
    assert_eq!((rect(&window, "panel").2, opacity(&window, "panel")), (120.0, 0.3));
    window.invoke("toggle").unwrap();
    assert_eq!((rect(&window, "panel").2, opacity(&window, "panel")), (70.0, 1.0));
    window.invoke("width").unwrap();
    assert_eq!((rect(&window, "panel").2, opacity(&window, "panel")), (90.0, 1.0));
    window.app().hot_reload();
    assert_eq!((rect(&window, "panel").2, opacity(&window, "panel")), (90.0, 1.0));
    window.invoke("toggle").unwrap();
    assert_eq!((rect(&window, "panel").2, opacity(&window, "panel")), (120.0, 0.3));
    clean(&window);
}

#[test]
fn shorthand_and_longhand_follow_their_authored_positions() {
    let mut window = Window::headless(|| {
        let gap = state("gap", || 10);
        on_command("gap", move || gap.set(20));
        col()
            .size_full()
            .child(
                row()
                    .id("longhand-last")
                    .style_with(move |p| p.gap(gap.get()))
                    .gap_x(3)
                    .child(el().id("a").size(10))
                    .child(el().id("b").size(10)),
            )
            .child(
                row()
                    .id("shorthand-last")
                    .style_with(move |p| p.gap_x(gap.get()))
                    .gap(4)
                    .child(el().id("c").size(10))
                    .child(el().id("d").size(10)),
            )
            .into()
    });
    for changed in [false, true] {
        if changed {
            window.invoke("gap").unwrap();
        }
        assert_eq!(rect(&window, "b").0 - rect(&window, "a").0, 13.0);
        assert_eq!(rect(&window, "d").0 - rect(&window, "c").0, 14.0);
    }
    clean(&window);
}

#[test]
fn state_and_screen_overrides_resolve_over_the_current_ordered_base() {
    let mut window = Window::headless(|| {
        let width = state("width", || 60);
        let selected = state("selected", || false);
        on_command("width", move || width.set(100));
        on_command("select", move || selected.update(|v| *v = !*v));
        col()
            .size_full()
            .child(
                el().id("panel")
                    .h(20)
                    .style_with(move |p| p.w(width.get()))
                    .w(80)
                    .bg(Token::Surface)
                    .selected(selected)
                    .when_medium(|p| p.w(120))
                    .style_when(State::SELECTED, |p| p.w(160)),
            )
            .into()
    });
    assert_eq!(rect(&window, "panel").2, 120.0);
    window.invoke("select").unwrap();
    window.invoke("width").unwrap();
    assert_eq!(rect(&window, "panel").2, 160.0);
    window.invoke("select").unwrap();
    assert_eq!(rect(&window, "panel").2, 120.0);
    window.app().reset_work();
    window.resize(400.0, 600.0);
    window.app().set_theme(Theme::dark());
    assert_eq!(window.app().work().closures_run, 0, "theme and screen overrides need no binding rerun");
    assert_eq!(rect(&window, "panel").2, 80.0);
    assert_eq!(window.kernel().visual_of("panel").unwrap().background, Some(Theme::dark().get(Token::Surface)));
    clean(&window);
}

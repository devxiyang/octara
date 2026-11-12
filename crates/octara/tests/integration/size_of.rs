//! Element sizes as a reactive source (Rune's `layout_with`).

use octara::prelude::*;

fn view() -> View {
    // The panel is half the window; its content switches layout below 320 px.
    let narrow = memo(|| size_of("panel").get().0 < 320.0);
    row()
        .size_full()
        .child(el().id("panel").w_1_2().child(show_else(
            move || narrow.get(),
            || text("compact").id("compact").into(),
            || text("wide").id("wide").into(),
        )))
        .into()
}

#[test]
fn content_follows_its_containers_size() {
    let mut app = Window::headless(view);
    assert!(app.kernel().exists("wide"), "400 px panel");
    app.resize(500.0, 600.0);
    assert!(app.kernel().exists("compact"), "250 px panel");
    // Within the same side of 320 px only the memo reruns.
    app.app().reset_work();
    app.resize(520.0, 600.0);
    assert!(app.app().work().closures_run <= 1, "{:?}", app.app().work());
    assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
}

#[test]
fn a_size_that_feeds_back_into_itself_is_cut_and_reported() {
    let app = Window::headless(|| {
        // Each layout adds a character, so the text is wider than it was: never settles.
        row().child(text(move || "x".repeat(size_of("grower").get().0 as usize / 4 + 1)).id("grower").whitespace_nowrap()).into()
    });
    assert!(app.app().diagnostics().iter().any(|d| d.code == "LAYOUT_LOOP"), "{:?}", app.app().diagnostics());
}

//! Element sizes as a reactive source (Rune's `layout_with`).

use octara::platform::{HeadlessPlatform, WindowOptions};
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
fn a_size_that_feeds_back_into_itself_stays_pending_until_input_stops_it() {
    let app = App::new(HeadlessPlatform::default());
    let enabled = app.run(|| state("enabled", || false));
    let mut window = app.open_window(WindowOptions::new("Feedback"), move || feedback(enabled)).unwrap();
    assert!(window.unsettled().is_empty(), "{:?}", window.unsettled());
    let frames = window.frames_presented();

    app.run(|| enabled.set(true));

    assert!(app.diagnostics().iter().any(|d| d.code == "LAYOUT_LOOP"), "{:?}", app.diagnostics());
    assert!(!window.unsettled().is_empty(), "a layout cutoff must not claim that its frame settled");
    assert!(!app.pending().is_empty(), "App settlement must include the unfinished layout");
    assert_eq!(window.frames_presented(), frames, "an unfinished layout must not be presented");
    app.reset_work();
    let diagnostics = app.diagnostic_counts();
    for _ in 0..3 {
        assert!(!window.unsettled().is_empty());
        assert_eq!(app.next_deadline(), None, "suspended feedback must not schedule an immediate retry");
    }
    assert_eq!(app.work().closures_run, 0, "pending queries must not retry suspended layout");
    assert_eq!(app.diagnostic_counts(), diagnostics);
    assert_eq!(window.frames_presented(), frames);

    let reply = octara::inspect::handle(&mut window, octara::inspect::Request::Settle { limit_ms: Some(0) });
    assert!(!reply.ok, "Inspector settlement must fail while layout feedback is unfinished");
    assert_eq!(reply.code.as_deref(), Some("STEP_FAILED"));
    assert_eq!(window.frames_presented(), frames, "a settlement attempt must not present unfinished layout");

    window.dispatch(Event::Press("stop".into())).unwrap();

    assert!(window.unsettled().is_empty(), "{:?}", window.unsettled());
    assert_eq!(width(&window, "feedback"), 40.0);
    assert_eq!(window.kernel().text_of("measured").as_deref(), Some("40"));
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    assert!(window.frames_presented() > frames);
    let reply = octara::inspect::handle(&mut window, octara::inspect::Request::Settle { limit_ms: Some(0) });
    assert!(reply.ok, "{:?}", reply.message);
    app.shutdown();
}

#[ui]
fn feedback(enabled: Signal<bool>) -> View {
    let size = size_of("feedback");
    col()
        .size_full()
        .child(el().id("feedback").h(20).style_with(move |p| {
            // A bounded oscillation keeps the regression safe even if a frame retries it.
            p.w(if enabled.get() && size.get().0 == 40.0 { 80.0 } else { 40.0 })
        }))
        .child(text(move || size.get().0.to_string()).id("measured"))
        .child(el().id("stop").role(Role::Button).label("Stop feedback").size_8().on_press(move || enabled.set(false)))
        .child(el().id("exit").role(Role::Button).label("Exit").size_8().on_press(|| App::current().shutdown()))
        .into()
}

#[test]
fn pending_size_feedback_can_shutdown_from_an_input_handler() {
    let app = App::new(HeadlessPlatform::default());
    let enabled = app.run(|| state("enabled", || false));
    let mut window = app.open_window(WindowOptions::new("Feedback"), move || feedback(enabled)).unwrap();
    app.run(|| enabled.set(true));
    assert!(!window.unsettled().is_empty());
    let frames = window.frames_presented();
    let diagnostics = app.diagnostic_counts();

    window.dispatch(Event::Press("exit".into())).unwrap();

    assert!(app.is_shutdown());
    assert!(window.is_closed());
    assert!(window.unsettled().is_empty(), "{:?}", window.unsettled());
    app.reset_work();
    app.poll();
    assert_eq!(app.work().closures_run, 0);
    assert_eq!(app.diagnostic_counts(), diagnostics);
    assert_eq!(window.frames_presented(), frames);
}

#[test]
fn finite_size_feedback_never_reports_settled_with_a_stale_measurement() {
    let app = Window::headless(|| {
        let size = size_of("feedback");
        col()
            .size_full()
            .child(el().id("feedback").h(20).style_with(move |p| p.w((size.get().0 + 1.0).min(12.0))))
            .child(text(move || size.get().0.to_string()).id("measured"))
            .into()
    });

    for _ in 0..16 {
        if app.unsettled().is_empty() {
            let measured: f32 = app.kernel().text_of("measured").unwrap().parse().unwrap();
            assert_eq!(width(&app, "feedback"), measured, "a settled frame must refresh its size signal after its final layout");
            assert_eq!(measured, 12.0, "finite feedback needs more than four rounds to finish");
            assert!(app.layout_issues().is_empty(), "{:?}", app.layout_issues());
            app.app().shutdown();
            return;
        }
        app.app().poll();
    }
    panic!("finite feedback should settle after bounded later operations: {:?}", app.unsettled());
}

fn width(window: &Window, id: &str) -> f32 {
    let k = window.kernel();
    k.rect(k.find(id).unwrap()).2
}

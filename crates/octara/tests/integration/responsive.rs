//! Window size class styles (`when_medium` …), the breakpoint signal, and checks at several sizes.

use octara::prelude::*;

fn view() -> View {
    let wide = memo(move || viewport().get().0 >= 500.0);
    col()
        .child(
            row()
                .child(text("a").id("a"))
                .child(text("b").id("b"))
                .id("pair")
                .flex_col()
                .when_medium(|p| p.flex_row().gap_4())
                .when_expanded(|p| p.gap_8()),
        )
        .child(text(move || format!("{:?}", breakpoint().get())).id("bp"))
        .child(text(move || if wide.get() { "wide" } else { "narrow" }.to_string()).id("width"))
        .into()
}

fn pos(app: &Window, id: &str) -> (f32, f32) {
    let k = app.kernel();
    let r = k.rect(k.find(id).unwrap());
    (r.0, r.1)
}

#[test]
fn screen_styles_follow_the_window_without_running_closures() {
    let mut app = Window::headless(view);
    assert_eq!(app.breakpoint(), Breakpoint::Medium, "800 px is medium");
    let (ax, ay) = pos(&app, "a");
    let (bx, by) = pos(&app, "b");
    assert_eq!(ay, by, "medium: side by side");
    let md_gap = bx - ax;

    app.app().reset_work();
    app.resize(1100.0, 600.0);
    assert_eq!(pos(&app, "b").0 - pos(&app, "a").0, md_gap + 16.0, "expanded: gap_8 instead of gap_4");
    assert_eq!(app.kernel().text_of("bp").unwrap(), "Expanded");
    // Styles are resolved by the kernel: only the breakpoint text and the `wide` memo (which
    // reads the viewport; its result did not change) ran.
    assert_eq!(app.app().work().closures_run, 2);

    app.app().reset_work();
    app.resize(400.0, 600.0);
    assert_eq!(pos(&app, "a").0, pos(&app, "b").0, "compact: stacked");
    assert!(pos(&app, "b").1 > pos(&app, "a").1);
    assert_eq!(app.kernel().text_of("width").unwrap(), "narrow");

    // Within a breakpoint and on the same side of 500 px only the memo reruns; its readers do not.
    app.app().reset_work();
    app.resize(390.0, 700.0);
    assert_eq!(app.app().work().closures_run, 1, "memo cut off; breakpoint unchanged");
}

#[test]
fn checks_run_at_several_sizes_and_name_the_failing_one() {
    let mut app = Window::headless(view);
    let ok = app.at_sizes(&[(375.0, 800.0), (1280.0, 800.0)], |app| {
        let t = app.kernel().text_of("bp").unwrap();
        if t == "Compact" || t == "Large" { Ok(()) } else { Err(t) }
    });
    assert_eq!(ok, Ok(()));
    let err =
        app.at_sizes(
            &[(375.0, 800.0)],
            |app| {
                if app.kernel().text_of("width").unwrap() == "wide" { Ok(()) } else { Err("too narrow".into()) }
            },
        );
    assert_eq!(err, Err("at 375×800: too narrow".into()));
    assert_eq!(app.size(), (800.0, 600.0), "size restored");
}

//! Several windows of one app ([core](../../../../docs/design/core.md)): they share
//! signals, theme and fonts; each has its own tree; code runs as the window it belongs to;
//! closing one disposes its scope and frees it.

use octara::platform::{HeadlessPlatform, WindowOptions};
use octara::prelude::*;
use std::time::Duration;

/// A shared count, a label showing it, and a button opening another window.
fn counter(count: Signal<u32>) -> impl Fn() -> View + 'static {
    move || {
        col()
            .child(text(move || format!("Count: {}", count.get())).id("count"))
            .child(el().id("inc").role(Role::Button).label("Increment").size_8().on_press(move || count.update(|c| *c += 1)))
            .into()
    }
}

#[test]
fn windows_share_state_and_keep_their_own_trees() {
    let app = App::new(HeadlessPlatform::default());
    let count = app.run(|| Signal::new(0u32));
    let mut a = app.open_window(WindowOptions::new("A"), counter(count)).unwrap();
    let b = app.open_window(WindowOptions::new("B").size(400.0, 300.0), counter(count)).unwrap();
    assert_eq!(app.windows(), [a.id(), b.id()]);
    assert_eq!(b.size(), (400.0, 300.0), "each window its own size");

    a.tap("inc").unwrap();
    assert_eq!(a.kernel().text_of("count").as_deref(), Some("Count: 1"));
    assert_eq!(b.kernel().text_of("count").as_deref(), Some("Count: 1"), "a press in A shows in B");
    assert!(!std::ptr::eq(&*a.kernel(), &*b.kernel()), "two trees");
}

#[test]
fn code_runs_as_the_window_it_belongs_to() {
    let app = App::new(HeadlessPlatform::default());
    let seen = app.run(|| Signal::new(Vec::<WindowId>::new()));
    let view = move || {
        let here = current_window();
        spawn(async move {
            sleep(Duration::from_millis(10)).await;
            seen.update(|s| s.push(here));
            assert_eq!(current_window(), here, "a task runs as the window that spawned it");
        });
        el().into()
    };
    let a = app.open_window(WindowOptions::new("A"), view).unwrap();
    let b = app.open_window(WindowOptions::new("B"), view).unwrap();
    app.advance(Duration::from_millis(20));
    let mut got = app.run(|| seen.get());
    got.sort();
    let mut want = vec![a.id(), b.id()];
    want.sort();
    assert_eq!(got, want);
}

#[test]
fn a_handler_opens_a_window_at_the_end_of_its_frame() {
    let app = App::new(HeadlessPlatform::default());
    let mut main = app
        .open_window(WindowOptions::new("Main"), || {
            el().id("open")
                .role(Role::Button)
                .label("Open")
                .size_8()
                .on_press(|| {
                    open_window(WindowOptions::new("Preferences"), || text("Preferences").id("title").into()).unwrap();
                })
                .into()
        })
        .unwrap();
    main.tap("open").unwrap();
    let windows = app.windows();
    assert_eq!(windows.len(), 2);
    let prefs = app.window(windows[1]).unwrap();
    assert_eq!(prefs.options().title, "Preferences");
    assert_eq!(prefs.kernel().text_of("title").as_deref(), Some("Preferences"), "built by the end of the frame");
}

#[test]
fn closing_a_window_disposes_its_scope_and_frees_it() {
    let app = App::new(HeadlessPlatform::default());
    let ticks = app.run(|| Signal::new(0u32));
    let _main = app.open_window(WindowOptions::new("Main"), || el().into()).unwrap();
    let baseline = app.work().reactive_nodes;
    let view = move || {
        interval(Duration::from_millis(10), move || ticks.update(|t| *t += 1));
        let local = state("local", || 0u32);
        text(move || local.get().to_string()).into()
    };
    let w = app.open_window(WindowOptions::new("Ticking"), view).unwrap();
    app.advance(Duration::from_millis(25));
    let before = app.run(|| ticks.get());
    assert!(before >= 2, "its interval runs: {before}");
    app.close_window(w.id());
    assert!(w.is_closed());
    app.advance(Duration::from_millis(50));
    assert_eq!(app.run(|| ticks.get()), before, "its interval stopped with its scope");
    assert_eq!(app.windows().len(), 1);

    for _ in 0..1000 {
        let w = app.open_window(WindowOptions::new("Again"), view).unwrap();
        app.close_window(w.id());
    }
    assert_eq!(app.work().reactive_nodes, baseline, "a thousand opens and closes leave nothing behind");
}

#[test]
fn theme_and_fonts_are_the_apps() {
    let app = App::new(HeadlessPlatform::default());
    let view = || text("Shared").id("label").into();
    let a = app.open_window(WindowOptions::new("A"), view).unwrap();
    let b = app.open_window(WindowOptions::new("B"), view).unwrap();
    app.set_theme(Theme::dark());
    for w in [&a, &b] {
        assert_eq!(w.kernel().theme().name, "dark", "every window follows the theme");
        assert_eq!(w.kernel().visual_of("label").unwrap().text, Theme::dark().get(Token::Text), "its text restyled");
    }
    // One text system: the same text measures the same in both windows.
    let width = |w: &Window| {
        let n = w.kernel().find("label").unwrap();
        w.kernel().text_metrics(n).width
    };
    assert_eq!(width(&a), width(&b));
}

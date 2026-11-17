//! Several windows of one app ([core](../../../../docs/design/core.md)): they share
//! signals, theme and fonts; each has its own tree; code runs as the window it belongs to;
//! closing one disposes its scope and frees it.

use octara::platform::{HeadlessPlatform, WindowOptions};
use octara::prelude::*;
use std::time::Duration;

#[test]
fn later_window_measurements_relayout_an_earlier_window() {
    for entry in ["app", "semantic", "pointer"] {
        let app = App::new(HeadlessPlatform::default());
        let (width, measured) = app.run(|| (state("width", || 100.0f32), state("measured", || 100.0f32)));
        let mut a = app
            .open_window(WindowOptions::new("A"), move || {
                col()
                    .size_full()
                    .child(el().id("measured").h_8().style_with(move |p| p.w(measured.get())))
                    .child(el().id("resize").role(Role::Button).label("Resize").size_8().on_press(move || width.set(220.0)))
                    .into()
            })
            .unwrap();
        let b = app
            .open_window(WindowOptions::new("B"), move || {
                watch(|| size_of("source").get().0, move |width, _| measured.set(*width));
                col().size_full().child(el().id("source").h_8().style_with(move |p| p.w(width.get()))).into()
            })
            .unwrap();
        // The rebuild checker intentionally resets in-progress pointer gestures, including
        // PRESSED between a tap's down and up. Compare rebuilds for the other two entries.
        app.set_check_consistency(entry != "pointer");

        match entry {
            "app" => app.run(|| width.set(220.0)),
            "semantic" => a.dispatch(Event::Press("resize".into())).unwrap(),
            "pointer" => a.tap("resize").unwrap(),
            _ => unreachable!(),
        }

        let k = a.kernel();
        assert_eq!(k.rect(k.find("measured").unwrap()).2, 220.0, "A must lay out B's measurement in the same operation");
        assert!(a.layout_issues().is_empty(), "{:?}", a.layout_issues());
        assert!(b.layout_issues().is_empty(), "{:?}", b.layout_issues());
        assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
    }
}

#[test]
fn later_window_measurements_present_native_props_in_an_earlier_window() {
    use octara::platform::HeadlessWindow;
    use octara_primitives::native::NativeChange;

    let app = App::new(HeadlessPlatform::default());
    let (width, measured) = app.run(|| (state("width", || 100.0f32), state("measured", || 100.0f32)));
    let a = app
        .open_window(WindowOptions::new("A"), move || {
            col()
                .size_full()
                .child(
                    native_view("measurement")
                        .id("native")
                        .label("Measurement")
                        .props(move || serde_json::json!({ "width": measured.get() })),
                )
                .into()
        })
        .unwrap();
    let _b = app
        .open_window(WindowOptions::new("B"), move || {
            watch(|| size_of("source").get().0, move |width, _| measured.set(*width));
            col().size_full().child(el().id("source").h_8().style_with(move |p| p.w(width.get()))).into()
        })
        .unwrap();

    app.run(|| width.set(220.0));

    let host = a.platform_window();
    let native = &host.as_any().downcast_ref::<HeadlessWindow>().unwrap().native;
    assert!(matches!(native.last(), Some(NativeChange::Update { props, .. }) if props["width"] == 220.0), "{native:?}");
    assert!(a.layout_issues().is_empty(), "{:?}", a.layout_issues());
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
}

#[test]
fn cross_window_layout_feedback_is_bounded_and_can_recover() {
    let app = App::new(HeadlessPlatform::default());
    let (a_width, b_width, enabled) =
        app.run(|| (state("a_width", || 100.0f32), state("b_width", || 100.0f32), state("enabled", || false)));
    let view = move |width: Signal<f32>, peer: Signal<f32>| {
        move || {
            watch(
                move || (enabled.get(), size_of("source").get().0),
                move |(enabled, width), _| {
                    if *enabled {
                        peer.set(width + 1.0);
                    }
                },
            );
            col().size_full().child(el().id("source").h_8().style_with(move |p| p.w(width.get()))).into()
        }
    };
    let a = app.open_window(WindowOptions::new("A"), view(a_width, b_width)).unwrap();
    let b = app.open_window(WindowOptions::new("B"), view(b_width, a_width)).unwrap();
    app.reset_work();

    app.run(|| enabled.set(true));

    assert!(app.diagnostics().iter().any(|d| d.code == "LAYOUT_LOOP"), "{:?}", app.diagnostics());
    assert!(app.work().closures_run < 200, "cross-window feedback must stop in a bounded number of rounds: {:?}", app.work());
    assert!(
        [&a, &b].into_iter().any(|window| window.unsettled().iter().any(|reason| reason == "window updates are still pending")),
        "the cutoff must report the window that still needs layout"
    );
    app.run(|| enabled.set(false));
    for (w, width) in [(&a, a_width), (&b, b_width)] {
        assert!(w.unsettled().is_empty(), "{:?}", w.unsettled());
        let expected = app.run(|| width.get());
        let k = w.kernel();
        assert_eq!(k.rect(k.find("source").unwrap()).2, expected, "pending layout is retained after the cutoff");
        assert!(w.layout_issues().is_empty(), "{:?}", w.layout_issues());
    }
    app.reset_work();
    app.settle();
    assert_eq!(app.work().closures_run, 0, "a stopped feedback loop leaves no busy retry");
}

#[test]
fn app_run_inside_a_handler_keeps_shared_state_after_the_window_closes() {
    use std::cell::Cell;
    use std::rc::Rc;

    let app = App::new(HeadlessPlatform::default());
    let shared = Rc::new(Cell::new(None));
    let captured = shared.clone();
    let nested = app.clone();
    let mut window = app
        .open_window(WindowOptions::new("A"), move || {
            let captured = captured.clone();
            let nested = nested.clone();
            col()
                .size_full()
                .child(el().id("share").role(Role::Button).label("Share").size_8().on_press(move || {
                    captured.set(Some(nested.run(|| state("captured", || 7))));
                }))
                .into()
        })
        .unwrap();

    window.tap("share").unwrap();
    app.close_window(window.id());

    assert_eq!(app.run(|| shared.get().unwrap().get()), 7, "App::run state belongs to the app even when called from a window's handler");
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
}

#[test]
fn app_run_preserves_pure_computation_write_restrictions() {
    let app = App::new(HeadlessPlatform::default());
    let count = app.run(|| state("count", || 0));
    let nested = app.clone();
    let window = app
        .open_window(WindowOptions::new("A"), move || {
            let nested = nested.clone();
            let value = memo(move || {
                nested.run(|| count.set(1));
                1
            });
            col().size_full().child(text(move || value.get().to_string())).into()
        })
        .unwrap();

    assert_eq!(app.run(|| count.get()), 0);
    assert!(app.diagnostics().iter().any(|d| d.code == "WRITE_IN_DERIVE"), "{:?}", app.diagnostics());
    app.close_window(window.id());
}

#[test]
fn nested_app_run_reads_keep_the_callers_reactive_dependencies() {
    let app = App::new(HeadlessPlatform::default());
    let count = app.run(|| state("count", || 1));
    let nested = app.clone();
    let window = app
        .open_window(WindowOptions::new("A"), move || {
            let nested = nested.clone();
            let doubled = memo(move || nested.run(|| count.get() * 2));
            col().size_full().child(text(move || doubled.get().to_string()).id("doubled")).into()
        })
        .unwrap();

    app.run(|| count.set(3));

    assert_eq!(window.kernel().text_of("doubled").as_deref(), Some("6"));
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
    app.close_window(window.id());
}

#[test]
fn reactive_feedback_stays_unsettled_until_later_input_stops_it() {
    let app = App::new(HeadlessPlatform::default());
    let (count, enabled) = app.run(|| (state("count", || 0u32), state("enabled", || false)));
    let window = app
        .open_window(WindowOptions::new("A"), move || {
            watch(
                move || (enabled.get(), count.get()),
                move |(enabled, count_value), _| {
                    if *enabled {
                        count.set(count_value + 1);
                    }
                },
            );
            col().size_full().child(text(move || count.get().to_string()).id("count")).into()
        })
        .unwrap();
    app.set_check_consistency(true);

    app.run(|| enabled.set(true));

    assert!(app.diagnostics().iter().any(|d| d.code == "FLUSH_RUNAWAY"), "{:?}", app.diagnostics());
    assert!(window.unsettled().iter().any(|reason| reason == "reactive updates are still pending"), "{:?}", window.unsettled());
    app.reset_work();
    let diagnostics = app.diagnostic_counts();
    for _ in 0..3 {
        assert!(window.unsettled().iter().any(|reason| reason == "reactive updates are still pending"));
    }
    assert_eq!(app.work().closures_run, 0, "pending queries must not retry suspended work or run the rebuild checker");
    assert_eq!(app.diagnostic_counts(), diagnostics);
    app.run(|| enabled.set(false));
    assert!(window.unsettled().is_empty(), "{:?}", window.unsettled());
    let expected = app.run(|| count.get()).to_string();
    assert_eq!(window.kernel().text_of("count"), Some(expected));
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
}

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
    let count = app.run(|| state("ticks", || 0u32));
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
    let seen = app.run(|| state("seen", Vec::<WindowId>::new));
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
fn closing_and_shutdown_release_hosts_even_when_window_handles_survive() {
    use octara::platform::{Frame, Platform, PlatformWindow};
    use std::any::Any;
    use std::cell::RefCell;
    use std::rc::Rc;

    struct Host {
        id: WindowId,
        dropped: Rc<RefCell<Vec<WindowId>>>,
    }
    impl PlatformWindow for Host {
        fn present(&mut self, _: Frame) {}
        fn as_any(&self) -> &dyn Any {
            self
        }
    }
    impl Drop for Host {
        fn drop(&mut self) {
            self.dropped.borrow_mut().push(self.id);
        }
    }
    struct System(Rc<RefCell<Vec<WindowId>>>);
    impl Platform for System {
        fn read_text(&mut self) -> Option<String> {
            None
        }
        fn write_text(&mut self, _: &str) {}
        fn open_window(&mut self, id: WindowId, _: &WindowOptions) -> Result<Box<dyn PlatformWindow>, String> {
            Ok(Box::new(Host { id, dropped: self.0.clone() }))
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    let dropped = Rc::new(RefCell::new(Vec::new()));
    let app = App::new(System(dropped.clone()));
    let open = || app.open_window(WindowOptions::new("Same name"), || el().into()).unwrap();
    let first = open();
    let second = open();
    assert!(dropped.borrow().is_empty());
    app.close_window(first.id());
    assert_eq!(*dropped.borrow(), [first.id()], "closing immediately releases only its host");
    assert!(first.is_closed());
    assert!(!second.is_closed());
    let reopened = open();
    assert_ne!(reopened.id(), first.id());
    app.close_window(first.id());
    assert_eq!(*dropped.borrow(), [first.id()], "an old handle cannot close the replacement host");

    app.shutdown();
    assert!(second.is_closed() && reopened.is_closed());
    let mut expected = vec![first.id(), second.id(), reopened.id()];
    expected.sort();
    dropped.borrow_mut().sort();
    assert_eq!(*dropped.borrow(), expected, "shutdown releases each live host once");
    app.shutdown();
    drop((first, second, reopened));
    assert_eq!(*dropped.borrow(), expected, "dropping stale handles does not release a host twice");
}

#[test]
fn closing_a_window_disposes_its_scope_and_frees_it() {
    let app = App::new(HeadlessPlatform::default());
    let ticks = app.run(|| state("ticks", || 0u32));
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
    let before = width(&a);
    app.set_text_engine(Box::new(octara::kernel::text::MonoEngine));
    for w in [&a, &b] {
        assert_eq!(width(w), 6.0 * 8.0, "both windows remeasure with the new engine without an extra event");
        assert_ne!(width(w), before, "old measurements were invalidated");
        assert!(w.layout_issues().is_empty(), "{:?}", w.layout_issues());
    }
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
}

#[test]
fn two_windows_of_one_view_keep_their_own_state() {
    let app = App::new(HeadlessPlatform::default());
    let view = || {
        let n = state("n", || 0u32);
        el().id("count").size_8().on_press(move || n.update(|n| *n += 1)).child(text(move || n.get().to_string()).id("n")).into()
    };
    let mut a = app.open_window(WindowOptions::new("A"), view).unwrap();
    let b = app.open_window(WindowOptions::new("B"), view).unwrap();
    a.tap("count").unwrap();
    a.tap("count").unwrap();
    let n = |w: &octara::app::Window| w.kernel().text(w.kernel().find("n").unwrap()).to_owned();
    assert_eq!((n(&a), n(&b)), ("2".to_owned(), "0".to_owned()), "a root's state is its window's");
}

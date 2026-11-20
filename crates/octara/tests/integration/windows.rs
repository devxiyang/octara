//! Several windows of one app ([core](../../../../docs/design/core.md)): they share
//! signals, theme and fonts; each has its own tree; code runs as the window it belongs to;
//! closing one disposes its scope and frees it.

use octara::app::{AppConfig, Event};
use octara::platform::{Frame, HeadlessPlatform, Platform, PlatformWindow, WindowOptions};
use octara::prelude::*;
use octara::reactive::untrack;
use octara::task::WorkKind;
use octara::view::native_view;
use octara_primitives::native::NativeChange;
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

#[derive(Default)]
struct HostLog {
    opened: Vec<WindowId>,
    frames: Vec<(WindowId, Frame)>,
    dropped: Vec<WindowId>,
}

struct RecordingPlatform(Rc<RefCell<HostLog>>);

impl Platform for RecordingPlatform {
    fn read_text(&mut self) -> Option<String> {
        None
    }
    fn write_text(&mut self, _: &str) {}
    fn open_window(&mut self, id: WindowId, _: &WindowOptions) -> Result<Box<dyn PlatformWindow>, String> {
        self.0.borrow_mut().opened.push(id);
        Ok(Box::new(RecordingWindow { id, log: self.0.clone() }))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct RecordingWindow {
    id: WindowId,
    log: Rc<RefCell<HostLog>>,
}

impl PlatformWindow for RecordingWindow {
    fn present(&mut self, frame: Frame) {
        self.log.borrow_mut().frames.push((self.id, frame));
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Drop for RecordingWindow {
    fn drop(&mut self) {
        self.log.borrow_mut().dropped.push(self.id);
    }
}

fn recording_app() -> (App, Rc<RefCell<HostLog>>) {
    let log = Rc::new(RefCell::new(HostLog::default()));
    (App::new(RecordingPlatform(log.clone())), log)
}

#[test]
fn display_ticks_publish_motion_only_in_the_window_that_needs_it() {
    let samples = [60, 120].map(|hz| {
        let (app, log) = recording_app();
        let lit = app.run(|| state("lit", || false));
        let color = move || if lit.get() { rgb(0xff0000) } else { rgb(0) };
        let mut moving =
            app.open_window(WindowOptions::new("Moving"), move || el().id("color").size_8().bg(color).motion(Spring::SNAP)).unwrap();
        let still = app.open_window(WindowOptions::new("Still"), move || el().size_8().bg(color)).unwrap();
        let node = moving.kernel().find("color").unwrap();
        let red = |frame: &octara::platform::PublishedFrame| {
            frame.list.quads().find(|quad| quad.source == node).unwrap().fill.unwrap().components[0]
        };
        let demand = |window: &Window| {
            let log = log.borrow();
            let (_, frame) = log.frames.iter().rfind(|(id, _)| *id == window.id()).unwrap();
            (frame.redraw, frame.continuous)
        };
        let first = moving.published_frame().unwrap();
        let still_before = still.published_revision().unwrap();
        assert_eq!(first.window, moving.id());
        assert_eq!(red(&first), 0.0);
        assert_eq!(demand(&moving), (true, false));

        app.run(|| lit.set(true));
        assert_eq!(demand(&moving), (true, true));
        assert_eq!(demand(&still), (true, false));
        assert_eq!(app.next_timer_deadline(), None);
        assert_eq!(app.next_deadline(), Some(Duration::from_secs_f64(1.0 / 60.0)));
        let still_frame = still.published_frame().unwrap();
        assert!(still_frame.revision > still_before);

        let mut last = moving.published_revision().unwrap();
        let mut samples = 0;
        let mut intermediate = false;
        for tick in 1..=3 * hz {
            let sample = Duration::from_secs_f64(f64::from(tick) / f64::from(hz));
            app.advance_to(sample);
            let frame = moving.published_frame().unwrap();
            assert!(frame.revision > last, "{hz} Hz sample {tick}");
            assert_eq!(frame.sampled_at, sample);
            assert_eq!(still.published_revision(), Some(still_frame.revision));
            last = frame.revision;
            if !demand(&moving).1 {
                assert_eq!(demand(&moving), (true, false), "the terminal sample still needs a draw");
                assert_eq!(red(&frame), 1.0);
                break;
            }
            let value = red(&frame);
            intermediate |= value > 0.0 && value < 1.0;
            samples += 1;
        }
        assert!(intermediate);
        assert_eq!(app.next_deadline(), None);
        assert_eq!(red(&first), 0.0, "retaining a published frame pins its old drawing");
        let final_frame = moving.published_frame().unwrap();
        app.advance(Duration::from_secs(1));
        let idle = moving.published_frame().unwrap();
        assert_eq!(idle.revision, final_frame.revision);
        assert_eq!(idle.sampled_at, final_frame.sampled_at);
        assert!(Rc::ptr_eq(&idle.list, &final_frame.list));

        moving.event(octara::app::InputEvent::Resize { width: 320.0, height: 200.0, scale: 2.0, safe_area: None });
        let resized = moving.published_frame().unwrap();
        assert!(resized.revision > idle.revision);
        assert_eq!((resized.list.size, resized.scale), ((320.0, 200.0), 2.0));
        app.close_window(moving.id());
        assert!(moving.published_frame().is_none());
        app.shutdown();
        samples
    });
    assert!(samples[1] >= 2 * samples[0], "{samples:?}");
}

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
            })
            .unwrap();
        let b = app
            .open_window(WindowOptions::new("B"), move || {
                watch(|| size_of("source").get().0, move |width, _| measured.set(*width));
                col().size_full().child(el().id("source").h_8().style_with(move |p| p.w(width.get())))
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
    let (app, host) = recording_app();
    let (width, measured) = app.run(|| (state("width", || 100.0f32), state("measured", || 100.0f32)));
    let mut a = app
        .open_window(WindowOptions::new("A"), move || {
            let caption = state("caption", || "Initial");
            col()
                .size_full()
                .child(
                    native_view("measurement")
                        .id("native")
                        .label("Measurement")
                        .size_8()
                        .props(move || serde_json::json!({ "caption": caption.get(), "width": measured.get() })),
                )
                .child(el().id("resize").role(Role::Button).label("Resize").size_8().on_press(move || {
                    caption.set("Updated");
                    width.set(220.0);
                }))
        })
        .unwrap();
    let _b = app
        .open_window(WindowOptions::new("B"), move || {
            watch(|| size_of("source").get().0, move |width, _| measured.set(*width));
            col().size_full().child(el().id("source").h_8().style_with(move |p| p.w(width.get())))
        })
        .unwrap();

    let first_frame = host.borrow().frames.len();
    a.dispatch(Event::Press("resize".into())).unwrap();

    let host = host.borrow();
    let props: Vec<_> = host.frames[first_frame..]
        .iter()
        .filter(|(id, _)| *id == a.id())
        .flat_map(|(_, frame)| &frame.native)
        .filter_map(|change| match change {
            NativeChange::Create { props, .. } | NativeChange::Update { props, .. } => Some(props),
            _ => None,
        })
        .collect();
    assert_eq!(props, [&serde_json::json!({ "caption": "Updated", "width": 220.0 })]);
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
            col().size_full().child(el().id("source").h_8().style_with(move |p| p.w(width.get())))
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
    let app = App::new(HeadlessPlatform::default());
    let shared = Rc::new(Cell::new(None));
    let captured = shared.clone();
    let nested = app.clone();
    let mut window = app
        .open_window(WindowOptions::new("A"), move || {
            let captured = captured.clone();
            let nested = nested.clone();
            col().size_full().child(el().id("share").role(Role::Button).label("Share").size_8().on_press(move || {
                captured.set(Some(nested.run(|| state("captured", || 7))));
            }))
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
            col().size_full().child(text(move || value.get().to_string()))
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
            col().size_full().child(text(move || doubled.get().to_string()).id("doubled"))
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
            col().size_full().child(text(move || count.get().to_string()).id("count"))
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
        el()
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
    #[ui]
    fn preferences() -> El {
        col()
            .size_full()
            .child(text("Preferences").id("title"))
            .child(native_view("preview").label("Preview").size_8().props(serde_json::json!({ "caption": "Preferences" })))
    }

    let (app, host) = recording_app();
    let mut main = app
        .open_window(WindowOptions::new("Main"), || {
            el().id("open").role(Role::Button).label("Open").size_8().on_press(|| {
                open_window(WindowOptions::new("Preferences"), preferences).unwrap();
            })
        })
        .unwrap();
    main.tap("open").unwrap();
    let windows = app.windows();
    assert_eq!(windows.len(), 2);
    let prefs = app.window(windows[1]).unwrap();
    assert_eq!(prefs.options().title, "Preferences");
    assert_eq!(prefs.kernel().text_of("title").as_deref(), Some("Preferences"), "built by the end of the frame");

    let host = host.borrow();
    let (_, first) = host.frames.iter().find(|(id, _)| *id == prefs.id()).expect("preferences was presented");
    assert!(first.redraw, "{first:?}");
    let Some(NativeChange::Create { id, kind, props, .. }) = first.native.first() else {
        panic!("missing mounted content in the first frame: {first:?}");
    };
    assert_eq!((kind.as_str(), props["caption"].as_str()), ("preview", Some("Preferences")));
    assert!(
        first
            .native
            .iter()
            .any(|change| { matches!(change, NativeChange::Place { placements } if placements.iter().any(|p| p.id == *id && p.visible)) }),
        "missing the mounted view's first placement: {first:?}"
    );
}

#[test]
fn immediate_watches_open_windows_within_one_app_update_budget() {
    #[ui]
    fn chain(mounted: Rc<Cell<usize>>, limit: Rc<Cell<usize>>) -> View {
        let ordinal = mounted.get() + 1;
        mounted.set(ordinal);
        watch(
            || (),
            move |_, _| {
                if ordinal < limit.get() {
                    let mounted = mounted.clone();
                    let limit = limit.clone();
                    open_window(WindowOptions::new("Chain"), move || chain(mounted.clone(), limit.clone())).unwrap();
                }
            },
        );
        text("Chain").id("chain").size_full().into()
    }

    let app = App::new(HeadlessPlatform::default());
    let mounted = Rc::new(Cell::new(0));
    let limit = Rc::new(Cell::new(20)); // A regression must fail the assertion without exhausting the test thread's stack.
    let (root_mounted, root_limit) = (mounted.clone(), limit.clone());
    app.run(|| {
        open_window(WindowOptions::new("Chain"), move || chain(root_mounted.clone(), root_limit.clone())).unwrap();
    });

    let first_update = mounted.get();
    assert!((1..=16).contains(&first_update), "mounted {first_update} roots in one App update");
    assert!(app.diagnostics().iter().any(|d| d.code == "LAYOUT_LOOP"), "{:?}", app.diagnostics());
    let pending = app.pending();
    assert!(pending.iter().any(|work| work.kind == WorkKind::Update), "{pending:?}");
    assert_eq!(mounted.get(), first_update, "observing pending work must leave it queued");

    limit.set(first_update);
    app.run(|| {});

    assert_eq!(mounted.get(), first_update + 1, "the queued root mounts in the next update");
    let windows = app.windows();
    assert_eq!(windows.len(), mounted.get());
    let last = app.window(*windows.last().unwrap()).unwrap();
    assert_eq!(last.kernel().text_of("chain").as_deref(), Some("Chain"));
    assert!(last.frames_presented() > 0);
    assert!(app.pending().is_empty(), "{:?}", app.pending());
    app.shutdown();
}

#[test]
fn pure_phases_cannot_open_or_close_windows() {
    #[ui]
    fn ready() -> View {
        text("Ready").size_full().into()
    }

    for phase in ["build", "init", "memo", "binding", "watch source"] {
        let (app, host) = recording_app();
        let (foreign, foreign_host) = recording_app();
        let main = app.open_window(WindowOptions::new("Main"), ready).unwrap();
        let other = foreign.open_window(WindowOptions::new("Other"), ready).unwrap();
        let (main_id, other_id) = (main.id(), other.id());
        let target = foreign.clone();
        let probe = app
            .open_window(WindowOptions::new(phase), move || {
                let target = target.clone();
                let attempt = move || {
                    assert!(open_window(WindowOptions::new("Refused"), ready).is_err(), "{phase}: ambient open");
                    close_window(main_id);
                    for (app, id) in [(&App::current(), main_id), (&target, other_id)] {
                        assert!(app.open_window(WindowOptions::new("Refused"), ready).is_err(), "{phase}: handle open");
                        app.close_window(id);
                    }
                    "Ready".to_owned()
                };
                let label = match phase {
                    "build" => text(attempt()),
                    "init" => text(state("initial", attempt)),
                    "memo" => text(memo(attempt)),
                    "binding" => text(attempt),
                    "watch source" => {
                        watch(attempt, |_, _| {});
                        text("Ready")
                    }
                    _ => unreachable!(),
                };
                label.id("probe").size_full()
            })
            .unwrap();

        assert_eq!(probe.kernel().text_of("probe").as_deref(), Some("Ready"));
        assert_eq!(app.windows(), [main_id, probe.id()], "{phase}");
        assert_eq!(host.borrow().opened, [main_id, probe.id()], "{phase}");
        assert_eq!(foreign.windows(), [other_id], "{phase}");
        assert_eq!(foreign_host.borrow().opened, [other_id], "{phase}");
        assert!(!main.is_closed() && !other.is_closed(), "{phase}");
        let code = if matches!(phase, "build" | "init") { "WRITE_IN_BUILD" } else { "WRITE_IN_DERIVE" };
        let diagnostics = app.diagnostic_counts();
        assert!(diagnostics.iter().all(|&(actual, _)| actual == code), "{phase}: {diagnostics:?}");
        assert_eq!(diagnostics.iter().map(|&(_, count)| count).sum::<u32>(), 6, "{phase}: one diagnostic per request");
        assert!(foreign.diagnostics().is_empty(), "{:?}", foreign.diagnostics());
        app.shutdown();
        foreign.shutdown();
    }
}

#[test]
fn closing_a_window_in_its_opening_turn_skips_mount_and_presentation() {
    for through_handle in [false, true] {
        let (app, host) = recording_app();
        let mounted = Rc::new(Cell::new(false));
        let root_mounted = mounted.clone();
        let id = app.run(|| {
            let options = WindowOptions::new("Canceled");
            let root = move || {
                root_mounted.set(true);
                text("Canceled").size_full()
            };
            if through_handle {
                let window = app.open_window(options, root).unwrap();
                app.close_window(window.id());
                window.id()
            } else {
                let id = open_window(options, root).unwrap();
                close_window(id);
                id
            }
        });

        assert!(!mounted.get(), "through_handle={through_handle}");
        assert!(app.window(id).is_none());
        assert!(app.windows().is_empty());
        assert!(host.borrow().frames.is_empty());
        assert!(app.pending().is_empty(), "{:?}", app.pending());
        assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
        app.shutdown();
    }
}

#[test]
fn retiring_during_scope_mount_skips_remaining_content_and_presentation() {
    struct DeferredContent(Rc<RefCell<Vec<&'static str>>>);

    impl octara::view::IntoView for DeferredContent {
        fn into_view(self, _: &'static std::panic::Location<'static>) -> View {
            self.0.borrow_mut().push("content conversion");
            text(move || {
                self.0.borrow_mut().push("binding");
                "Closing".to_owned()
            })
            .into()
        }
    }

    for shutdown in [false, true] {
        let (app, host) = recording_app();
        let baseline = app.work().reactive_nodes;
        let events = Rc::new(RefCell::new(Vec::new()));
        let mounted = events.clone();
        let window = app
            .open_window(WindowOptions::new("Retiring"), move || {
                let (first, later) = (mounted.clone(), mounted.clone());
                col()
                    .id("parent")
                    .size_full()
                    .child(scope("closing", move || {
                        let cleaned = first.clone();
                        on_cleanup(move || cleaned.borrow_mut().push("cleanup"));
                        watch(
                            || (),
                            move |_, _| {
                                if shutdown {
                                    App::current().shutdown();
                                } else {
                                    close_window(current_window());
                                }
                            },
                        );
                        first.borrow_mut().push("factory finished");
                        DeferredContent(first)
                    }))
                    .child(scope("later", move || {
                        later.borrow_mut().push("later factory");
                        text("Later")
                    }))
            })
            .unwrap();

        assert!(window.is_closed());
        assert_eq!(app.is_shutdown(), shutdown);
        assert!(!app.is_failed());
        assert_eq!(app.work().reactive_nodes, baseline);
        assert!(!window.kernel().exists("parent"));
        assert!(host.borrow().frames.is_empty());
        assert_eq!(host.borrow().dropped, [window.id()]);
        let expected = if shutdown { ["cleanup", "factory finished"] } else { ["factory finished", "cleanup"] };
        assert_eq!(*events.borrow(), expected);
        assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
        app.shutdown();
    }
}

#[test]
fn retiring_from_a_content_factory_skips_its_conversion() {
    struct Unconverted;

    impl IntoView for Unconverted {
        fn into_view(self, _: &'static std::panic::Location<'static>) -> View {
            panic!("a retired factory must not convert its result");
        }
    }

    fn retire() -> Unconverted {
        watch(|| (), |_, _| close_window(current_window()));
        Unconverted
    }

    #[derive(Debug)]
    enum Factory {
        Root,
        Show,
        Keyed,
        AnchoredOverlay,
        WindowOverlay,
    }

    for factory in [Factory::Root, Factory::Show, Factory::Keyed, Factory::AnchoredOverlay, Factory::WindowOverlay] {
        let (app, host) = recording_app();
        let options = WindowOptions::new("Retiring");
        let window = match factory {
            Factory::Root => app.open_window(options, retire),
            Factory::Show => app.open_window(options, || show(|| true, retire)),
            Factory::Keyed => app.open_window(options, || keyed(|| "key", |_| retire())),
            Factory::AnchoredOverlay => app.open_window(options, || el().size_8().overlay(|| true, Placement::BELOW, retire)),
            Factory::WindowOverlay => app.open_window(options, || el().overlay_window(|| true, retire)),
        }
        .unwrap();
        assert!(window.is_closed(), "{factory:?}");
        assert!(host.borrow().frames.is_empty());
        assert_eq!(host.borrow().dropped, [window.id()]);
        assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
        app.shutdown();
    }
}

#[test]
fn a_string_root_uses_the_headless_call_site_as_its_source() {
    let line = line!() + 1;
    let window = Window::headless(|| "Ready");
    let kernel = window.kernel();
    let node = kernel.children(kernel.root())[0];
    let source = kernel.origin(node).unwrap();
    assert_eq!(kernel.text(node), "Ready");
    assert_eq!((source.file(), source.line()), (file!(), line));
}

#[test]
fn closing_and_shutdown_release_hosts_even_when_window_handles_survive() {
    let (app, host) = recording_app();
    let open = || app.open_window(WindowOptions::new("Same name"), || el()).unwrap();
    let first = open();
    let second = open();
    assert!(host.borrow().dropped.is_empty());
    app.close_window(first.id());
    assert_eq!(host.borrow().dropped, [first.id()], "closing immediately releases only its host");
    assert!(first.is_closed());
    assert!(!second.is_closed());
    let reopened = open();
    assert_ne!(reopened.id(), first.id());
    app.close_window(first.id());
    assert_eq!(host.borrow().dropped, [first.id()], "an old handle cannot close the replacement host");

    app.shutdown();
    assert!(second.is_closed() && reopened.is_closed());
    let mut expected = vec![first.id(), second.id(), reopened.id()];
    expected.sort();
    host.borrow_mut().dropped.sort();
    assert_eq!(host.borrow().dropped, expected, "shutdown releases each live host once");
    app.shutdown();
    drop((first, second, reopened));
    assert_eq!(host.borrow().dropped, expected, "dropping stale handles does not release a host twice");
}

#[test]
fn closing_a_window_disposes_its_scope_and_frees_it() {
    let app = App::new(HeadlessPlatform::default());
    let ticks = app.run(|| state("ticks", || 0u32));
    let _main = app.open_window(WindowOptions::new("Main"), || el()).unwrap();
    let baseline = app.work().reactive_nodes;
    let view = move || {
        interval(Duration::from_millis(10), move || ticks.update(|t| *t += 1));
        let local = state("local", || 0u32);
        text(move || local.get().to_string())
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
fn configured_resources_are_visible_during_build_and_first_frame() {
    use octara::app::TargetPlatform;

    let builds = Rc::new(Cell::new(0));
    let config = AppConfig {
        theme: Theme::dark().breakpoint(Breakpoint::Medium, 900.0),
        text_engine: Box::new(octara_text::MonoEngine),
        target_platform: TargetPlatform::Ios,
    };
    let app = App::with_config(HeadlessPlatform::default(), config);
    for name in ["First", "Second"] {
        let builds = builds.clone();
        let window = app
            .open_window(WindowOptions::new(name), move || {
                builds.set(builds.get() + 1);
                assert_eq!(App::current().theme().name, "dark", "configuration is visible before any view exists");
                assert_eq!(App::current().target_platform(), TargetPlatform::Ios);
                assert_eq!(untrack(|| breakpoint().get()), Breakpoint::Compact, "the initial breakpoint uses the configured theme");
                col()
                    .size_full()
                    .items_start()
                    .child(text("Ready").id("label").text_size(px(20.0)))
                    .child(el().id("touch").w(px(30.0)).h_8().when_touch(|p| p.w(px(80.0))))
            })
            .unwrap();
        let k = window.kernel();
        assert_eq!(k.text_metrics(k.find("label").unwrap()).width, 50.0);
        assert_eq!(k.visual_of("label").unwrap().text, Theme::dark().get(Token::Text));
        assert_eq!(k.rect(k.find("touch").unwrap()).2, 80.0, "touch policy applied in the initial layout");
        assert_eq!(window.frames_presented(), 1, "no second frame to fix startup resources");
        assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    }
    assert_eq!(builds.get(), 2, "each window builds once");
    assert!(app.pending().is_empty(), "initialization needs no deferred task");
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
}

#[test]
fn theme_and_text_engine_are_the_apps() {
    let app =
        App::with_config(HeadlessPlatform::default(), AppConfig { text_engine: Box::new(octara_text::MonoEngine), ..Default::default() });
    let view = || text("Shared").id("label");
    let a = app.open_window(WindowOptions::new("A"), view).unwrap();
    let b = app.open_window(WindowOptions::new("B"), view).unwrap();
    app.set_theme(Theme::dark());
    for w in [&a, &b] {
        assert_eq!(w.kernel().theme().name, "dark", "every window follows the theme");
        assert_eq!(w.kernel().visual_of("label").unwrap().text, Theme::dark().get(Token::Text), "its text restyled");
        let k = w.kernel();
        assert_eq!(k.text_metrics(k.find("label").unwrap()).width, 6.0 * 8.0, "both windows use the configured engine");
        assert!(w.layout_issues().is_empty(), "{:?}", w.layout_issues());
    }
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
}

#[test]
fn two_windows_of_one_view_keep_their_own_state() {
    let app = App::new(HeadlessPlatform::default());
    let view = || {
        let n = state("n", || 0u32);
        el().id("count").size_8().on_press(move || n.update(|n| *n += 1)).child(text(move || n.get().to_string()).id("n"))
    };
    let mut a = app.open_window(WindowOptions::new("A"), view).unwrap();
    let b = app.open_window(WindowOptions::new("B"), view).unwrap();
    a.tap("count").unwrap();
    a.tap("count").unwrap();
    let n = |w: &octara::app::Window| w.kernel().text(w.kernel().find("n").unwrap()).to_owned();
    assert_eq!((n(&a), n(&b)), ("2".to_owned(), "0".to_owned()), "a root's state is its window's");
}

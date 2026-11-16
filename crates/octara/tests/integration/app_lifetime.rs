//! App ownership is explicit and survives its windows until shutdown.

use octara::platform::HeadlessPlatform;
use octara::prelude::*;
use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::time::Duration;

#[test]
fn app_state_initializes_once_and_is_inherited_by_reopened_windows() {
    let app = App::new(HeadlessPlatform::default());
    let initialized = Cell::new(0);
    let count = app.run(|| {
        let count = state("count", || {
            initialized.set(initialized.get() + 1);
            3
        });
        provide(count);
        count
    });
    for _ in 0..50 {
        let same = app.run(|| state("count", || panic!("existing App state must not initialize again")));
        assert_eq!(count, same);
    }
    assert_eq!(initialized.get(), 1);
    let view = || {
        let count = use_context::<Signal<i32>>().unwrap();
        col().size_full().child(text(move || count.get().to_string()).id("count")).into()
    };
    let first = app.open_window(WindowOptions::new("First"), view).unwrap();
    app.hot_reload();
    app.close_window(first.id());
    app.run(|| count.set(8));
    let second = app.open_window(WindowOptions::new("Second"), view).unwrap();
    assert_eq!(second.kernel().text_of("count").as_deref(), Some("8"));
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
    app.shutdown();
    assert!(second.is_closed());
}

#[test]
fn an_app_key_type_conflict_preserves_the_original_state() {
    let app = App::new(HeadlessPlatform::default());
    let value = app.run(|| state("value", || 7u32));
    let initialized = Cell::new(false);
    assert!(
        app.run(|| catch_unwind(AssertUnwindSafe(|| state("value", || {
            initialized.set(true);
            "wrong type".to_owned()
        }))))
        .is_err()
    );
    assert!(!initialized.get());
    assert_eq!(app.run(|| value.get()), 7);
    assert_eq!(app.run(|| state("value", || 0u32)), value);
    assert_eq!(app.diagnostics()[0].code.id, "STATE_TYPE_CHANGED");
    app.shutdown();
}

#[test]
fn shutdown_closes_windows_retires_work_and_runs_cleanup_once() {
    let app = App::new(HeadlessPlatform::default());
    let cleaned = Rc::new(RefCell::new(Vec::new()));
    let root_cleaned = cleaned.clone();
    let (send, task) = app.run(|| {
        let value = state("value", || 42);
        on_cleanup(move || {
            root_cleaned.borrow_mut().push(format!("app {}", value.get()));
            App::current().shutdown(); // Shutdown is safe to call again from cleanup.
        });
        let (send, receive) = channel::<u32>();
        receive.named("root-events").on(move |v| value.set(v));
        let task = spawn(async {
            sleep(Duration::from_secs(60)).await;
            panic!("cancelled timer ran");
        });
        (send, task)
    });
    let child_cleaned = cleaned.clone();
    let mut window = app
        .open_window(WindowOptions::new("Child"), move || {
            let cleaned = child_cleaned.clone();
            on_cleanup(move || cleaned.borrow_mut().push("window".into()));
            col().size_full().child(el().id("action").role(Role::Button).label("Action").size_8().on_press(|| panic!("late input"))).into()
        })
        .unwrap();
    app.poll();
    app.shutdown();
    app.shutdown();
    assert!(app.is_shutdown());
    assert!(window.is_closed());
    assert!(task.is_finished());
    assert!(send.send(99).is_err());
    assert!(app.send("root-events", "99").is_err());
    assert!(window.dispatch(Event::Press("action".into())).is_err());
    assert_eq!(&*cleaned.borrow(), &["window", "app 42"]);
    assert!(app.pending().is_empty());
    assert_eq!(app.next_deadline(), None);
    app.advance(Duration::from_secs(120));
    app.poll();
    assert!(app.open_window(WindowOptions::new("Late"), || panic!("late window build")).is_err());
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
    let called = Cell::new(false);
    assert!(catch_unwind(AssertUnwindSafe(|| app.run(|| called.set(true)))).is_err());
    assert!(!called.get(), "shutdown rejects work before user code");
}

#[test]
fn shutdown_from_a_window_handler_does_not_resume_that_window() {
    let app = App::new(HeadlessPlatform::default());
    let mut window = app
        .open_window(WindowOptions::new("Exit"), || {
            col().size_full().child(el().id("exit").role(Role::Button).label("Exit").size_8().on_press(|| App::current().shutdown())).into()
        })
        .unwrap();
    window.dispatch(Event::Press("exit".into())).unwrap();
    assert!(window.is_closed());
    assert!(app.is_shutdown());
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
}

#[test]
fn a_pure_computation_cannot_shutdown_the_app() {
    let app = App::new(HeadlessPlatform::default());
    app.run(|| {
        let derived = memo(|| {
            untrack(|| App::current().shutdown());
            1
        });
        assert_eq!(derived.get(), 1);
    });
    assert!(!app.is_shutdown());
    assert!(app.diagnostics().iter().any(|d| d.code.id == "WRITE_IN_DERIVE"));
    app.shutdown();
    assert!(app.is_shutdown());
}

#[test]
fn an_escaped_panic_blocks_all_windows_but_allows_teardown_and_other_apps() {
    let app = App::new(HeadlessPlatform::default());
    let cleaned = Rc::new(Cell::new(0));
    let value = app.run(|| {
        let value = state("value", || 1u32);
        let cleaned = cleaned.clone();
        on_cleanup(move || cleaned.set(value.get()));
        value
    });
    let mut first = app
        .open_window(WindowOptions::default(), move || {
            el().id("fail")
                .size(40)
                .on_press(move || {
                    value.update(|v| {
                        *v = 9;
                        panic!("partially changed application state");
                    })
                })
                .into()
        })
        .unwrap();
    let invoked = Rc::new(Cell::new(false));
    let called = invoked.clone();
    let mut second = app
        .open_window(WindowOptions::default(), move || {
            let called = called.clone();
            el().id("other").size(40).on_press(move || called.set(true)).into()
        })
        .unwrap();
    let frames = (first.frames_presented(), second.frames_presented());
    assert!(catch_unwind(AssertUnwindSafe(|| first.tap("fail"))).is_err());
    assert!(app.is_failed());
    assert!(second.tap("other").unwrap_err().contains("APP_FAILED"));
    assert!(second.dispatch(Event::Press("other".into())).unwrap_err().contains("APP_FAILED"));
    assert!(second.force_state("other", State::HOVER, true).unwrap_err().contains("APP_FAILED"));
    assert!(second.press_key("tab").unwrap_err().contains("APP_FAILED"));
    assert!(app.send("missing", "payload").unwrap_err().contains("APP_FAILED"));
    let windows = app.windows();
    assert!(app.open_window(WindowOptions::default(), || panic!("must not build")).is_err());
    assert_eq!(app.windows(), windows, "rejected opening must not allocate a window");
    assert!(app.run_until_idle(std::time::Duration::ZERO).is_err());
    assert!(!invoked.get());
    assert!(catch_unwind(AssertUnwindSafe(|| app.run(|| value.get()))).is_err());
    assert_eq!((first.frames_presented(), second.frames_presented()), frames);
    assert!(!first.unsettled().is_empty());
    assert!(!second.unsettled().is_empty());
    let picture = octara::capture::Picture { width: 1, height: 1, rgba: vec![0, 0, 0, 255] };
    let path = std::env::temp_dir().join(format!("octara-failed-app-{}.png", std::process::id()));
    assert!(second.capture_display(&picture, path.to_str().unwrap(), 1.0, vec![]).unwrap_err().contains("APP_FAILED"));
    assert!(!path.exists(), "failure is refused even if the caller supplied an empty pending list");
    assert!(app.diagnostics().iter().any(|d| d.code == "APP_FAILED"));
    let independent = App::new(HeadlessPlatform::default());
    assert_eq!(independent.run(|| state("ok", || 42).get()), 42);
    assert!(!independent.is_failed());
    independent.shutdown();
    app.shutdown();
    assert_eq!(cleaned.get(), 9, "teardown sees the actual partial state; no rollback is promised");
    assert!(first.is_closed() && second.is_closed());
    assert!(app.pending().is_empty());
}

#[test]
fn catching_a_nested_entry_panic_does_not_allow_the_outer_frame_to_publish() {
    let mut window = Window::headless(|| {
        el().id("nested")
            .size(40)
            .on_press(|| {
                let result = catch_unwind(AssertUnwindSafe(|| App::current().run(|| panic!("nested operation"))));
                assert!(result.is_err());
            })
            .into()
    });
    let frames = window.frames_presented();
    assert!(catch_unwind(AssertUnwindSafe(|| window.tap("nested"))).is_err());
    assert!(window.app().is_failed());
    assert_eq!(window.frames_presented(), frames);
    window.app().shutdown();
}

#[test]
fn refused_input_keeps_the_app_running_and_closed_windows_refuse_delivery() {
    let called = Rc::new(Cell::new(0));
    let capture = called.clone();
    let mut window = Window::headless(move || {
        let capture = capture.clone();
        el().id("accept").size(40).on_press(move || capture.set(capture.get() + 1)).into()
    });
    let app = window.app();
    assert!(window.dispatch(Event::Press("missing".into())).is_err());
    assert!(window.dispatch(Event::NumericInput("accept".into(), f64::NAN)).is_err());
    assert!(app.send("missing", "payload").is_err());
    assert!(!app.is_failed());
    window.dispatch(Event::Press("accept".into())).unwrap();
    assert_eq!(called.get(), 1);
    app.shutdown();
    assert!(window.tap("accept").is_err());
    assert!(window.press_key("tab").is_err());
    assert!(window.dispatch(Event::Press("accept".into())).is_err());
    assert_eq!(called.get(), 1);
    assert!(!app.is_failed());
}

#[test]
fn failed_host_submission_does_not_publish_a_completed_frame() {
    use octara::platform::{Frame, Platform, PlatformWindow};
    use std::any::Any;
    struct Host(Rc<Cell<bool>>);
    impl PlatformWindow for Host {
        fn present(&mut self, _: Frame) {
            assert!(!self.0.get(), "host submission failed");
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }
    struct System(Rc<Cell<bool>>);
    impl Platform for System {
        fn read_text(&mut self) -> Option<String> {
            None
        }
        fn write_text(&mut self, _: &str) {}
        fn open_window(&mut self, _: WindowId, _: &WindowOptions) -> Result<Box<dyn PlatformWindow>, String> {
            Ok(Box::new(Host(self.0.clone())))
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }
    let fail = Rc::new(Cell::new(false));
    let app = App::new(System(fail.clone()));
    let count = app.run(|| state("count", || 0));
    let window = app.open_window(WindowOptions::new("Publication"), move || text(move || count.get().to_string()).into()).unwrap();
    let frames = window.frames_presented();
    fail.set(true);
    assert!(catch_unwind(AssertUnwindSafe(|| app.run(|| count.set(1)))).is_err());
    assert_eq!(window.frames_presented(), frames, "a failed host handoff is not a completed presentation");
    assert!(app.is_failed());
    app.shutdown();
}

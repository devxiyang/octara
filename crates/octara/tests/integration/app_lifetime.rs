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
        catch_unwind(AssertUnwindSafe(|| app.run(|| state("value", || {
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

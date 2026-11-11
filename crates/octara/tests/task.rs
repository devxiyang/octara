//! Tasks, timers, channels, background work and resources: owned by scopes, on the app's
//! clock, landing in a deterministic order.

use octara::prelude::*;
use octara::task::{WorkKind, now};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn log_view(extra: impl Fn(Signal<Vec<String>>) -> View + 'static) -> impl Fn() -> View {
    move || {
        let log = state("log", Vec::<String>::new);
        col().child(text(move || log.get().join(",")).id("log")).child(extra(log)).into()
    }
}

fn log(app: &App) -> String {
    app.kernel().text_of("log").unwrap()
}

fn push(log: Signal<Vec<String>>, s: impl Into<String>) {
    let s = s.into();
    log.update(|l| l.push(s));
}

#[test]
fn a_task_runs_to_its_first_wait_then_follows_the_clock() {
    let mut app = App::new(log_view(|log| {
        spawn(async move {
            push(log, "start");
            sleep(ms(100)).await;
            push(log, format!("woke at {:?}", now()));
        });
        el().into()
    }));
    assert_eq!(log(&app), "start");
    assert_eq!(app.next_deadline(), Some(ms(100)));
    app.advance(ms(99));
    assert_eq!(log(&app), "start");
    app.advance(ms(5));
    assert_eq!(log(&app), "start,woke at 100ms", "the timer fires at its own time, not at the end of the step");
    assert_eq!(app.next_deadline(), None);
}

struct Dropped(Rc<Cell<bool>>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.set(true);
    }
}

#[test]
fn disposing_the_scope_cancels_its_tasks() {
    let dropped = Rc::new(Cell::new(false));
    let d = dropped.clone();
    let mut app = App::new(move || {
        let open = state("open", || true);
        let d = d.clone();
        col()
            .child(el().id("close").w(px(10.0)).h(px(10.0)).on_press(move || open.set(false)))
            .child(show(
                move || open.get(),
                move || {
                    let guard = Dropped(d.clone());
                    spawn(async move {
                        let _guard = guard;
                        sleep(ms(1000)).await;
                    });
                    text("panel").into()
                },
            ))
            .into()
    });
    assert_eq!(app.pending().len(), 1);
    assert!(app.pending()[0].scope.starts_with("app"), "{:?}", app.pending());
    app.tap("close").unwrap();
    assert!(dropped.get(), "the task (and what it holds) is dropped with its scope");
    assert!(app.pending().is_empty());
    assert_eq!(app.next_deadline(), None);
}

#[test]
fn a_detached_task_outlives_its_scope() {
    let mut app = App::new(log_view(|log| {
        let open = state("open", || true);
        col()
            .child(el().id("close").w(px(10.0)).h(px(10.0)).on_press(move || open.set(false)))
            .child(show(
                move || open.get(),
                move || {
                    spawn(async move {
                        sleep(ms(10)).await;
                        push(log, "still here");
                    })
                    .detach();
                    el().into()
                },
            ))
            .into()
    }));
    app.tap("close").unwrap();
    assert_eq!(app.pending()[0].scope, "detached");
    app.advance(ms(10));
    assert_eq!(log(&app), "still here");
}

#[test]
fn intervals_tick_on_the_clock_without_drift() {
    let mut app = App::new(log_view(|log| {
        interval(ms(30), move || push(log, format!("{:?}", now())));
        el().into()
    }));
    app.advance(ms(100));
    assert_eq!(log(&app), "30ms,60ms,90ms");
    assert_eq!(app.pending()[0].kind, WorkKind::Subscription);
    assert_eq!(app.pending()[0].due, Some(ms(120)));
    assert!(app.run_until_idle(ms(1000)).is_ok(), "a subscription is not unfinished work");
}

#[test]
fn values_from_other_threads_arrive_through_a_channel() {
    let (tx, rx) = channel::<u32>();
    let rx = std::cell::RefCell::new(Some(rx));
    let mut app = App::new(log_view(move |log| {
        if let Some(rx) = rx.borrow_mut().take() {
            rx.on(move |v| push(log, v.to_string()));
        }
        el().into()
    }));
    let t = std::thread::spawn(move || {
        for v in 1..=3 {
            tx.send(v).unwrap();
        }
    });
    t.join().unwrap();
    app.poll();
    assert_eq!(log(&app), "1,2,3");
}

#[test]
fn background_results_land_in_spawn_order() {
    let mut app = App::new(log_view(|log| {
        for (name, wait) in [("slow", 60u64), ("fast", 1)] {
            spawn(async move {
                let r = background(move || {
                    std::thread::sleep(Duration::from_millis(wait));
                    name
                })
                .await;
                push(log, r);
            });
        }
        el().into()
    }));
    assert!(app.pending().iter().any(|p| p.label.contains("background")));
    let left = app.settle();
    assert!(left.is_empty(), "{left:?}");
    assert_eq!(log(&app), "slow,fast", "woken tasks run in spawn order, whichever thread finished first");
}

#[test]
fn named_channels_take_text_from_tests_and_agents() {
    let mut app = App::new(log_view(|log| {
        channel::<u32>().1.named("count").on(move |n| push(log, format!("n={n}")));
        el().into()
    }));
    app.send("count", "7").unwrap();
    app.dispatch(Event::Send("count".into(), "8".into())).unwrap();
    assert_eq!(log(&app), "n=7,n=8");
    assert!(app.send("count", "seven").unwrap_err().contains("does not parse"));
    assert!(app.send("nope", "1").unwrap_err().contains("no channel named `nope`"));
}

fn search_app() -> App {
    App::new(|| {
        let query = state("query", || "a".to_string());
        let found = resource(
            move || query.get(),
            |q: String| async move {
                // A slower answer for "a" than for "ab": the older one would arrive last.
                sleep(ms(if q == "a" { 100 } else { 10 })).await;
                if q == "bad" { Err(format!("no results for {q}")) } else { Ok(format!("results for {q}")) }
            },
        );
        col()
            .child(text(move || found.value().unwrap_or_default()).id("value"))
            .child(text(move || if found.loading() { "loading" } else { "idle" }.to_string()).id("loading"))
            .child(text(move || found.error().unwrap_or_default()).id("error"))
            .child(el().id("refetch").w(px(10.0)).h(px(10.0)).on_press(move || found.refetch()))
            .child(text_input().id("q").w(px(100.0)).value(move || query.get()).on_input(move |v| query.set(v)))
            .into()
    })
}

fn texts(app: &App) -> (String, String, String) {
    let k = app.kernel();
    (k.text_of("value").unwrap(), k.text_of("loading").unwrap(), k.text_of("error").unwrap())
}

#[test]
fn a_resource_loads_by_key_and_an_old_answer_never_lands() {
    let mut app = search_app();
    assert_eq!(texts(&app), ("".into(), "loading".into(), "".into()));
    assert_eq!(app.pending()[0].label, "resource");
    // The key changes while "a" is still loading: "a" is cancelled, "ab" lands.
    app.advance(ms(5));
    app.dispatch(Event::Input("q".into(), "ab".into())).unwrap();
    app.advance(ms(200));
    assert_eq!(texts(&app), ("results for ab".into(), "idle".into(), "".into()));
    // Reloading keeps the last value; an error keeps it too.
    app.dispatch(Event::Input("q".into(), "bad".into())).unwrap();
    assert_eq!(texts(&app), ("results for ab".into(), "loading".into(), "".into()));
    assert!(app.run_until_idle(ms(1000)).is_ok());
    assert_eq!(texts(&app), ("results for ab".into(), "idle".into(), "no results for bad".into()));
    // Refetch loads the same key again; a success clears the error.
    app.dispatch(Event::Input("q".into(), "ok".into())).unwrap();
    app.tap("refetch").unwrap();
    assert!(app.run_until_idle(ms(1000)).is_ok());
    assert_eq!(texts(&app), ("results for ok".into(), "idle".into(), "".into()));
}

#[test]
fn run_until_idle_reports_work_still_running_at_its_limit() {
    let mut app = App::new(|| {
        spawn(async {
            sleep(ms(5000)).await;
        });
        el().into()
    });
    let left = app.run_until_idle(ms(1000)).unwrap_err();
    assert_eq!(left.len(), 1);
    assert_eq!((left[0].kind, left[0].due), (WorkKind::Task, Some(ms(5000))));
    assert!(left[0].at.contains("tests/task.rs"), "{:?}", left[0]);
    assert!(app.run_until_idle(ms(5000)).is_ok());
    assert_eq!(app.now(), ms(5000));
}

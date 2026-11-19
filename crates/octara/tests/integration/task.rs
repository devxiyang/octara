//! Tasks, timers, channels, background work and resources: owned by scopes, on the app's
//! clock, landing in a deterministic order.

use octara::app::Event;
use octara::prelude::*;
use octara::reactive::untrack;
use octara::task::{WorkKind, now};
use std::cell::{Cell, RefCell};
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

fn log(app: &Window) -> String {
    app.kernel().text_of("log").unwrap()
}

fn push(log: Signal<Vec<String>>, s: impl Into<String>) {
    let s = s.into();
    log.update(|l| l.push(s));
}

#[test]
fn a_task_runs_to_its_first_wait_then_follows_the_clock() {
    let app = Window::headless(log_view(|log| {
        spawn(async move {
            push(log, "start");
            sleep(ms(100)).await;
            push(log, format!("woke at {:?}", now()));
        });
        el().into()
    }));
    assert_eq!(log(&app), "start");
    assert_eq!(app.app().next_deadline(), Some(ms(100)));
    app.app().advance(ms(99));
    assert_eq!(log(&app), "start");
    app.app().advance(ms(5));
    assert_eq!(log(&app), "start,woke at 100ms", "the timer fires at its own time, not at the end of the step");
    assert_eq!(app.app().next_deadline(), None);
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
    let mut app = Window::headless(move || {
        let open = state("open", || true);
        let d = d.clone();
        col().child(el().id("close").w(px(10.0)).h(px(10.0)).on_press(move || open.set(false))).child(show(
            move || open.get(),
            move || {
                let guard = Dropped(d.clone());
                spawn(async move {
                    let _guard = guard;
                    sleep(ms(1000)).await;
                });
                text("panel")
            },
        ))
    });
    assert_eq!(app.app().pending().len(), 1);
    assert!(app.app().pending()[0].scope.starts_with("app/window"), "{:?}", app.app().pending());
    app.tap("close").unwrap();
    assert!(dropped.get(), "the task (and what it holds) is dropped with its scope");
    assert!(app.app().pending().is_empty());
    assert_eq!(app.app().next_deadline(), None);
}

#[test]
fn an_app_task_outlives_its_initiating_ui_scope() {
    let mut app = Window::headless(log_view(|log| {
        let open = state("open", || true);
        col()
            .child(el().id("close").w(px(10.0)).h(px(10.0)).on_press(move || open.set(false)))
            .child(show(
                move || open.get(),
                move || {
                    App::current().run(|| {
                        spawn(async move {
                            sleep(ms(10)).await;
                            push(log, "still here");
                        });
                    });
                    el()
                },
            ))
            .into()
    }));
    app.tap("close").unwrap();
    assert_eq!(app.app().pending()[0].scope, "app");
    app.app().advance(ms(10));
    assert_eq!(log(&app), "still here");
}

#[test]
fn intervals_tick_on_the_clock_without_drift() {
    let app = Window::headless(log_view(|log| {
        interval(ms(30), move || push(log, format!("{:?}", now())));
        el().into()
    }));
    app.app().advance(ms(100));
    assert_eq!(log(&app), "30ms,60ms,90ms");
    assert_eq!(app.app().pending()[0].kind, WorkKind::Subscription);
    assert_eq!(app.app().pending()[0].due, Some(ms(120)));
    assert!(app.app().run_until_idle(ms(1000)).is_ok(), "a subscription is not unfinished work");
}

#[test]
fn values_from_other_threads_arrive_through_a_channel() {
    let (tx, rx) = channel::<u32>();
    let rx = std::cell::RefCell::new(Some(rx));
    let app = Window::headless(log_view(move |log| {
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
    app.app().poll();
    assert_eq!(log(&app), "1,2,3");
}

#[test]
fn app_poll_drains_root_work_before_opening_and_after_closing_windows() {
    use octara::platform::{HeadlessPlatform, WindowOptions};
    let app = App::new(HeadlessPlatform::default());
    let delivered = Rc::new(RefCell::new(Vec::new()));
    let seen = delivered.clone();
    let send = app.run(|| {
        let count = state("count", || 0u32);
        let (send, receive) = channel();
        let (forward, forwarded) = channel();
        receive.on(move |value| count.set(value));
        watch(
            move || count.get(),
            move |&value, previous| {
                if previous.is_some() {
                    forward.send(value).unwrap();
                }
            },
        );
        forwarded.on(move |value| seen.borrow_mut().push(value));
        send
    });

    send.send(1).unwrap();
    app.poll();
    assert_eq!(*delivered.borrow(), [1]);

    let window = app.open_window(WindowOptions::default(), || el().size_full()).unwrap();
    app.close_window(window.id());
    assert!(app.windows().is_empty());

    send.send(2).unwrap();
    app.poll();
    assert_eq!(*delivered.borrow(), [1, 2]);
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
    app.shutdown();
}

#[test]
fn a_bursting_channel_yields_to_a_ready_cancellation() {
    const BURST: usize = 4_096;
    let delivered = Rc::new(Cell::new(0));
    let before_cancel = Rc::new(Cell::new(None));
    let (seen, cancelled_at) = (delivered.clone(), before_cancel.clone());
    let app = Window::headless(move || {
        let (send, receive) = channel::<usize>();
        let seen = seen.clone();
        let counted = seen.clone();
        let listener = receive.on(move |_| counted.set(counted.get() + 1));
        for value in 0..BURST {
            send.send(value).unwrap();
        }
        let cancelled_at = cancelled_at.clone();
        spawn(async move {
            cancelled_at.set(Some(seen.get()));
            listener.cancel();
        });
        el()
    });
    let at = before_cancel.get().expect("the other ready task ran");
    assert!(at > 0 && at < BURST, "the cancellation must run between bounded parts of the burst, received {at}/{BURST}");
    assert_eq!(delivered.get(), at, "the cancelled listener must not drain the rest");
    assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
}

#[test]
fn a_channel_burst_keeps_fifo_and_drains_its_closed_tail_across_yields() {
    let delivered = Rc::new(std::cell::RefCell::new(Vec::new()));
    let counted = delivered.clone();
    let app = Window::headless(move || {
        let (send, receive) = channel::<usize>();
        for value in 0..257 {
            send.send(value).unwrap();
        }
        drop(send);
        let counted = counted.clone();
        receive.on(move |value| counted.borrow_mut().push(value));
        el()
    });
    assert_eq!(*delivered.borrow(), (0..257).collect::<Vec<_>>());
    assert!(app.app().pending().is_empty(), "closed receiver finishes after delivering its tail");
    assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
}

#[test]
fn background_results_land_in_spawn_order() {
    let app = Window::headless(log_view(|log| {
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
    assert!(app.app().pending().iter().any(|p| p.label.contains("background")));
    let left = app.app().settle();
    assert!(left.is_empty(), "{left:?}");
    assert_eq!(log(&app), "slow,fast", "woken tasks run in spawn order, whichever thread finished first");
}

#[test]
fn named_channels_take_text_from_tests_and_agents() {
    let mut app = Window::headless(log_view(|log| {
        channel::<u32>().1.named("count").on(move |n| push(log, format!("n={n}")));
        el().into()
    }));
    app.app().send("count", "7").unwrap();
    app.dispatch(Event::Send("count".into(), "8".into())).unwrap();
    assert_eq!(log(&app), "n=7,n=8");
    assert!(app.app().send("count", "seven").unwrap_err().contains("does not parse"));
    assert!(app.app().send("nope", "1").unwrap_err().contains("no channel named `nope`"));
}

fn search_app() -> Window {
    Window::headless(|| {
        let query = state("query", || "a".to_string());
        let found = async_memo(
            move || query.get(),
            |q: String| async move {
                // A slower answer for "a" than for "ab": the older one would arrive last.
                sleep(ms(if q == "a" { 100 } else { 10 })).await;
                if q == "bad" { Err(format!("no results for {q}")) } else { Ok(format!("results for {q}")) }
            },
        );
        col()
            .child(text(move || found.with(|s| s.value().cloned()).unwrap_or_default()).id("value"))
            .child(text(move || if found.with(AsyncState::is_pending) { "loading" } else { "idle" }.to_string()).id("loading"))
            .child(text(move || found.with(|s| s.error().cloned()).unwrap_or_default()).id("error"))
            .child(el().id("refresh").w(px(10.0)).h(px(10.0)).on_press(move || found.refresh()))
            .child(input().id("q").w(px(100.0)).value(move || query.get()).on_input(move |v| query.set(v)))
    })
}

fn texts(app: &Window) -> (String, String, String) {
    let k = app.kernel();
    (k.text_of("value").unwrap(), k.text_of("loading").unwrap(), k.text_of("error").unwrap())
}

#[test]
fn an_async_memo_loads_by_key_and_an_old_answer_never_lands() {
    let mut app = search_app();
    assert_eq!(texts(&app), ("".into(), "loading".into(), "".into()));
    assert_eq!(app.app().pending()[0].label, "async_memo");
    // The key changes while "a" is still loading: "a" is cancelled, "ab" lands.
    app.app().advance(ms(5));
    app.dispatch(Event::Input("q".into(), "ab".into())).unwrap();
    app.app().advance(ms(200));
    assert_eq!(texts(&app), ("results for ab".into(), "idle".into(), "".into()));
    // Reloading keeps the last value; an error keeps it too.
    app.dispatch(Event::Input("q".into(), "bad".into())).unwrap();
    assert_eq!(texts(&app), ("results for ab".into(), "loading".into(), "".into()));
    assert!(app.app().run_until_idle(ms(1000)).is_ok());
    assert_eq!(texts(&app), ("results for ab".into(), "idle".into(), "no results for bad".into()));
    // Refetch loads the same key again; a success clears the error.
    app.dispatch(Event::Input("q".into(), "ok".into())).unwrap();
    app.tap("refresh").unwrap();
    assert!(app.app().run_until_idle(ms(1000)).is_ok());
    assert_eq!(texts(&app), ("results for ok".into(), "idle".into(), "".into()));
}

#[test]
fn run_until_idle_reports_work_still_running_at_its_limit() {
    let app = Window::headless(|| {
        spawn(async {
            sleep(ms(5000)).await;
        });
        el()
    });
    let left = app.app().run_until_idle(ms(1000)).unwrap_err();
    assert_eq!(left.len(), 1);
    assert_eq!((left[0].kind, left[0].due), (WorkKind::Task, Some(ms(5000))));
    assert!(left[0].at.contains("tests/integration/task.rs"), "{:?}", left[0]);
    assert!(app.app().run_until_idle(ms(5000)).is_ok());
    assert_eq!(app.app().now(), ms(5000));
}

#[test]
fn a_clock_steps_while_someone_reads_it() {
    use std::time::Duration;
    let mut w = Window::headless(|| {
        let shown = state("shown", || true);
        col()
            .child(show(
                move || shown.get(),
                || {
                    let t = clock(Duration::from_millis(100));
                    let u = clock(Duration::from_millis(100));
                    text(move || format!("{} {}", t.get().as_millis(), u.get().as_millis())).id("t")
                },
            ))
            .child(el().id("hide").size(10.0).on_press(move || shown.set(false)))
    });
    assert_eq!(w.kernel().text_of("t").unwrap(), "0 0");
    assert_eq!(w.app().pending().len(), 1, "both readers share one timer");
    assert_eq!(w.app().pending()[0].scope, "app");
    w.app().advance(Duration::from_millis(250));
    assert_eq!(w.kernel().text_of("t").unwrap(), "200 200", "in steps of its period");
    assert!(w.app().next_deadline().is_some());
    assert!(w.app().run_until_idle(Duration::from_secs(1)).is_ok(), "it does not keep the app busy");
    w.tap("hide").unwrap();
    assert_eq!(w.app().next_deadline(), None, "no reader left: the timer stopped");
    assert!(w.app().diagnostics().is_empty(), "{:?}", w.app().diagnostics());
}

#[test]
fn timers_at_fractional_milliseconds_come_due() {
    use std::time::Duration;
    let w = Window::headless(|| {
        let n = state("n", || 0u32);
        spawn(async move {
            for _ in 0..50 {
                sleep(Duration::from_micros(16_667)).await;
                n.update(|n| *n += 1);
            }
        });
        text(move || n.get().to_string()).id("n")
    });
    assert!(w.app().run_until_idle(Duration::from_secs(5)).is_ok(), "{:?}", w.app().pending());
    assert_eq!(w.kernel().text_of("n").unwrap(), "50", "advancing to each deadline fires its timer");
}

#[test]
fn a_handler_restores_its_component_context_and_owns_spawned_tasks() {
    let observed = Rc::new(Cell::new(None));
    let dropped = Rc::new(Cell::new(false));
    let (seen, guard) = (observed.clone(), dropped.clone());
    let mut app = Window::headless(move || {
        let open = state("open", || true);
        let (seen, guard) = (seen.clone(), guard.clone());
        col().child(el().id("hide").size(10.0).on_press(move || open.set(false))).child(show(
            move || open.get(),
            move || {
                provide(42u32);
                let (seen, guard) = (seen.clone(), guard.clone());
                el().id("start").size(10.0).on_press(move || {
                    seen.set(use_context::<u32>());
                    let guard = Dropped(guard.clone());
                    spawn(async move {
                        let _guard = guard;
                        sleep(ms(1000)).await;
                    });
                })
            },
        ))
    });
    app.tap("start").unwrap();
    assert_eq!(observed.get(), Some(42), "callbacks see the scope captured when mounted");
    assert!(app.app().pending()[0].scope.starts_with("app/window"));
    app.tap("hide").unwrap();
    assert!(dropped.get(), "a task started by the handler belongs to its mounted scope");
    assert!(app.app().pending().is_empty());
    assert_eq!(app.app().next_deadline(), None);
    assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
}

#[test]
fn cancelling_a_task_withdraws_its_pending_request() {
    use octara::ask::Answer;
    use octara::platform::{HeadlessPlatform, HeadlessWindow};
    use std::cell::RefCell;
    for native in [false, true] {
        let running = Rc::new(RefCell::new(None::<octara::task::Task>));
        let task = running.clone();
        let platform = HeadlessPlatform::with_window(HeadlessWindow { shows_asks: native, ..Default::default() });
        let mut app = Window::headless_on(platform, move || {
            let (start, cancel) = (task.clone(), task.clone());
            on_command("cancel-task", move || cancel.borrow().as_ref().unwrap().cancel());
            col().child(el().id("ask").size(10.0).on_press(move || {
                *start.borrow_mut() = Some(spawn(async {
                    prompt(PromptLevel::Info, "Continue?", None, &["Yes"]).await;
                }));
            }))
        });
        app.tap("ask").unwrap();
        let id = app.asks()[0].0;
        // Cancellation is an App command; the prompt correctly blocks its background UI.
        app.invoke("cancel-task").unwrap();
        assert!(app.asks().is_empty(), "dropping the awaiting future withdraws its request (native={native})");
        assert!(app.answer(id, Answer::Prompt(Some(0))).is_err(), "late answers are stale");
        assert!(!app.kernel().exists("prompt:0"));
        assert!(app.app().pending().is_empty());
        if native {
            let p = app.platform_window();
            let p = p.as_any().downcast_ref::<HeadlessWindow>().unwrap();
            assert_eq!(p.withdrawn, [id]);
        }
        assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
    }
}

#[test]
fn a_panicking_task_fails_the_app_without_running_ready_siblings() {
    let ran = Rc::new(Cell::new(false));
    let seen = ran.clone();
    let mut app = Window::headless(move || {
        let seen = seen.clone();
        el().id("start").size(10.0).on_press(move || {
            spawn(async { panic!("task poll failed") });
            let seen = seen.clone();
            spawn(async move { seen.set(true) });
        })
    });
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.tap("start")));
    assert!(result.is_err());
    assert!(app.app().is_failed());
    assert_eq!(app.app().pending().iter().filter(|work| work.kind == WorkKind::Task).count(), 1);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.app().poll())).is_err());
    assert!(!ran.get(), "pending siblings cannot execute after the App fails");
    assert!(app.app().diagnostics().iter().any(|d| d.code == "APP_FAILED"));
    assert!(!app.unsettled().is_empty());
    app.app().shutdown();
    assert!(app.app().pending().is_empty());
}

#[test]
fn disposing_a_scope_retires_requests_even_when_the_future_is_retained() {
    use octara::ask::Answer;
    use std::cell::RefCell;
    use std::future::Future;
    use std::pin::Pin;
    type RequestFuture = Pin<Box<dyn Future<Output = Option<Vec<std::path::PathBuf>>>>>;
    let requests = Rc::new(RefCell::new(Vec::<RequestFuture>::new()));
    let retained = requests.clone();
    let mut app = Window::headless(move || {
        let open = state("open", || true);
        let retained = retained.clone();
        col().child(el().id("toggle").size(10.0).on_press(move || open.update(|open| *open = !*open))).child(show(
            move || open.get(),
            move || {
                retained.borrow_mut().push(Box::pin(prompt_for_paths(Default::default())));
                text("scope")
            },
        ))
    });
    let old = app.asks()[0].0;
    app.tap("toggle").unwrap();
    assert!(app.asks().is_empty(), "the future cannot retain a disposed request owner");
    app.tap("toggle").unwrap();
    let new = app.asks()[0].0;
    assert_ne!(old, new);
    assert!(app.answer(old, Answer::Paths(None)).is_err());
    app.answer(new, Answer::Paths(None)).unwrap();
    requests.borrow_mut().clear();
    assert!(app.asks().is_empty());
    assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
}

#[test]
fn dropping_a_request_before_presentation_does_not_show_it() {
    use octara::platform::{HeadlessPlatform, HeadlessWindow};
    let platform = HeadlessPlatform::with_window(HeadlessWindow { shows_asks: true, ..Default::default() });
    let mut app = Window::headless_on(platform, || {
        el().id("ask").size(10.0).on_press(|| {
            drop(prompt_for_paths(Default::default()));
        })
    });
    app.tap("ask").unwrap();
    assert!(app.asks().is_empty());
    let p = app.platform_window();
    let p = p.as_any().downcast_ref::<HeadlessWindow>().unwrap();
    assert!(p.asks.is_empty(), "a cancelled unpresented request never reaches the platform");
    assert!(p.withdrawn.is_empty(), "only presented requests need a withdrawal");
}

#[test]
fn pure_computations_cannot_start_tasks_or_requests_through_untrack() {
    let ran = Rc::new(Cell::new(false));
    let finished = Rc::new(Cell::new(false));
    let (seen, rejected) = (ran.clone(), finished.clone());
    let mut app = Window::headless(move || {
        let (seen, rejected) = (seen.clone(), rejected.clone());
        let value = memo(move || {
            let seen = seen.clone();
            let command_seen = seen.clone();
            let task = untrack(|| spawn(async move { seen.set(true) }));
            rejected.set(task.is_finished());
            untrack(|| drop(prompt_for_paths(Default::default())));
            untrack(|| on_command("forbidden", move || command_seen.set(true)));
            1
        });
        text(move || value.get().to_string())
    });
    assert!(!ran.get());
    assert!(finished.get());
    assert!(app.asks().is_empty());
    assert!(app.app().pending().is_empty());
    assert!(app.invoke("forbidden").is_err());
    assert!(!ran.get());
    assert!(app.app().diagnostics().iter().any(|d| d.code == "WRITE_IN_DERIVE"));
}

#[test]
fn closing_a_window_cancels_its_waiting_requests() {
    use octara::platform::{HeadlessPlatform, WindowOptions};
    let app = App::new(HeadlessPlatform::default());
    let window = app
        .open_window(WindowOptions::new("request owner"), || {
            spawn(async {
                prompt(PromptLevel::Info, "Continue?", None, &["Yes"]).await;
            });
            el().size_full()
        })
        .unwrap();
    assert_eq!(window.asks().len(), 1);
    app.close_window(window.id());
    assert!(window.asks().is_empty());
    assert!(app.pending().is_empty());
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
}

#[test]
fn an_async_memo_rechecks_its_input_after_polling_user_code() {
    let app = App::new(octara::platform::HeadlessPlatform::default());
    let (key, found) = app.run(|| {
        let key = state("key", || 1u32);
        let found = async_memo(
            move || key.get(),
            move |requested| async move {
                if requested == 1 {
                    key.set(2);
                    Ok::<_, String>("obsolete")
                } else {
                    std::future::pending().await
                }
            },
        );
        (key, found)
    });
    app.poll();
    app.run(|| {
        assert_eq!(key.get(), 2);
        assert_eq!(
            found.with(|s| s.value().cloned()),
            None,
            "a result whose input changed during poll must never become the retained value"
        );
        assert!(found.with(AsyncState::is_pending));
    });
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
    app.shutdown();
    assert!(app.pending().is_empty());
}

#[test]
fn an_async_memo_refreshed_during_its_poll_cannot_publish_the_previous_attempt() {
    let app = App::new(octara::platform::HeadlessPlatform::default());
    let slot = Rc::new(Cell::new(None::<octara::task::AsyncMemo<u32, String>>));
    let attempts = Rc::new(Cell::new(0));
    let found = app.run(|| {
        let (slot_for_load, attempts) = (slot.clone(), attempts.clone());
        let found = async_memo(
            || (),
            move |_| {
                let (slot, attempts) = (slot_for_load.clone(), attempts.clone());
                async move {
                    attempts.set(attempts.get() + 1);
                    if attempts.get() == 1 {
                        slot.get().unwrap().refresh();
                        Ok(1)
                    } else {
                        std::future::pending().await
                    }
                }
            },
        );
        slot.set(Some(found));
        found
    });
    app.poll();
    app.run(|| {
        assert_eq!(attempts.get(), 2);
        assert_eq!(found.with(|s| s.value().cloned()), None);
        assert!(found.with(AsyncState::is_pending));
    });
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
    app.shutdown();
    assert!(app.pending().is_empty());
}

#[test]
fn polling_inside_an_async_memo_read_preserves_its_ready_result() {
    let app = App::new(octara::platform::HeadlessPlatform::default());
    let (send, found) = app.run(|| {
        let (send, receive) = channel::<u32>();
        let receive = RefCell::new(Some(receive));
        let found = async_memo(
            || (),
            move |_| {
                let mut receive = receive.borrow_mut().take().unwrap();
                async move { Ok::<_, ()>(receive.recv().await.unwrap()) }
            },
        );
        (send, found)
    });
    app.poll();
    send.send(7).unwrap();

    app.run(|| {
        found.with(|state| {
            assert!(state.is_pending());
            app.poll();
        });
    });
    app.run(|| assert_eq!(found.get(), AsyncState::Ready(7)));
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
    app.shutdown();
}

#[test]
fn async_memo_replacement_retires_each_attempt_even_when_the_key_returns() {
    use std::cell::RefCell;
    struct Attempt(Rc<Cell<usize>>);
    impl Drop for Attempt {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    let app = App::new(octara::platform::HeadlessPlatform::default());
    let senders = Rc::new(RefCell::new(Vec::new()));
    let dropped = Rc::new(Cell::new(0));
    let (key, found) = app.run(|| {
        let key = state("key", || "a");
        let (senders, dropped) = (senders.clone(), dropped.clone());
        let found = async_memo(
            move || key.get(),
            move |_| {
                let (tx, mut rx) = channel::<u32>();
                senders.borrow_mut().push(tx);
                let guard = Attempt(dropped.clone());
                async move {
                    let _guard = guard;
                    Ok::<_, String>(rx.recv().await.unwrap())
                }
            },
        );
        (key, found)
    });
    app.run(|| key.set("b"));
    app.run(|| key.set("a"));
    app.run(|| found.refresh());
    assert_eq!(senders.borrow().len(), 4);
    assert_eq!(dropped.get(), 3);
    for sender in &senders.borrow()[..3] {
        assert!(sender.send(99).is_err(), "retired receivers reject late results");
    }
    senders.borrow()[3].send(4).unwrap();
    app.poll();
    app.run(|| {
        assert_eq!(found.with(|s| s.value().cloned()), Some(4));
        assert!(!found.with(AsyncState::is_pending));
        found.refresh();
    });
    assert_eq!(dropped.get(), 4);
    assert_eq!(app.pending().len(), 1);
    app.shutdown();
    assert_eq!(dropped.get(), 5);
    assert!(senders.borrow()[4].send(5).is_err());
    assert!(app.pending().is_empty());
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
}

#[test]
fn an_async_memo_retired_during_poll_does_not_touch_disposed_state() {
    let app = App::new(octara::platform::HeadlessPlatform::default());
    let dropped = Rc::new(Cell::new(false));
    app.run(|| {
        let dropped = dropped.clone();
        async_memo(
            || (),
            move |_| {
                let guard = Dropped(dropped.clone());
                async move {
                    let _guard = guard;
                    App::current().shutdown();
                    Ok::<_, String>(1)
                }
            },
        );
    });
    app.poll();
    assert!(dropped.get());
    assert!(app.pending().is_empty());
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
}

#[test]
fn async_memo_borrows_non_clone_results_and_clears_errors_when_refreshed() {
    struct Value(u32);
    struct Failure;
    let app = App::new(octara::platform::HeadlessPlatform::default());
    let calls = Rc::new(Cell::new(0));
    let (input, incidental, derived) = app.run(|| {
        let input = state("input", || 7u32);
        let incidental = state("incidental", || 0u32);
        let calls = calls.clone();
        let derived = async_memo(
            move || input.get(),
            move |key| {
                let _sample = incidental.get();
                calls.set(calls.get() + 1);
                let attempt = calls.get();
                async move {
                    let _sample = incidental.get();
                    if attempt == 2 { Err(Failure) } else { Ok(Value(key)) }
                }
            },
        );
        assert!(derived.with(AsyncState::is_pending));
        assert!(derived.with(|s| s.value().is_none()));
        (input, incidental, derived)
    });
    app.poll();
    app.run(|| {
        assert!(derived.with(|s| matches!(s, AsyncState::Ready(Value(7)))));
        input.set(7);
        incidental.set(1);
    });
    assert_eq!(calls.get(), 1, "equal input and incidental reads do not restart work");
    app.run(|| derived.refresh());
    app.run(|| assert!(derived.with(|s| matches!(s, AsyncState::Pending { previous: Some(Value(7)) }))));
    app.poll();
    app.run(|| {
        assert!(derived.with(|s| matches!(s, AsyncState::Failed { error: Failure, previous: Some(Value(7)) })));
        assert!(derived.with(|s| s.error().is_some()));
        derived.refresh();
    });
    app.run(|| {
        assert!(derived.with(AsyncState::is_pending));
        assert!(derived.with(|s| s.error().is_none()), "retry clears the previous error");
        assert_eq!(derived.with(|s| s.value().map(|v| v.0)), Some(7));
    });
    app.poll();
    app.run(|| assert!(derived.with(|s| matches!(s, AsyncState::Ready(Value(7))))));
    assert_eq!(calls.get(), 3);
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
    app.shutdown();
}

#[test]
fn handlers_and_tasks_replace_only_their_registered_scope_context() {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let out = seen.clone();
    let mut window = Window::headless(move || {
        provide(1u32);
        let root = out.clone();
        let out = out.clone();
        col().child(el().id("root-context").size(10.0).on_press(move || root.borrow_mut().push(use_context::<u32>().unwrap()))).child(show(
            || true,
            move || {
                provide(2u32);
                let captured = use_context::<u32>().unwrap();
                let out = out.clone();
                el().id("replace-context").size(10.0).on_press(move || {
                    provide(3u32);
                    out.borrow_mut().extend([captured, use_context::<u32>().unwrap()]);
                    let out = out.clone();
                    spawn(async move {
                        provide(4u32);
                        out.borrow_mut().push(use_context::<u32>().unwrap());
                    });
                })
            },
        ))
    });
    window.tap("replace-context").unwrap();
    window.app().poll();
    window.tap("root-context").unwrap();
    assert_eq!(*seen.borrow(), [2, 3, 4, 1]);
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
}

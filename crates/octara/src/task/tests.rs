use super::test_support::{DroppedResult, WakeCounter};
use super::*;
use crate::app::App;
use crate::platform::{HeadlessPlatform, WindowOptions};
use crate::reactive::state;
use crate::view::{Styled, el};
use std::sync::atomic::AtomicUsize;

#[test]
fn stopping_a_watch_keeps_its_tasks_until_the_owning_scope_retires() {
    let app = App::new(HeadlessPlatform::default());
    let tasks = Rc::new(RefCell::new(Vec::new()));
    let interests = Rc::new(RefCell::new(Vec::new()));
    let (source, child, observer) = app.run(|| {
        let source = Signal::scoped(0);
        let child = rt(|r| r.create_scope(r.current_scope(), "watch tasks"));
        let (tasks, interests) = (tasks.clone(), interests.clone());
        let observer = rt(|r| {
            r.with_scope(Some(child), || {
                watch(
                    move || source.get(),
                    move |_, _| {
                        let capture = Rc::new(());
                        interests.borrow_mut().push(Rc::downgrade(&capture));
                        let task = spawn(async move {
                            let _capture = capture;
                            std::future::pending::<()>().await;
                        });
                        tasks.borrow_mut().push(task);
                    },
                )
            })
        });
        (source, child, observer)
    });
    app.poll();
    app.run(|| source.set(1));
    app.poll();
    assert_eq!(tasks.borrow().len(), 2, "rerunning the watch does not replace prior work");
    app.run(|| observer.stop());
    app.run(|| source.set(2));
    app.poll();
    assert_eq!(tasks.borrow().len(), 2);
    assert!(tasks.borrow().iter().all(|task| !task.is_finished()));
    assert!(interests.borrow().iter().all(|interest| interest.upgrade().is_some()));
    app.run(|| rt(|r| r.dispose_scope(child)));
    assert!(tasks.borrow().iter().all(Task::is_finished));
    assert!(interests.borrow().iter().all(|interest| interest.upgrade().is_none()));
    assert!(app.pending().is_empty());
    assert!(app.diagnostics().is_empty());
    app.shutdown();
}

#[test]
fn cleanup_panic_stops_tasks_and_later_shutdown_resumes_unstarted_cleanup() {
    let app = App::new(HeadlessPlatform::default());
    let state_value = Rc::new(());
    let retained_state = Rc::downgrade(&state_value);
    let task_value = Rc::new(());
    let retained_task = Rc::downgrade(&task_value);
    let skipped = Rc::new(Cell::new(false));
    let out = skipped.clone();
    let task = app.run(|| {
        let _state = Signal::scoped(state_value);
        let task = spawn(async move {
            let _value = task_value;
            std::future::pending::<()>().await;
        });
        rt(|r| {
            r.on_cleanup(Box::new(move || out.set(true)));
            r.on_cleanup(Box::new(|| std::panic::panic_any(73_u32)));
        });
        task
    });
    app.poll();
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.shutdown())).unwrap_err();
    assert_eq!(failure.downcast_ref::<u32>(), Some(&73));
    assert!(app.is_failed() && app.is_shutdown());
    assert!(task.is_finished());
    assert!(retained_task.upgrade().is_none(), "tasks are cancelled before user cleanup");
    assert!(!skipped.get(), "remaining user cleanups are not called while unwinding");
    assert!(retained_state.upgrade().is_some(), "interrupted node teardown can retain state");
    assert!(app.pending().iter().any(|work| work.label.contains("teardown is incomplete")));
    app.shutdown();
    assert!(skipped.get(), "only unstarted cleanup resumes");
    assert!(retained_state.upgrade().is_none(), "shutdown releases state while the App handle stays alive");
    assert!(app.pending().is_empty());
    drop(app);
    assert!(retained_state.upgrade().is_none(), "the runtime still owns and ultimately drops its values");
}

#[test]
fn shutdown_after_branch_cleanup_panic_finishes_the_retired_scope() {
    let app = App::new(HeadlessPlatform::default());
    let value = Rc::new(());
    let retained = Rc::downgrade(&value);
    let child = app.run(|| {
        rt(|r| {
            let child = r.create_scope(r.current_scope(), "failing branch");
            r.with_scope(Some(child), || {
                let _value = Signal::scoped(value);
                r.on_cleanup(Box::new(|| panic!("branch cleanup failed")));
            });
            child
        })
    });
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        app.run(|| rt(|r| r.dispose_scope(child)));
    }));
    assert!(failure.is_err());
    assert!(app.is_failed() && !app.is_shutdown());
    app.shutdown();
    assert!(app.is_shutdown());
    assert!(retained.upgrade().is_none(), "retired scopes remain reachable until release finishes");
    drop(app);
    assert!(retained.upgrade().is_none());
}

#[test]
fn shutdown_resumes_after_a_task_destructor_panics() {
    struct FailsOnDrop;
    impl Drop for FailsOnDrop {
        fn drop(&mut self) {
            panic!("task drop failed");
        }
    }
    let app = App::new(HeadlessPlatform::default());
    let retained = Rc::new(());
    let weak = Rc::downgrade(&retained);
    let (first, second) = app.run(|| {
        let guard = FailsOnDrop;
        let first = spawn(async move {
            let _guard = guard;
            std::future::pending::<()>().await;
        });
        let second = spawn(async move {
            let _retained = retained;
            std::future::pending::<()>().await;
        });
        (first, second)
    });
    app.poll();
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.shutdown())).is_err());
    assert!(first.is_finished());
    assert!(!second.is_finished());
    assert!(weak.upgrade().is_some());
    assert!(app.pending().iter().any(|work| work.label.contains("teardown is incomplete")));
    app.shutdown();
    assert!(second.is_finished());
    assert!(weak.upgrade().is_none());
    assert!(app.pending().is_empty());
}

#[test]
fn interrupted_window_teardown_and_reload_keep_resources_reachable() {
    for reload in [false, true] {
        let app = App::new(HeadlessPlatform::default());
        let probes = Rc::new(RefCell::new(Vec::new()));
        let cleaned = Rc::new(Cell::new(0));
        let mut windows = Vec::new();
        for fail in [true, false] {
            let (probes, cleaned) = (probes.clone(), cleaned.clone());
            windows.push(
                app.open_window(Default::default(), move || {
                    let value = Rc::new(());
                    probes.borrow_mut().push(Rc::downgrade(&value));
                    crate::reactive::graph::state("retained", || value.clone());
                    crate::reactive::provide(value);
                    let cleaned = cleaned.clone();
                    crate::reactive::on_cleanup(move || {
                        cleaned.set(cleaned.get() + 1);
                        if fail {
                            panic!("window cleanup failed");
                        }
                    });
                    el().size_full().into()
                })
                .unwrap(),
            );
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if reload {
                app.hot_reload();
            } else {
                app.shutdown();
            }
        }));
        assert!(result.is_err());
        assert!(app.is_failed());
        assert_eq!(cleaned.get(), 1);
        app.shutdown();
        assert_eq!(cleaned.get(), 2, "each cleanup is attempted only once");
        assert!(windows.iter().all(crate::app::Window::is_closed));
        assert!(app.windows().is_empty());
        assert!(probes.borrow().iter().all(|probe| probe.upgrade().is_none()), "named state and contexts release with handles retained");
        assert!(app.pending().is_empty());
        app.shutdown();
        assert_eq!(cleaned.get(), 2);
    }
}

#[test]
fn large_ready_batch_yields_and_requests_continuation() {
    let app = App::new(HeadlessPlatform::default());
    let completed = Rc::new(Cell::new(0));
    let scheduler = app.run(|| {
        for _ in 0..TASK_POLLS_PER_TURN + 1 {
            let completed = completed.clone();
            spawn(async move { completed.set(completed.get() + 1) });
        }
        sched()
    });
    let wakes = Arc::new(AtomicUsize::new(0));
    let notified = wakes.clone();
    scheduler.set_waker(Arc::new(move || {
        notified.fetch_add(1, Ordering::Relaxed);
    }));
    app.poll();
    assert_eq!(completed.get(), TASK_POLLS_PER_TURN);
    assert_eq!(wakes.load(Ordering::Relaxed), 1, "finite work must request another host turn");
    assert!(app.pending().iter().any(|work| work.kind == WorkKind::Update));
    app.poll();
    assert_eq!(completed.get(), TASK_POLLS_PER_TURN + 1);
    assert!(app.pending().is_empty());
    app.shutdown();
}

#[test]
fn deferred_batch_precedes_self_wakes_and_shutdown_retires_it() {
    let app = App::new(HeadlessPlatform::default());
    let finite_ran = Rc::new(Cell::new(false));
    let polls = Rc::new(Cell::new(0));
    let scheduler = app.run(|| {
        for _ in 0..TASK_POLLS_PER_TURN {
            let polls = polls.clone();
            spawn(std::future::poll_fn(move |cx| {
                polls.set(polls.get() + 1);
                cx.waker().wake_by_ref();
                Poll::<()>::Pending
            }));
        }
        let finite = finite_ran.clone();
        spawn(async move { finite.set(true) });
        sched()
    });
    app.poll();
    assert_eq!(polls.get(), TASK_POLLS_PER_TURN);
    assert!(!finite_ran.get());
    app.poll();
    assert!(finite_ran.get(), "old batch must finish before self-wakes enter a new batch");
    assert_eq!(polls.get(), TASK_POLLS_PER_TURN * 2 - 1);
    app.shutdown();
    assert!(!scheduler.has_ready());
    assert!(app.pending().is_empty());
}

#[test]
fn repeated_wakes_are_one_poll_and_nested_drains_share_the_allowance() {
    let app = App::new(HeadlessPlatform::default());
    let count = Rc::new(Cell::new(0));
    let out = count.clone();
    let nested = app.clone();
    let task = app.run(|| {
        spawn(std::future::poll_fn(move |cx| {
            out.set(out.get() + 1);
            for _ in 0..8 {
                cx.waker().wake_by_ref();
            }
            nested.poll();
            Poll::<()>::Pending
        }))
    });
    app.poll();
    assert_eq!(count.get(), TASK_POLLS_PER_TURN);
    task.cancel();
    app.poll();
    assert_eq!(count.get(), TASK_POLLS_PER_TURN);
    assert!(app.pending().is_empty());
    app.shutdown();
}

#[test]
fn windows_share_one_turn_and_remain_incomplete_while_tasks_yield() {
    let app = App::new(HeadlessPlatform::default());
    let first = app.open_window(Default::default(), || el().size_full().into()).unwrap();
    let second = app.open_window(Default::default(), || el().size_full().into()).unwrap();
    let polls = Rc::new(Cell::new(0));
    let out = polls.clone();
    let task = app.run(|| {
        spawn(std::future::poll_fn(move |cx| {
            out.set(out.get() + 1);
            cx.waker().wake_by_ref();
            Poll::<()>::Pending
        }))
    });
    assert_eq!(polls.get(), TASK_POLLS_PER_TURN, "App::run also shares one allowance");
    polls.set(0);
    app.poll();
    assert_eq!(polls.get(), TASK_POLLS_PER_TURN, "windows must not multiply the allowance");
    assert!(app.pending().iter().any(|work| work.kind == WorkKind::Update));
    assert!(!first.is_closed() && !second.is_closed());
    assert!(!first.unsettled().is_empty() && !second.unsettled().is_empty());
    polls.set(0);
    app.advance(Duration::from_millis(1));
    assert_eq!(polls.get(), TASK_POLLS_PER_TURN, "clock advancement shares the same boundary");
    task.cancel();
    app.poll();
    assert!(app.pending().is_empty(), "cancelled work must not leave windows incomplete");
    app.shutdown();
}

#[test]
fn nested_app_poll_preserves_the_active_tasks_wake() {
    let app = App::new(HeadlessPlatform::default());
    let nested = app.clone();
    let polls = Rc::new(Cell::new(0));
    let count = polls.clone();
    app.run(|| {
        spawn(std::future::poll_fn(move |cx| {
            count.set(count.get() + 1);
            if count.get() == 1 {
                cx.waker().wake_by_ref();
                nested.poll();
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        }));
    });
    app.poll();
    assert_eq!(polls.get(), 2, "nested polling must leave the wake for the outer scheduler");
    assert!(app.pending().is_empty());
    app.shutdown();
}

#[test]
fn reused_slots_do_not_reorder_ready_tasks() {
    let app = App::new(HeadlessPlatform::default());
    let log = Rc::new(RefCell::new(Vec::new()));
    let wakes = Rc::new(RefCell::new(Vec::new()));
    let ready = Rc::new(Cell::new(false));
    app.run(|| {
        // Reusing slot zero must not make the next task younger than a fresh slot.
        for _ in 0..4 {
            spawn(std::future::pending()).cancel();
        }
        for name in ["older", "newer"] {
            let (log, wakes, ready) = (log.clone(), wakes.clone(), ready.clone());
            spawn(std::future::poll_fn(move |cx| {
                if ready.get() {
                    log.borrow_mut().push(name);
                    Poll::Ready(())
                } else {
                    wakes.borrow_mut().push((name, cx.waker().clone()));
                    Poll::Pending
                }
            }));
        }
    });
    app.poll();
    ready.set(true);
    let wakes = wakes.borrow();
    for name in ["newer", "older"] {
        let waker = &wakes.iter().find(|(n, _)| *n == name).unwrap().1;
        waker.wake_by_ref();
        waker.wake_by_ref();
    }
    app.poll();
    assert_eq!(*log.borrow(), ["older", "newer"], "arrival order and slot generation must not override spawn order");
    assert!(app.pending().is_empty());
    app.shutdown();
}

#[test]
fn creation_order_keeps_older_cancellation_ahead_of_newer_work_after_slot_reuse() {
    let app = App::new(HeadlessPlatform::default());
    let younger = Rc::new(RefCell::new(None::<Task>));
    let ran = Rc::new(Cell::new(false));
    app.run(|| {
        for _ in 0..4 {
            spawn(std::future::pending()).cancel();
        }
        let target = younger.clone();
        spawn(async move {
            target.borrow().as_ref().unwrap().cancel();
        });
        let ran = ran.clone();
        *younger.borrow_mut() = Some(spawn(async move {
            ran.set(true);
        }));
    });
    app.poll();
    assert!(!ran.get(), "the earlier cancellation must win even when its slot was reused");
    assert!(app.pending().is_empty());
    app.shutdown();
}

#[test]
fn a_retired_waker_cannot_poll_the_task_reusing_its_slot() {
    let app = App::new(HeadlessPlatform::default());
    let saved = Rc::new(RefCell::new(None::<Waker>));
    let capture = saved.clone();
    let old = app.run(|| {
        spawn(std::future::poll_fn(move |cx| {
            *capture.borrow_mut() = Some(cx.waker().clone());
            Poll::<()>::Pending
        }))
    });
    app.poll();
    old.cancel();
    let polls = Rc::new(Cell::new(0));
    let capture = polls.clone();
    let replacement = app.run(|| {
        spawn(std::future::poll_fn(move |_| {
            capture.set(capture.get() + 1);
            Poll::<()>::Pending
        }))
    });
    assert_eq!(old.id.unwrap().slot(), replacement.id.unwrap().slot());
    app.poll();
    assert_eq!(polls.get(), 1);
    saved.borrow().as_ref().unwrap().wake_by_ref();
    app.poll();
    assert_eq!(polls.get(), 1, "filter stale identity before looking up creation order");
    app.shutdown();
}

#[test]
fn a_sleep_has_one_timer_and_cancellation_removes_it_immediately() {
    let app = App::new(HeadlessPlatform::default());
    let (task, scheduler) = app.run(|| {
        let mut wait = Box::pin(sleep(Duration::from_secs(3600)));
        let mut polls = 0;
        let task = spawn(std::future::poll_fn(move |cx| {
            assert!(wait.as_mut().poll(cx).is_pending());
            polls += 1;
            if polls < 32 {
                cx.waker().wake_by_ref();
            }
            Poll::<()>::Pending
        }));
        (task, sched())
    });
    app.poll();
    assert_eq!(scheduler.timers.borrow().len(), 1);
    task.cancel();
    assert!(scheduler.timers.borrow().is_empty());
    assert!(app.next_deadline().is_none());
    app.shutdown();
}

#[test]
fn dropping_one_wait_preserves_other_waits_of_the_same_task() {
    let app = App::new(HeadlessPlatform::default());
    let completed = Rc::new(Cell::new(false));
    let out = completed.clone();
    let scheduler = app.run(|| {
        spawn(async move {
            let mut early = Box::pin(sleep(Duration::from_millis(10)));
            let mut later = Box::pin(sleep(Duration::from_millis(20)));
            std::future::poll_fn(|cx| {
                assert!(early.as_mut().poll(cx).is_pending());
                assert!(later.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            drop(early);
            later.await;
            out.set(true);
        });
        sched()
    });
    app.poll();
    assert_eq!(scheduler.timers.borrow().len(), 1);
    assert_eq!(app.next_deadline(), Some(Duration::from_millis(20)));
    app.advance(Duration::from_millis(10));
    assert!(!completed.get());
    app.advance(Duration::from_millis(10));
    assert!(completed.get());
    assert!(scheduler.timers.borrow().is_empty());
    app.shutdown();
}

#[test]
fn zero_periods_are_rejected_before_registration() {
    let app = App::new(HeadlessPlatform::default());
    app.run(|| {
        let scheduler = sched();
        for operation in [0, 1] {
            let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if operation == 0 {
                    interval(Duration::ZERO, || panic!("must not execute"));
                } else {
                    crate::view::clock(Duration::ZERO);
                }
            }));
            assert!(failure.is_err());
        }
        assert!(scheduler.pending().is_empty());
        rt(|r| assert_eq!(r.cleanup_count(r.current_scope().unwrap()), 0));
    });
    assert!(!app.is_failed(), "validation was caught before crossing the App boundary");
    assert!(app.diagnostics().iter().any(|d| d.code == "INVALID_INTERVAL"));
    app.shutdown();
}

#[test]
fn a_sleep_rejects_a_different_apps_clock() {
    let first = App::new(HeadlessPlatform::default());
    let second = App::new(HeadlessPlatform::default());
    let wait = first.run(|| sleep(Duration::from_millis(10)));
    second.advance(Duration::from_secs(1));
    second.run(|| {
        spawn(wait);
    });
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| second.poll())).is_err());
    assert!(second.is_failed());
    assert!(!first.is_failed());
    first.shutdown();
    second.shutdown();
}

#[test]
fn dropping_a_background_wait_releases_its_waker_before_the_worker_finishes() {
    let app = App::new(HeadlessPlatform::default());
    let wake = Arc::new(WakeCounter(AtomicUsize::new(0)));
    let waker = Waker::from(wake.clone());
    let dropped = Arc::new(AtomicUsize::new(0));
    let output = dropped.clone();
    let (release, wait) = std::sync::mpsc::channel();
    let (finished, discarded) = std::sync::mpsc::channel::<()>();
    let (mut future, scheduler) = app.run(|| {
        (
            Box::pin(background(move || {
                wait.recv().unwrap();
                (DroppedResult(output), finished)
            })),
            sched(),
        )
    });
    assert!(future.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
    drop(waker);
    assert_eq!(Arc::strong_count(&wake), 2, "a pending receiver retains its task waker");
    drop(future);
    assert_eq!(Arc::strong_count(&wake), 1, "the blocked producer must not retain cancelled interest");
    release.send(()).unwrap();
    assert!(scheduler.wait_background(Duration::from_secs(5)));
    assert!(
        matches!(discarded.recv_timeout(Duration::from_secs(5)), Err(std::sync::mpsc::RecvTimeoutError::Disconnected)),
        "the unclaimed result must be dropped after completion accounting"
    );
    assert_eq!(wake.0.load(Ordering::Relaxed), 0, "late completion does not wake a retired receiver");
    assert_eq!(dropped.load(Ordering::Relaxed), 1);
    assert!(app.pending().is_empty());
    assert!(!app.is_failed());
    app.shutdown();
}

#[test]
fn a_retained_platform_reply_does_not_retain_a_cancelled_receiver() {
    let app = App::new(HeadlessPlatform::default());
    let shared = app.run(|| Arc::downgrade(&sched().shared));
    let (reply, future) = app.run(later::<DroppedResult>);
    let mut future = Box::pin(future);
    let wake = Arc::new(WakeCounter(AtomicUsize::new(0)));
    let waker = Waker::from(wake.clone());
    assert!(future.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
    drop(waker);
    assert_eq!(Arc::strong_count(&wake), 2);
    drop(future);
    assert_eq!(Arc::strong_count(&wake), 1, "an unanswered platform callback does not pin the task waker");
    app.shutdown();
    drop(app);
    assert!(shared.upgrade().is_none(), "a retained platform callback must not prolong the retired scheduler");
    let dropped = Arc::new(AtomicUsize::new(0));
    reply(DroppedResult(dropped.clone()));
    assert_eq!(dropped.load(Ordering::Relaxed), 1);
    assert_eq!(wake.0.load(Ordering::Relaxed), 0);
}

#[test]
fn background_panics_reach_the_owner_without_exhausting_the_shared_pool() {
    for attempt in 0..6 {
        let app = App::new(HeadlessPlatform::default());
        let (job, scheduler) = app.run(|| (background(move || std::panic::panic_any(attempt)), sched()));
        assert!(scheduler.wait_background(Duration::from_secs(5)), "a panic must retire inflight work");
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            app.run(|| {
                spawn(job);
            });
            app.poll();
        }))
        .expect_err("the awaiting task must observe the worker panic");
        assert_eq!(failure.downcast_ref::<i32>(), Some(&attempt), "preserve the original payload");
        assert!(app.is_failed());
        assert!(app.pending().iter().all(|work| work.kind == WorkKind::Update), "only App failure remains pending");
        app.shutdown();
        assert!(app.pending().is_empty());
    }

    let app = App::new(HeadlessPlatform::default());
    let result = Rc::new(Cell::new(None));
    let out = result.clone();
    let scheduler = app.run(|| {
        spawn(async move {
            out.set(Some(background(|| Err::<u32, _>("expected failure")).await));
        });
        sched()
    });
    app.poll();
    assert!(scheduler.wait_background(Duration::from_secs(5)));
    app.poll();
    assert_eq!(result.get(), Some(Err("expected failure")));
    assert!(!app.is_failed(), "business errors remain ordinary values");
    app.shutdown();
}

#[test]
fn cancelled_background_panics_retire_without_resuming_the_cancelled_task() {
    let app = App::new(HeadlessPlatform::default());
    let continued = Rc::new(Cell::new(false));
    let out = continued.clone();
    let (release, wait) = std::sync::mpsc::channel();
    let scheduler = app.run(|| {
        let task = spawn(async move {
            background(move || {
                wait.recv().unwrap();
                panic!("cancelled work failed");
            })
            .await;
            out.set(true);
        });
        // Run until the worker has been submitted and the task is waiting on its result.
        let scheduler = sched();
        scheduler.run_ready();
        task.cancel();
        scheduler
    });
    release.send(()).unwrap();
    assert!(scheduler.wait_background(Duration::from_secs(5)));
    app.poll();
    assert!(!continued.get());
    assert!(!app.is_failed());
    assert!(app.pending().is_empty());
    app.shutdown();
}

#[test]
fn finished_and_cancelled_tasks_release_scope_registrations() {
    let app = App::new(HeadlessPlatform::default());
    for _ in 0..256 {
        app.run(|| {
            let task = spawn(std::future::pending());
            rt(|r| assert_eq!(r.cleanup_count(r.current_scope().unwrap()), 1));
            task.cancel();
            rt(|r| assert_eq!(r.cleanup_count(r.current_scope().unwrap()), 0));
            spawn(async {});
        });
        app.poll();
        app.run(|| rt(|r| assert_eq!(r.cleanup_count(r.current_scope().unwrap()), 0)));
    }
    assert!(app.pending().is_empty());
    app.shutdown();
}

#[test]
fn shutdown_retires_the_current_poll_and_ignores_late_wakes() {
    struct Dropped(Rc<Cell<bool>>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }

    let runtime = crate::reactive::graph::Runtime::new();
    runtime.enter(|| {
        let scope = runtime.create_scope(None, "app");
        let scheduler = Rc::new(Scheduler::default());
        let wakes = Arc::new(AtomicUsize::new(0));
        let notified = wakes.clone();
        scheduler.set_waker(Arc::new(move || {
            notified.fetch_add(1, Ordering::Relaxed);
        }));
        let late_waker = Rc::new(RefCell::new(None::<Waker>));
        let current_dropped = Rc::new(Cell::new(false));
        let sibling_ran = Rc::new(Cell::new(false));
        let (closing, saved, dropped) = (scheduler.clone(), late_waker.clone(), current_dropped.clone());
        let guard = Dropped(current_dropped.clone());
        let current = scheduler.insert(
            Box::pin(async move {
                let _guard = guard;
                std::future::poll_fn(move |cx| {
                    *saved.borrow_mut() = Some(cx.waker().clone());
                    closing.shutdown();
                    assert!(!dropped.get(), "a future is retained until its active poll returns");
                    cx.waker().wake_by_ref();
                    Poll::<()>::Pending
                })
                .await;
            }),
            scope,
            WorkKind::Task,
            "current".into(),
            Location::caller(),
        );
        let sibling = sibling_ran.clone();
        scheduler.insert(Box::pin(async move { sibling.set(true) }), scope, WorkKind::Task, "sibling".into(), Location::caller());
        scheduler.add_timer(100.0, current, Waker::noop().clone());
        let interest = Arc::new(NamedInterest { scheduler: Arc::downgrade(&scheduler.shared) });
        scheduler
            .names
            .borrow_mut()
            .insert("queued".into(), NamedChannel { inject: Rc::new(|_| Ok(())), receiver: Arc::downgrade(&interest), _cleanup: None });

        scheduler.run_ready();

        assert!(current_dropped.get());
        assert!(!sibling_ran.get());
        assert_eq!(scheduler.current.get(), None);
        assert!(scheduler.slots.borrow().iter().all(Option::is_none));
        assert!(scheduler.timers.borrow().is_empty());
        assert!(scheduler.names.borrow().is_empty());
        late_waker.borrow().as_ref().unwrap().wake_by_ref();
        scheduler.shutdown();
        assert!(!scheduler.run_ready());
        assert!(!scheduler.has_ready());
        assert_eq!(wakes.load(Ordering::Relaxed), 0);
    });
}

#[test]
fn shutdown_rejects_new_background_work_without_dispatching_it() {
    let app = App::new(HeadlessPlatform::default());
    let ran = Arc::new(AtomicBool::new(false));
    let called = ran.clone();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        app.run(|| {
            sched().shutdown();
            drop(background(move || {
                called.store(true, Ordering::Release);
            }));
        });
    }));
    assert!(result.is_err());
    assert!(!ran.load(Ordering::Acquire));
    assert!(app.pending().iter().all(|work| work.kind == WorkKind::Update), "only App failure is pending, not a background job");
    assert!(app.is_failed());
    app.shutdown();
    assert!(app.diagnostics().iter().any(|diagnostic| diagnostic.code == "WRITE_DISPOSED"));
}

#[test]
fn an_app_task_has_no_window_owner_and_outlives_the_last_window() {
    let app = App::new(HeadlessPlatform::default());
    let shared = app.run(|| state("shared", || 0));
    let owner = Rc::new(Cell::new(Some(crate::reactive::graph::Owner(0))));
    let seen = owner.clone();
    let window = app
        .open_window(WindowOptions::new("temporary"), move || {
            let seen = seen.clone();
            App::current().run(|| {
                spawn(async move {
                    sleep(Duration::from_millis(10)).await;
                    seen.set(crate::reactive::graph::current_owner());
                    shared.set(1);
                });
            });
            el().size_full().into()
        })
        .unwrap();
    assert_eq!(app.pending()[0].due, Some(Duration::from_millis(10)), "the task reached its first wait");
    assert_eq!(app.pending()[0].scope, "app");
    app.close_window(window.id());
    assert_eq!(app.pending().len(), 1, "closing the last window preserves the app task");
    app.advance(Duration::from_millis(10));
    assert_eq!(owner.get(), None, "app tasks run without a current window");
    assert_eq!(app.run(|| shared.get()), 1);
    assert!(app.pending().is_empty());
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
}

#[test]
fn a_completed_ui_tasks_cleanup_does_not_cancel_a_reused_app_task() {
    let app = App::new(HeadlessPlatform::default());
    let window = app
        .open_window(WindowOptions::new("temporary"), || {
            spawn(async {});
            el().size_full().into()
        })
        .unwrap();
    assert!(app.pending().is_empty());
    let finished = Rc::new(Cell::new(false));
    let done = finished.clone();
    app.run(|| {
        spawn(async move {
            sleep(Duration::from_millis(10)).await;
            done.set(true);
        });
    });

    app.close_window(window.id());

    assert_eq!(app.pending()[0].scope, "app");
    app.advance(Duration::from_millis(10));
    assert!(finished.get());
    assert!(app.pending().is_empty());
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
}

#[test]
fn exhausted_task_generation_does_not_alias_an_old_handle() {
    let app = App::new(HeadlessPlatform::default());
    let (old, scheduler) = app.run(|| {
        let task = spawn(std::future::pending());
        task.cancel();
        (task, sched())
    });
    scheduler.generations.borrow_mut()[old.id.unwrap().slot()] = u32::MAX;
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.run(|| spawn(std::future::pending())))).is_err());
    assert!(old.is_finished());
    assert!(app.is_failed());
    app.shutdown();
}

#[test]
fn replacing_host_notification_releases_captures_outside_the_lock() {
    struct Capture(Weak<Shared>, Arc<AtomicBool>);
    impl Drop for Capture {
        fn drop(&mut self) {
            let shared = self.0.upgrade().unwrap();
            assert!(shared.wake.try_lock().is_ok(), "host callback capture dropped under wake lock");
            self.1.store(true, Ordering::Release);
        }
    }
    let scheduler = Scheduler::default();
    let dropped = Arc::new(AtomicBool::new(false));
    let capture = Capture(Arc::downgrade(&scheduler.shared), dropped.clone());
    scheduler.set_waker(Arc::new(move || {
        let _ = &capture;
    }));
    scheduler.set_waker(Arc::new(|| {}));
    assert!(dropped.load(Ordering::Acquire));
    scheduler.shutdown();
}

fn wait_for_completion_failure(scheduler: &Scheduler) {
    let until = std::time::Instant::now() + Duration::from_secs(5);
    while scheduler.shared.failures.lock().unwrap().is_empty() {
        assert!(std::time::Instant::now() < until, "completion failure did not arrive");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn completion_wake_panics_fail_the_owner_without_exhausting_workers() {
    struct PanicWake(u32);
    impl Wake for PanicWake {
        fn wake(self: Arc<Self>) {
            std::panic::panic_any(self.0);
        }
    }
    for attempt in 0u32..6 {
        let app = App::new(HeadlessPlatform::default());
        let (release, wait) = std::sync::mpsc::channel();
        let (mut future, scheduler) = app.run(|| {
            (
                Box::pin(background(move || {
                    wait.recv().unwrap();
                    42
                })),
                sched(),
            )
        });
        let waker = Waker::from(Arc::new(PanicWake(attempt)));
        assert!(future.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
        release.send(()).unwrap();
        wait_for_completion_failure(&scheduler);
        assert!(scheduler.wait_background(Duration::ZERO));
        assert!(!scheduler.idle());
        assert!(app.pending().iter().any(|work| work.kind == WorkKind::Update));
        let entered = Cell::new(false);
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.run(|| entered.set(true)))).unwrap_err();
        assert_eq!(panic.downcast_ref::<u32>(), Some(&attempt));
        assert!(!entered.get());
        assert!(app.is_failed());
        drop(future);
        app.shutdown();
    }
}

#[test]
fn cancelled_result_destructor_failure_retires_work_and_reaches_the_owner() {
    struct BadDrop;
    impl Drop for BadDrop {
        fn drop(&mut self) {
            panic!("unclaimed result destructor");
        }
    }
    let app = App::new(HeadlessPlatform::default());
    let (release, wait) = std::sync::mpsc::channel();
    let (future, scheduler) = app.run(|| {
        (
            background(move || {
                wait.recv().unwrap();
                BadDrop
            }),
            sched(),
        )
    });
    drop(future);
    release.send(()).unwrap();
    wait_for_completion_failure(&scheduler);
    assert!(scheduler.wait_background(Duration::ZERO));
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.poll())).is_err());
    assert!(app.is_failed());
    app.shutdown();
}

#[test]
fn native_reply_notification_failure_uses_the_same_app_boundary() {
    struct PanicWake;
    impl Wake for PanicWake {
        fn wake(self: Arc<Self>) {
            panic!("native notification");
        }
    }
    let app = App::new(HeadlessPlatform::default());
    let (reply, future) = app.run(later::<u32>);
    let mut future = Box::pin(future);
    assert!(future.as_mut().poll(&mut Context::from_waker(&Waker::from(Arc::new(PanicWake)))).is_pending());
    reply(42); // External callback returns; the owning App receives the failure.
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.poll())).is_err());
    assert!(app.is_failed());
    drop(future);
    app.shutdown();
}

#[test]
fn shutdown_can_retire_an_unobserved_completion_failure() {
    let app = App::new(HeadlessPlatform::default());
    let scheduler = app.run(sched);
    scheduler.shared.complete(|| panic!("completion before shutdown"));
    app.shutdown();
    assert!(app.is_shutdown());
    assert!(app.pending().is_empty());
    assert!(scheduler.shared.failures.lock().unwrap().is_empty());
}

use std::cell::{Cell, RefCell};
use std::future::{Future, pending, poll_fn};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

use octara_runtime::app::AppCommands;
use octara_runtime::arena::ReactiveWorld;

#[cfg(not(target_family = "wasm"))]
use octara_runtime::task::channel;
use octara_runtime::task::{AsyncState, TaskCx, TaskScheduler, VirtualClock};

#[test]
fn task_writes_borrow_locals_and_close_between_polls() {
    let mut world = ReactiveWorld::new();
    let mut commands = AppCommands::default();
    let mut tasks = TaskScheduler::new();
    let scope = world.root_scope();
    let signal = world.build(scope, None).signal(0);
    let retained = Rc::new(RefCell::new(None::<TaskCx>));
    let task_handle = retained.clone();

    tasks.spawn(scope, None, "task", move |task| async move {
        *task_handle.borrow_mut() = Some(task.clone());
        let mut local = 0;
        task.write(|write| {
            assert!(task.write(|_| ()).is_none());
            local += write.get(signal) + 1;
            write.set(signal, local);
        });
        task.sleep(Duration::from_nanos(1)).await;

        let panic = catch_unwind(AssertUnwindSafe(|| task.write(|_| panic!("task write"))));
        assert!(panic.is_err());
        task.write(|write| write.set(signal, local + 1));
    });

    assert_eq!(tasks.poll_ready(&mut world, &mut commands), 1);
    assert_eq!(world.read().get(signal), 1);
    let retained = retained.borrow().clone().unwrap();
    assert!(retained.write(|_| ()).is_none());
    tasks.advance_to(Duration::from_nanos(1));
    assert_eq!(tasks.poll_ready(&mut world, &mut commands), 1);
    assert_eq!(world.read().get(signal), 2);
    assert!(retained.write(|_| ()).is_none());
    assert!(tasks.pending().is_empty());
}

#[test]
fn retiring_a_scope_cancels_its_moveable_wait_and_stale_waker() {
    let mut world = ReactiveWorld::new();
    let mut commands = AppCommands::default();
    let mut tasks = TaskScheduler::new();
    let parent = world.new_scope(world.root_scope());
    let child = world.new_scope(parent);
    let stored_wait = Rc::new(RefCell::new(None));
    let stored_waker = Rc::new(RefCell::new(None::<Waker>));
    tasks.spawn(child, None, "cancelled before poll", |_| async { panic!("retired task polled") });
    tasks.cancel_scope(child);

    // Keep its future pending after moving the registered Sleep out.
    let wait = stored_wait.clone();
    let waker = stored_waker.clone();
    tasks.spawn(child, None, "pending child", move |task| {
        let mut sleep = Some(task.sleep(Duration::from_secs(1)));
        poll_fn(move |cx| {
            let mut sleep = sleep.take().expect("poll once");
            assert!(Pin::new(&mut sleep).poll(cx).is_pending());
            *wait.borrow_mut() = Some(sleep);
            *waker.borrow_mut() = Some(cx.waker().clone());
            Poll::Pending
        })
    });
    assert_eq!(tasks.poll_ready(&mut world, &mut commands), 1);
    assert_eq!(tasks.next_deadline(), Some(Duration::from_secs(1)));

    world.dispose(parent);
    tasks.cancel_retired_scopes(&world);
    assert!(tasks.pending().is_empty());
    assert!(tasks.next_deadline().is_none());
    stored_waker.borrow().as_ref().unwrap().wake_by_ref();
    assert!(!tasks.has_ready());
}

#[derive(Default)]
struct WakeCounter(AtomicUsize);

impl Wake for WakeCounter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn virtual_timers_keep_the_original_schedule_and_notify_the_latest_waker() {
    let clock = VirtualClock::new();
    let period = Duration::from_micros(16_667);
    let mut interval = clock.interval(period);
    let first = Arc::new(WakeCounter::default());
    let latest = Arc::new(WakeCounter::default());
    let first_waker = Waker::from(first.clone());
    let latest_waker = Waker::from(latest.clone());

    {
        let tick = std::pin::pin!(interval.tick());
        assert!(tick.poll(&mut Context::from_waker(&first_waker)).is_pending());
    }
    assert!(clock.next_deadline().is_none());

    {
        let mut tick = std::pin::pin!(interval.tick());
        assert!(tick.as_mut().poll(&mut Context::from_waker(&first_waker)).is_pending());
        assert!(tick.as_mut().poll(&mut Context::from_waker(&latest_waker)).is_pending());
        assert_eq!(clock.next_deadline(), Some(period));
        clock.advance_to(period * 3);
        assert_eq!(tick.poll(&mut Context::from_waker(&latest_waker)), Poll::Ready(period));
    }
    assert_eq!(first.0.load(Ordering::Relaxed), 0);
    assert_eq!(latest.0.load(Ordering::Relaxed), 1);

    for expected in [period * 2, period * 3] {
        let tick = std::pin::pin!(interval.tick());
        assert_eq!(tick.poll(&mut Context::from_waker(&latest_waker)), Poll::Ready(expected));
    }
}

#[test]
#[cfg(not(target_family = "wasm"))]
fn queued_wakes_from_other_threads_deliver_channel_values() {
    let mut world = ReactiveWorld::new();
    let mut commands = AppCommands::default();
    let mut tasks = TaskScheduler::new();
    let scope = world.root_scope();
    let finished = world.build(scope, None).signal(Vec::<u32>::new());
    let (send, mut receive) = channel();
    tasks.spawn(scope, None, "channel", move |task| async move {
        let mut values = Vec::new();
        while let Some(value) = task.recv(&mut receive).await {
            values.push(value);
        }
        task.write(|write| write.set(finished, values));
    });
    assert_eq!(tasks.poll_ready(&mut world, &mut commands), 1);
    assert!(!tasks.has_ready());

    std::thread::spawn(move || {
        send.send(1).unwrap();
        send.send(2).unwrap();
    })
    .join()
    .unwrap();
    assert!(tasks.has_ready());
    assert_eq!(tasks.poll_ready(&mut world, &mut commands), 1);
    assert_eq!(world.read().get(finished), [1, 2]);
}

#[test]
#[cfg(not(target_family = "wasm"))]
fn background_work_wakes_the_host_and_publishes_on_the_task_thread() {
    let mut world = ReactiveWorld::new();
    let mut commands = AppCommands::default();
    let mut tasks = TaskScheduler::new();
    let scope = world.root_scope();
    let signal = world.build(scope, None).signal(0);
    let (start, wait) = std::sync::mpsc::channel();
    let (wake, host) = std::sync::mpsc::channel();
    tasks.set_wake(Some(Arc::new(move || wake.send(()).unwrap())));
    let task_thread = std::thread::current().id();
    tasks.spawn(scope, None, "background", move |task| async move {
        let value = task
            .background(move || {
                wait.recv().unwrap();
                assert_ne!(std::thread::current().id(), task_thread);
                7
            })
            .await;
        assert_eq!(std::thread::current().id(), task_thread);
        task.write(|write| write.set(signal, value));
    });
    assert_eq!(host.try_recv(), Ok(()));
    assert_eq!(tasks.poll_ready(&mut world, &mut commands), 1);
    assert!(!tasks.has_ready());
    start.send(()).unwrap();
    assert_eq!(host.recv_timeout(Duration::from_secs(5)), Ok(()));
    assert_eq!(tasks.poll_ready(&mut world, &mut commands), 1);
    assert_eq!(world.read().get(signal), 7);
}

#[test]
fn a_large_ready_batch_does_not_starve_its_later_tasks() {
    let mut world = ReactiveWorld::new();
    let mut commands = AppCommands::default();
    let mut tasks = TaskScheduler::new();
    let scope = world.root_scope();
    for _ in 0..256 {
        tasks.spawn(scope, None, "self waking", |_| {
            poll_fn(|cx| {
                cx.waker().wake_by_ref();
                Poll::Pending
            })
        });
    }
    let ran_last = Rc::new(Cell::new(false));
    let result = ran_last.clone();
    tasks.spawn(scope, None, "last", |_| async move { result.set(true) });

    assert_eq!(tasks.poll_ready(&mut world, &mut commands), 256);
    assert!(!ran_last.get());
    assert_eq!(tasks.poll_ready(&mut world, &mut commands), 1);
    assert!(ran_last.get());
}

#[test]
fn async_memo_rejects_a_result_if_its_input_changes_in_the_final_poll() {
    let mut world = ReactiveWorld::new();
    let mut commands = AppCommands::default();
    let mut tasks = TaskScheduler::new();
    let scope = world.root_scope();
    let mut build = world.build(scope, None).with_tasks(tasks.spawner());
    let input = build.signal(1);
    let memo = build.async_memo(
        move |read| read.get(input),
        move |key, task| async move {
            match key {
                1 => {
                    task.write(|write| write.set(input, 2));
                    Ok::<_, ()>("stale")
                }
                2 => pending().await,
                _ => Ok("fresh"),
            }
        },
    );

    world.flush_watches();
    world.run_watch_effects_with(&mut commands, &tasks.spawner());
    assert_eq!(tasks.poll_ready(&mut world, &mut commands), 1);
    assert_eq!(world.read().get(memo), AsyncState::Pending { previous: None });

    world.flush_watches();
    world.run_watch_effects_with(&mut commands, &tasks.spawner());
    assert_eq!(tasks.poll_ready(&mut world, &mut commands), 1);
    world.write(scope, None).set(input, 3);
    world.flush_watches();
    world.run_watch_effects_with(&mut commands, &tasks.spawner());
    assert_eq!(tasks.poll_ready(&mut world, &mut commands), 1);
    assert_eq!(world.read().get(memo), AsyncState::Ready("fresh"));
    assert!(tasks.pending().is_empty());
}

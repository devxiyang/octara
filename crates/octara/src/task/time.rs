use super::*;

// ---- time ----

/// The app's clock: time since it started, as the shell (or a test) advances it.
pub fn now() -> Duration {
    Duration::from_secs_f64(sched().now_ms() / 1000.0)
}

/// Completes on the creating App's clock. A pending wait must be polled by one of that
/// App's tasks; dropping it retires its timer.
pub struct Sleep {
    due: f64,
    scheduler: std::rc::Weak<Scheduler>,
    registration: Option<u64>,
}

impl Sleep {
    fn unregister(&mut self) {
        if let Some(registration) = self.registration.take()
            && let Some(scheduler) = self.scheduler.upgrade()
        {
            let retired: Vec<_> = scheduler.timers.borrow_mut().extract_if(.., |timer| timer.1 == registration).collect();
            drop(retired);
        }
    }
}

impl Drop for Sleep {
    fn drop(&mut self) {
        self.unregister();
    }
}

impl Future for Sleep {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let s = sched();
        if !std::rc::Weak::ptr_eq(&self.scheduler, &Rc::downgrade(&s)) {
            rt(|r| r.diag(&codes::WRONG_RUNTIME, "Sleep belongs to another App".into(), Location::caller()));
            panic!("[{}] Sleep belongs to another App", codes::WRONG_RUNTIME);
        }
        if s.now_ms() >= self.due {
            self.unregister();
            return Poll::Ready(());
        }
        let id = s.current.get().expect("Sleep must be polled by an App task");
        if s.shared.closed.load(Ordering::Acquire) || !s.alive(id) {
            self.unregister();
            return Poll::Pending;
        }
        let waker = cx.waker().clone();
        if let Some(registration) = self.registration {
            // A wait may move between tasks in the same App; retain its single timer.
            let mut timers = s.timers.borrow_mut();
            if let Some(timer) = timers.iter_mut().find(|timer| timer.1 == registration) {
                timer.2 = id;
                let old = std::mem::replace(&mut timer.3, waker);
                drop(timers);
                drop(old);
                return Poll::Pending;
            }
        }
        self.registration = Some(s.add_timer(self.due, id, waker));
        Poll::Pending
    }
}

/// Waits `d` on the creating App's clock. Cancelling the wait removes its timer.
pub fn sleep(d: Duration) -> Sleep {
    let s = sched();
    Sleep { due: whole_ns(s.now_ms() + d.as_secs_f64() * 1000.0), scheduler: Rc::downgrade(&s), registration: None }
}

/// Waits until the creating App's clock reads `t`.
pub fn sleep_until(t: Duration) -> Sleep {
    Sleep { due: whole_ns(t.as_secs_f64() * 1000.0), scheduler: Rc::downgrade(&sched()), registration: None }
}

#[track_caller]
pub(crate) fn require_period(period: Duration) {
    if period.is_zero() {
        let at = Location::caller();
        rt(|r| r.diag(&codes::INVALID_INTERVAL, "period must be greater than zero".into(), at));
        panic!("[{}] period must be greater than zero", codes::INVALID_INTERVAL);
    }
}

/// A time in milliseconds rounded to the clock's resolution, the nanosecond (`Duration`'s): the
/// clock and every due time go through it, so a due time handed out as a `Duration` and moved
/// to is reached exactly.
pub(crate) fn whole_ns(ms: f64) -> f64 {
    (ms * 1e6).round() / 1e6
}

/// Calls `f` every `period` on the app's clock while the current scope lives (ticks are counted
/// from now, so they do not drift).
///
/// # Panics
/// A zero period is rejected with INVALID_INTERVAL before registering work.
#[track_caller]
pub fn interval(period: Duration, f: impl Fn() + 'static) -> Task {
    require_period(period);
    let start = now();
    let fut = async move {
        let mut due = start + period;
        loop {
            sleep_until(due).await;
            f();
            due += period;
        }
    };
    spawn_with(fut, WorkKind::Subscription, format!("interval {period:?}"), Location::caller())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::platform::HeadlessPlatform;

    struct ChildWake {
        ready: Arc<AtomicBool>,
        parent: Waker,
    }
    impl Wake for ChildWake {
        fn wake(self: Arc<Self>) {
            self.ready.store(true, Ordering::Release);
            self.parent.wake_by_ref();
        }
    }

    #[test]
    fn sleep_notifies_the_waker_of_the_future_that_polled_it() {
        let app = App::new(HeadlessPlatform::default());
        let done = Rc::new(Cell::new(false));
        let out = done.clone();
        let obsolete_ready = Arc::new(AtomicBool::new(false));
        let obsolete = obsolete_ready.clone();
        app.run(|| {
            let mut wait = Box::pin(sleep(Duration::from_millis(10)));
            let ready = Arc::new(AtomicBool::new(true));
            let mut registered = false;
            spawn(std::future::poll_fn(move |cx| {
                if !ready.swap(false, Ordering::AcqRel) {
                    return Poll::Pending;
                }
                if !registered {
                    let old = Waker::from(Arc::new(ChildWake { ready: obsolete.clone(), parent: cx.waker().clone() }));
                    assert!(wait.as_mut().poll(&mut Context::from_waker(&old)).is_pending());
                    registered = true;
                }
                let child = Waker::from(Arc::new(ChildWake { ready: ready.clone(), parent: cx.waker().clone() }));
                if wait.as_mut().poll(&mut Context::from_waker(&child)).is_ready() {
                    out.set(true);
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            }));
        });
        app.poll();
        app.advance(Duration::from_millis(10));
        assert!(done.get(), "a child future must receive its wake before its parent can poll it again");
        assert!(!obsolete_ready.load(Ordering::Acquire), "only the latest polling waker is notified");
        app.shutdown();
    }
}

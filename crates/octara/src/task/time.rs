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
            scheduler.timers.borrow_mut().retain(|timer| timer.1 != registration);
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
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
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
        if let Some(registration) = self.registration {
            // A wait may move between tasks in the same App; retain its single timer.
            let mut timers = s.timers.borrow_mut();
            if let Some(timer) = timers.iter_mut().find(|timer| timer.1 == registration) {
                timer.2 = id;
                return Poll::Pending;
            }
        }
        self.registration = Some(s.add_timer(self.due, id));
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

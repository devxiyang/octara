//! The app's clock and its pending work: time moves only when the shell (with real time) or a
//! test (`advance`) moves it; timers, long presses and momentum act on it. Tests and agents land
//! asynchronous work with `settle` and run it out with `run_until_idle`.

use super::App;
use crate::task::{PendingWork, WorkKind};
use std::time::Duration;

/// A frame while something moves (60 Hz; shells present on their display's own cadence).
const MOTION_FRAME_MS: f64 = 1000.0 / 60.0;

/// How long `settle` waits for background jobs (real time) before reporting them pending.
const BACKGROUND_WAIT: Duration = Duration::from_secs(10);

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}
fn duration(ms: f64) -> Duration {
    Duration::from_secs_f64(ms / 1000.0)
}

impl App {
    /// The app's clock: time since it started, as the shell (or a test) advances it.
    pub fn now(&self) -> Duration {
        duration(self.ui.sched.now_ms())
    }

    /// Moves the clock forward by `dt`: timers and long presses due on the way fire in order,
    /// momentum glides.
    pub fn advance(&mut self, dt: Duration) {
        let to = self.now() + dt;
        self.advance_to(to);
    }

    /// Moves the clock to `t` (shells call this before each event and frame with the time since
    /// launch). Whatever comes due on the way runs at its own time. Time never goes back.
    pub fn advance_to(&mut self, t: Duration) {
        let sched = self.ui.sched.clone();
        let t = ms(t).max(sched.now_ms());
        for _ in 0..100_000 {
            let next = [sched.next_timer(), self.long_press_due()].into_iter().flatten().min_by(f64::total_cmp);
            let Some(due) = next.filter(|d| *d <= t) else { break };
            self.set_clock(due.max(sched.now_ms()));
            self.input_frame(|| {
                sched.fire_due();
                self.pointer_time();
            });
        }
        self.set_clock(t);
        let glides = self.ui.kernel.borrow().gliding_scrolls();
        // Hover held while layout moved catches up once it has settled.
        let catch_up = self.hover_held() && !self.ui.kernel.borrow().layout_in_motion();
        if self.gliding() || glides || catch_up {
            self.input_frame(|| {
                self.pointer_time();
                self.ui.kernel.borrow_mut().step_glides();
            });
        }
    }

    /// When the app next needs the clock advanced (a timer, a long press coming due, the next
    /// frame of momentum or of motion), if anything is pending. Shells wake up then.
    pub fn next_deadline(&self) -> Option<Duration> {
        let frame = self.ui.kernel.borrow_mut().in_motion().then(|| self.ui.sched.now_ms() + MOTION_FRAME_MS);
        [self.ui.sched.next_timer(), self.pointer_deadline(), frame].into_iter().flatten().min_by(f64::total_cmp).map(duration)
    }

    /// Moves the clock: the scheduler's and the time the kernel shows.
    fn set_clock(&self, ms: f64) {
        self.ui.sched.set_now_ms(ms);
        self.ui.kernel.borrow_mut().set_time(ms / 1000.0);
    }

    /// Reduce Motion (the system setting): positions and sizes land at once; opacity and
    /// colors fade briefly instead of moving on their springs.
    pub fn set_reduced_motion(&mut self, on: bool) {
        self.ui.kernel.borrow_mut().set_reduced_motion(on);
    }

    /// Lands everything in motion on its target now.
    pub fn settle_motion(&mut self) {
        self.ui.kernel.borrow_mut().land_motion();
    }

    /// Runs what other threads woke (background results, channel messages). Shells call this
    /// when the app's waker fires.
    pub fn poll(&mut self) {
        self.input_frame(|| {});
    }

    /// Lets the shell know when another thread woke the app (call [`App::poll`] then).
    pub fn set_waker(&mut self, wake: impl Fn() + Send + Sync + 'static) {
        self.ui.sched.set_waker(std::sync::Arc::new(wake));
    }

    /// Lands asynchronous work without moving the clock: waits (in real time) for background
    /// jobs, and runs every woken task, until none is left. Returns what is still pending
    /// (tasks waiting on timers or channels, subscriptions).
    pub fn settle(&mut self) -> Vec<PendingWork> {
        let sched = self.ui.sched.clone();
        for _ in 0..1000 {
            sched.wait_background(BACKGROUND_WAIT);
            self.poll();
            if sched.idle() {
                break;
            }
        }
        self.pending()
    }

    /// Settles, then moves the clock from deadline to deadline (at most `limit` ahead) until no
    /// task is left: subscriptions (intervals, channel listeners) do not count. `Err` lists what
    /// is still running at the limit.
    pub fn run_until_idle(&mut self, limit: Duration) -> Result<(), Vec<PendingWork>> {
        let end = self.now() + limit;
        loop {
            let pending = self.settle();
            let working: Vec<PendingWork> = pending.into_iter().filter(|p| p.kind != WorkKind::Subscription).collect();
            if working.is_empty() {
                return Ok(());
            }
            match self.next_deadline().filter(|d| *d <= end) {
                Some(d) => self.advance_to(d),
                None => return Err(working),
            }
        }
    }

    /// What is pending: tasks, subscriptions and background jobs, with their scope, where they
    /// started, and when the timer each waits on is due.
    pub fn pending(&self) -> Vec<PendingWork> {
        let mut out = self.enter(|| self.ui.sched.pending());
        let k = self.ui.kernel.borrow();
        out.extend(k.motions().into_iter().map(|(id, rest)| PendingWork {
            kind: WorkKind::Motion,
            label: format!("motion of {}", k.describe(id)),
            scope: k.test_id(id).unwrap_or("").to_owned(),
            at: k.origin(id).map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default(),
            due: Some(duration(rest * 1000.0)),
        }));
        out
    }

    /// Sends `payload` to the channel named `name` (see `Receiver::named`), as the outside world
    /// would; it lands at once.
    pub fn send(&mut self, name: &str, payload: &str) -> Result<(), String> {
        self.ui.sched.send_named(name, payload)?;
        self.poll();
        Ok(())
    }
}

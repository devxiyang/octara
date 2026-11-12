//! The app's clock and its pending work: time moves only when the shell (with real time) or a
//! test (`advance`) moves it; timers, long presses and momentum act on it. Tests and agents land
//! asynchronous work with `settle` and run it out with `run_until_idle`. One clock for the app;
//! each window's long presses, glides and motion follow it.

use super::{App, Window};
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
    /// The open windows' handles.
    pub(crate) fn each(&self) -> Vec<Window> {
        self.windows().into_iter().filter_map(|id| self.window(id)).collect()
    }

    /// The app's clock: time since it started, as the shell (or a test) advances it.
    pub fn now(&self) -> Duration {
        duration(self.app.sched.now_ms())
    }

    /// Moves the clock forward by `dt`: timers and long presses due on the way fire in order,
    /// momentum glides.
    pub fn advance(&self, dt: Duration) {
        self.advance_to(self.now() + dt);
    }

    /// Moves the clock to `t` (shells call this before each event and frame with the time since
    /// launch). Whatever comes due on the way runs at its own time. Time never goes back.
    pub fn advance_to(&self, t: Duration) {
        let sched = self.app.sched.clone();
        let t = ms(t).max(sched.now_ms());
        for _ in 0..100_000 {
            let windows = self.each();
            let presses = windows.iter().filter_map(|w| w.long_press_due());
            let next = sched.next_timer().into_iter().chain(presses).min_by(f64::total_cmp);
            let Some(due) = next.filter(|d| *d <= t) else { break };
            self.set_clock(due.max(sched.now_ms()));
            self.enter(|| sched.fire_due());
            for w in &windows {
                w.input_frame(|| w.pointer_time());
            }
        }
        self.set_clock(t);
        for w in self.each() {
            let glides = w.w.kernel.borrow().gliding_scrolls();
            // Hover held while layout moved catches up once it has settled.
            let catch_up = w.hover_held() && !w.w.kernel.borrow().layout_in_motion();
            if w.gliding() || glides || catch_up {
                w.input_frame(|| {
                    w.pointer_time();
                    w.w.kernel.borrow_mut().step_glides();
                });
            }
        }
    }

    /// When the app next needs the clock advanced (a timer, a long press coming due, the next
    /// frame of momentum or of motion in any window), if anything is pending. Shells wake up then.
    pub fn next_deadline(&self) -> Option<Duration> {
        let windows = self.each();
        let own = windows.iter().filter_map(|w| w.deadline());
        self.app.sched.next_timer().into_iter().chain(own).min_by(f64::total_cmp).map(duration)
    }

    /// Moves the clock: the scheduler's and the time each window's kernel shows.
    fn set_clock(&self, ms: f64) {
        self.app.sched.set_now_ms(ms);
        for w in self.each() {
            w.w.kernel.borrow_mut().set_time(ms / 1000.0);
        }
    }

    /// Reduce Motion (the system setting): positions and sizes land at once; opacity and
    /// colors fade briefly instead of moving on their springs.
    pub fn set_reduced_motion(&self, on: bool) {
        for w in self.each() {
            w.w.kernel.borrow_mut().set_reduced_motion(on);
        }
    }

    /// Lands everything in motion on its target now, in every window.
    pub fn settle_motion(&self) {
        for w in self.each() {
            w.w.kernel.borrow_mut().land_motion();
        }
    }

    /// Runs what other threads woke (background results, channel messages). Shells call this
    /// when the app's waker fires.
    pub fn poll(&self) {
        match self.each().first() {
            Some(w) => w.input_frame(|| {}),
            None => self.enter(|| {
                self.app.sched.run_ready();
            }),
        }
    }

    /// Lands asynchronous work without moving the clock: waits (in real time) for background
    /// jobs, and runs every woken task, until none is left. Returns what is still pending
    /// (tasks waiting on timers or channels, subscriptions).
    pub fn settle(&self) -> Vec<PendingWork> {
        let sched = self.app.sched.clone();
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
    pub fn run_until_idle(&self, limit: Duration) -> Result<(), Vec<PendingWork>> {
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
    /// started, and when the timer each waits on is due; and motion in every window.
    pub fn pending(&self) -> Vec<PendingWork> {
        let mut out = self.enter(|| self.app.sched.pending());
        for w in self.each() {
            out.extend(w.motion_work());
        }
        out
    }

    /// Sends `payload` to the channel named `name` (see `Receiver::named`), as the outside world
    /// would; it lands at once.
    pub fn send(&self, name: &str, payload: &str) -> Result<(), String> {
        self.app.sched.send_named(name, payload)?;
        self.poll();
        Ok(())
    }
}

impl Window {
    /// When this window next needs a frame: a long press coming due, momentum, motion, a
    /// secure field's shown character hiding (ms on the app's clock).
    fn deadline(&self) -> Option<f64> {
        let frame = self.w.kernel.borrow_mut().in_motion().then(|| self.app.sched.now_ms() + MOTION_FRAME_MS);
        let reveal = self.w.kernel.borrow().reveal_deadline().map(|t| t * 1000.0);
        [self.pointer_deadline(), frame, reveal].into_iter().flatten().min_by(f64::total_cmp)
    }

    /// This window's motion, as pending work.
    fn motion_work(&self) -> Vec<PendingWork> {
        let k = self.w.kernel.borrow();
        k.motions()
            .into_iter()
            .map(|(id, rest)| PendingWork {
                kind: WorkKind::Motion,
                label: format!("motion of {}", k.describe(id)),
                scope: k.test_id(id).unwrap_or("").to_owned(),
                at: k.origin(id).map(|l| format!("{}:{}", crate::diagnostics::source_file(l), l.line())).unwrap_or_default(),
                due: Some(duration(rest * 1000.0)),
            })
            .collect()
    }
}

//! The app's clock and its pending work: time moves only when the shell (with real time) or a
//! test (`advance`) moves it; timers, long presses and momentum act on it. Tests and agents land
//! asynchronous work with `settle` and run it out with `run_until_idle`. One clock for the app;
//! each window's long presses, glides and motion follow it.

use super::{App, Window, WindowState};
use crate::task::{PendingWork, WorkKind};
use std::time::Duration;

/// The default interval when a clock driver relies on deadlines for animation samples.
const MOTION_FRAME_MS: f64 = 1000.0 / 60.0;

/// How long `settle` waits for background jobs (real time) before reporting them pending.
const BACKGROUND_WAIT: Duration = Duration::from_secs(10);

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}
/// A due time (whole nanoseconds, see `whole_ns`) as a `Duration`: moving the clock there reaches
/// exactly the time it stands for.
fn due(ms: f64) -> Duration {
    Duration::from_nanos((ms * 1e6).round() as u64)
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
    /// launch). Due work runs at its own time while the turn allowance permits; remaining
    /// ready tasks continue on a later turn at the advanced clock. Time never goes back.
    pub fn advance_to(&self, t: Duration) {
        self.app.assert_usable();
        if self.is_shutdown() {
            return;
        }
        self.enter(|| self.advance_clock_to(t));
    }

    fn advance_clock_to(&self, t: Duration) {
        let sched = self.app.sched.clone();
        let t = crate::task::whole_ns(ms(t)).max(sched.now_ms());
        for _ in 0..100_000 {
            let windows = self.each();
            let presses = windows.iter().filter_map(|w| w.pointer_due());
            let next = sched.next_timer().into_iter().chain(presses).min_by(f64::total_cmp);
            let Some(due) = next.filter(|d| *d <= t) else { break };
            self.set_clock(due.max(sched.now_ms()));
            self.enter(|| sched.fire_due());
            if windows.is_empty() {
                // App work survives its last window. Run it at this timer's own time, then
                // settle any app watches or window requests before advancing to the next due.
                self.app.poll_requested.set(true);
                self.app.frame(None, std::panic::Location::caller());
            }
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
        // The clock moving is an operation like any other: it ends with a frame, which presents
        // what time alone changed (motion's next frame, a caret's blink) to each platform window.
        self.app.frame(None, std::panic::Location::caller());
    }

    /// When the app next needs a timer, gesture deadline or animation sample. Clock drivers
    /// using display callbacks should combine [`Self::next_timer_deadline`] with each window's
    /// [`crate::platform::Frame::continuous`] demand instead.
    /// Always after now (by a nanosecond at least): a deadline reached means "as soon as
    /// possible", so moving the clock to it always moves it on.
    pub fn next_deadline(&self) -> Option<Duration> {
        self.deadline(true)
    }

    /// The next deadline for tasks, gestures, carets and secure entry. Window drivers schedule
    /// animation samples on their own frame clocks.
    pub fn next_timer_deadline(&self) -> Option<Duration> {
        self.deadline(false)
    }

    fn deadline(&self, include_animation: bool) -> Option<Duration> {
        if self.is_shutdown() {
            return None;
        }
        let windows = self.each();
        let own = windows.iter().filter_map(Window::timer_deadline);
        let animation = (include_animation && windows.iter().any(Window::animation_frame_demand))
            .then(|| self.app.sched.now_ms() + MOTION_FRAME_MS)
            .into_iter();
        let soonest = crate::task::whole_ns(self.app.sched.now_ms() + 1e-6);
        self.app.sched.next_timer().into_iter().chain(own).chain(animation).min_by(f64::total_cmp).map(|d| due(d.max(soonest)))
    }

    /// Moves the clock: the scheduler's and the time each window's kernel shows.
    fn set_clock(&self, ms: f64) {
        self.app.sched.set_now_ms(ms);
        for w in self.each() {
            w.w.kernel.borrow_mut().set_time(ms / 1000.0);
        }
    }

    /// Lands everything in motion on its target now, in every window.
    pub fn settle_motion(&self) {
        for w in self.each() {
            w.w.kernel.borrow_mut().land_motion();
            w.input_frame(|| w.w.land_paced());
        }
    }

    /// Runs what other threads woke (background results, channel messages). Shells call this
    /// when the app's waker fires. A bounded turn may leave ready work pending and request
    /// another wake; use `pending()` to inspect unfinished work. A nested call requests a drain
    /// at the enclosing update boundary, after the caller gives back its borrows.
    #[track_caller]
    pub fn poll(&self) {
        use crate::reactive::graph::{Operation, Runtime};
        let at = std::panic::Location::caller();
        if self.is_shutdown() || !Runtime::caller_allows(Operation::Write, at) {
            return;
        }
        self.app.poll_requested.set(true);
        if self.app.depth.get() != 0 {
            return;
        }
        // Keep the input cause and stationary-pointer refresh of a window-host poll.
        match self.each().into_iter().find(Window::can_prepare) {
            Some(w) => w.input_frame(|| {}),
            None => self.app.frame(None, at),
        }
    }

    /// Lands asynchronous work without moving the clock: waits (in real time) for background
    /// jobs, and runs every woken task, until none is left. Returns what is still pending
    /// (tasks waiting on timers or channels, subscriptions).
    pub fn settle(&self) -> Vec<PendingWork> {
        if self.is_shutdown() {
            return self.pending();
        }
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
        if self.is_failed() && !self.is_shutdown() {
            return Err(self.pending());
        }
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
    /// started, and when the timer each waits on is due; motion in every window; and unfinished
    /// reactive/window preparation. Unfinished updates do not create timers or retry deadlines.
    pub fn pending(&self) -> Vec<PendingWork> {
        let mut out = self.app.rt.enter(|| self.app.sched.pending());
        if self.is_failed() && !self.is_shutdown() {
            out.push(PendingWork {
                kind: WorkKind::Update,
                label: "App failed after a panic; shutdown required".into(),
                scope: "app".into(),
                at: String::new(),
                due: None,
            });
        }
        if self.is_shutdown() && self.app.teardown.get() != super::Teardown::Complete {
            out.push(PendingWork {
                kind: WorkKind::Update,
                label: "App execution stopped; resource teardown is incomplete".into(),
                scope: "app".into(),
                at: String::new(),
                due: None,
            });
        }
        if self.app.sched.yielded() {
            out.push(PendingWork {
                kind: WorkKind::Update,
                label: "ready tasks yielded at the App turn limit".into(),
                scope: "app".into(),
                at: String::new(),
                due: None,
            });
        }
        if self.app.rt.has_pending() {
            out.push(PendingWork {
                kind: WorkKind::Update,
                label: "reactive updates are still pending".into(),
                scope: "app".into(),
                at: String::new(),
                due: None,
            });
        }
        if self.app.has_window_requests() {
            out.push(PendingWork {
                kind: WorkKind::Update,
                label: "window lifecycle requests are still pending".into(),
                scope: "app".into(),
                at: String::new(),
                due: None,
            });
        }
        for w in self.each() {
            if !w.can_prepare() {
                continue;
            }
            out.extend(w.motion_work());
            if !w.frame_is_settled() {
                out.push(PendingWork {
                    kind: WorkKind::Update,
                    label: "window updates are still pending".into(),
                    scope: format!("window {} ({})", w.w.id.number(), w.w.options.title),
                    at: String::new(),
                    due: None,
                });
            }
        }
        out
    }

    /// Sends `payload` to the channel named `name` (see `Receiver::named`), as the outside world
    /// would; it lands at once.
    pub fn send(&self, name: &str, payload: &str) -> Result<(), String> {
        self.app.check_running()?;
        self.app.sched.send_named(name, payload)?;
        self.poll();
        Ok(())
    }
}

impl Window {
    fn timer_deadline(&self) -> Option<f64> {
        let k = self.w.kernel.borrow();
        let reveal = k.reveal_deadline().map(|t| t * 1000.0);
        let caret = k.caret_deadline().map(|t| t * 1000.0);
        [self.pointer_due(), reveal, caret].into_iter().flatten().min_by(f64::total_cmp)
    }

    pub(super) fn animation_frame_demand(&self) -> bool {
        let k = self.w.kernel.borrow();
        self.gliding() || k.gliding_scrolls() || !k.at_rest()
    }

    /// This window's motion, as pending work.
    fn motion_work(&self) -> Vec<PendingWork> {
        let k = self.w.kernel.borrow();
        let paced = self
            .w
            .paced
            .borrow()
            .iter()
            .filter(|(_, p)| p.behind())
            .map(|(_, p)| PendingWork {
                kind: WorkKind::Motion,
                label: "paced text".into(),
                scope: String::new(),
                at: format!("{}:{}", crate::diagnostics::source_file(p.at), p.at.line()),
                due: None,
            })
            .collect::<Vec<_>>();
        k.motions()
            .into_iter()
            .map(|(id, rest)| PendingWork {
                kind: WorkKind::Motion,
                label: format!("motion of {}", k.describe(id)),
                scope: k.test_id(id).unwrap_or("").to_owned(),
                at: k.origin(id).map(|l| format!("{}:{}", crate::diagnostics::source_file(l), l.line())).unwrap_or_default(),
                due: Some(duration(rest * 1000.0)),
            })
            .chain(paced)
            .collect()
    }
}

impl WindowState {
    /// Shows all registered paced texts now, outside the registry borrow.
    pub(crate) fn land_paced(&self) {
        let paced: Vec<_> = self.paced.borrow().iter().map(|(_, p)| p.clone()).collect();
        for p in paced {
            p.land();
        }
    }
}

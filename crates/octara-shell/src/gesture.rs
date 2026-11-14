//! A gesture in a real window. The host advances its wall clock and polls this job between
//! native events; this module never moves app time or blocks the event loop.

use octara::app::{InputEvent, Phase, PointerEvent, PointerKind, Window, WindowId};
use octara::diagnostics as codes;
use octara::inspect::{Delivery, Reply, Request, TargetGuard};
use octara::kernel::NodeId;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const MAX_WAIT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    DoubleTap,
    LongPress,
}

enum Stage {
    /// A previous click run must expire before this independent double click starts.
    First,
    Second {
        first_wall: Instant,
        first_clock: Duration,
        click_sequence: u64,
        gap: Duration,
    },
    Hold {
        pointer: u64,
        until: Instant,
    },
}

pub(crate) struct Job {
    window: WindowId,
    node: NodeId,
    target: String,
    point: (f32, f32),
    stage: Stage,
    deadline: Instant,
    delivered: bool,
    guard: Option<TargetGuard>,
}

fn failed(why: impl Into<String>) -> Reply {
    Reply::fail(&codes::STEP_FAILED, why)
}

impl Job {
    /// Start after the host has ticked and polled the app. A successful start can still be
    /// waiting for an earlier click run, so callers must use `delivered` for feedback.
    #[cfg(test)]
    pub(crate) fn start(window: &mut Window, target: &str, kind: Kind, now: Instant) -> Result<Self, Box<Reply>> {
        Self::start_guarded(window, target, kind, now, None)
    }

    pub(crate) fn start_guarded(
        window: &mut Window,
        target: &str,
        kind: Kind,
        now: Instant,
        guard: Option<TargetGuard>,
    ) -> Result<Self, Box<Reply>> {
        if guard.as_ref().is_some_and(|guard| !guard.is_current(window)) {
            let mut reply = Reply::fail(&codes::STALE_TARGET, "the observed gesture target is no longer current");
            reply.delivery = Some(Delivery::Refused);
            return Err(Box::new(reply));
        }
        if !window.kernel().exists(target) {
            return Err(Box::new(Reply::fail(&codes::NO_SUCH_ELEMENT, format!("no element with test id `{target}`"))));
        }
        let (node, point) = window.pointer_target(target).map_err(|why| Box::new(failed(why)))?;
        if guard.as_ref().is_some_and(|guard| guard.node() != Some(node)) {
            let mut reply = Reply::fail(&codes::STALE_TARGET, "the gesture target differs from the observed node");
            reply.delivery = Some(Delivery::Refused);
            return Err(Box::new(reply));
        }
        if window.held_pointer().is_some() {
            return Err(Box::new(failed("the window already has a pointer down")));
        }
        let tuning = window.tuning();
        let timing = match kind {
            Kind::DoubleTap => tuning.double_click,
            Kind::LongPress => tuning.long_press,
        };
        if timing > MAX_WAIT
            || (matches!(kind, Kind::DoubleTap) && (timing.is_zero() || !tuning.mouse_slop.is_finite() || tuning.mouse_slop < 0.0))
        {
            return Err(Box::new(Reply::fail(
                &codes::BAD_REQUEST,
                "live gesture timing must fit within 30 seconds; double-click timing must be positive and its distance finite and non-negative",
            )));
        }
        let mut job = Self {
            window: window.id(),
            node,
            target: target.into(),
            point,
            stage: Stage::First,
            deadline: now + MAX_WAIT,
            delivered: false,
            guard,
        };
        match kind {
            Kind::DoubleTap => {
                if window.pointer_click_cooldown().is_zero() {
                    job.first_click(window, now)?;
                }
            }
            Kind::LongPress => {
                // Native touches have their own ids. Give each synthetic hold a distinct id;
                // cancellation never sends a mouse cancel or releases another held pointer.
                static NEXT: AtomicU64 = AtomicU64::new(u64::MAX);
                let pointer = NEXT.fetch_sub(1, Ordering::Relaxed);
                window.event(InputEvent::Pointer(PointerEvent::touch(pointer, Phase::Down, point.0, point.1)));
                job.delivered = true;
                job.stage = Stage::Hold { pointer, until: now + timing };
            }
        }
        Ok(job)
    }

    pub(crate) fn window(&self) -> WindowId {
        self.window
    }

    pub(crate) fn delivered(&self) -> bool {
        self.delivered
    }

    fn reply(&self, mut reply: Reply) -> Reply {
        reply.delivery = Some(if self.delivered { Delivery::Delivered } else { Delivery::Refused });
        reply
    }

    /// Only releases a held touch belonging to this job. A mouse click is completed in one
    /// UI turn and never remains held while this job waits.
    pub(crate) fn cancel(&self, window: &mut Window) {
        if window.id() != self.window {
            return;
        }
        if let Stage::Hold { pointer, .. } = self.stage
            && window.held_pointer() == Some((pointer, PointerKind::Touch))
        {
            window.event(InputEvent::Pointer(PointerEvent::touch(pointer, Phase::Cancel, self.point.0, self.point.1)));
        }
    }

    fn current_point(&self, window: &Window) -> Result<(f32, f32), Box<Reply>> {
        if self.guard.as_ref().is_some_and(|guard| !guard.is_current(window)) {
            return Err(Box::new(self.reply(Reply::fail(&codes::STALE_TARGET, "the observed gesture target is no longer current"))));
        }
        if window.id() != self.window || window.is_closed() {
            return Err(Box::new(self.reply(failed("the target window closed during the gesture"))));
        }
        let (node, point) = window.pointer_target(&self.target).map_err(|why| Box::new(self.reply(failed(why))))?;
        if node != self.node {
            let code = if self.guard.is_some() { &codes::STALE_TARGET } else { &codes::STEP_FAILED };
            return Err(Box::new(self.reply(Reply::fail(code, "the gesture target was replaced while waiting"))));
        }
        Ok(point)
    }

    fn click(&mut self, window: &mut Window, point: (f32, f32)) -> Reply {
        if let Some(guard) = &self.guard {
            let reply = octara::inspect::handle_guarded(window, Request::Tap { id: self.target.clone() }, Some(guard));
            self.delivered |= reply.delivery == Some(Delivery::Delivered);
            self.reply(reply)
        } else {
            for phase in [Phase::Move, Phase::Down, Phase::Up] {
                window.event(InputEvent::Pointer(PointerEvent::mouse(phase, point.0, point.1)));
            }
            self.delivered = true;
            self.reply(Reply::ok())
        }
    }

    fn first_click(&mut self, window: &mut Window, now: Instant) -> Result<(), Box<Reply>> {
        let first_clock = window.app().now();
        let click_sequence = window.pointer_click_sequence().wrapping_add(1);
        let reply = self.click(window, self.point);
        if !reply.ok {
            return Err(Box::new(reply));
        }
        let gap = window.tuning().double_click / 2;
        self.stage = Stage::Second { first_wall: now, first_clock, click_sequence, gap: gap.min(Duration::from_millis(50)) };
        Ok(())
    }

    /// The host ticks to actual elapsed time before calling this. `None` keeps waiting.
    pub(crate) fn poll(&mut self, window: &mut Window, now: Instant) -> Option<Reply> {
        if window.id() == self.window
            && !window.is_closed()
            && let Stage::Hold { pointer, .. } = self.stage
            && window.pointer_long_pressed(pointer)
        {
            // The handler can itself remove the node or cover it with a menu. Recognition is
            // already complete; cancel only the spent touch without pressing a replacement.
            window.event(InputEvent::Pointer(PointerEvent::touch(pointer, Phase::Up, self.point.0, self.point.1)));
            return Some(self.reply(Reply::ok()));
        }
        let point = match self.current_point(window) {
            Ok(point) => point,
            Err(reply) => {
                self.cancel(window);
                return Some(*reply);
            }
        };
        if now >= self.deadline {
            self.cancel(window);
            return Some(self.reply(failed("the realtime gesture wait budget ended")));
        }
        match self.stage {
            Stage::First => {
                if window.held_pointer().is_some() {
                    return Some(self.reply(failed("another pointer went down while the double click was waiting")));
                }
                if window.pointer_click_cooldown().is_zero() {
                    self.point = point;
                    if let Err(reply) = self.first_click(window, now) {
                        return Some(*reply);
                    }
                }
                None
            }
            Stage::Second { first_wall, first_clock, click_sequence, gap } => {
                let wall_elapsed = now.saturating_duration_since(first_wall);
                let clock_elapsed = window.app().now().saturating_sub(first_clock);
                let tuning = window.tuning();
                if !tuning.mouse_slop.is_finite() || tuning.mouse_slop < 0.0 {
                    return Some(self.reply(failed("the double-click distance became invalid after the first click")));
                }
                if wall_elapsed > tuning.double_click || clock_elapsed > tuning.double_click {
                    return Some(
                        self.reply(failed("the second click missed the real double-click interval; the first click was already delivered")),
                    );
                }
                if wall_elapsed < gap || clock_elapsed < gap {
                    return None;
                }
                if window.held_pointer().is_some() {
                    return Some(self.reply(failed("another pointer went down before the second click")));
                }
                if window.pointer_click_sequence() != click_sequence {
                    return Some(self.reply(failed("another click interrupted the double click after its first click")));
                }
                let distance = ((point.0 - self.point.0).powi(2) + (point.1 - self.point.1).powi(2)).sqrt();
                if distance > tuning.mouse_slop {
                    return Some(self.reply(failed("the target moved beyond the double-click distance after the first click")));
                }
                Some(self.click(window, point))
            }
            Stage::Hold { pointer, until } => {
                if window.held_pointer() != Some((pointer, PointerKind::Touch)) {
                    return Some(self.reply(failed("the held touch was cancelled or replaced before long-press recognition")));
                }
                let k = window.kernel();
                let at = self.point;
                let inside = k.hit_test(at.0, at.1).is_some_and(|hit| k.is_within(hit, self.node));
                drop(k);
                if !inside {
                    self.cancel(window);
                    return Some(self.reply(failed("the held point no longer reaches its original target")));
                }
                if now >= until {
                    // The native loop has ticked beyond the hold duration. Without recognition
                    // lifting could become a short press; cancel instead of inventing success.
                    self.cancel(window);
                    return Some(self.reply(failed("the target did not recognize the held touch as a long press")));
                }
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use octara::prelude::*;

    fn window() -> Window {
        Window::headless(|| {
            let presses = state("presses", || 0);
            let doubles = state("doubles", || 0);
            let longs = state("longs", || 0);
            col()
                .size_full()
                .child(text(move || format!("{},{},{}", presses.get(), doubles.get(), longs.get())).id("log"))
                .child(
                    el().id("target")
                        .w(120)
                        .h(48)
                        .on_press(move || presses.update(|n| *n += 1))
                        .on_double_press(move || doubles.update(|n| *n += 1))
                        .on_long_press(move || longs.update(|n| *n += 1)),
                )
                .into()
        })
    }

    fn guard(window: &Window) -> TargetGuard {
        let target = window.inspect().find("target").unwrap().target.clone();
        let request = octara::serde_json::json!({ "op": "double_tap", "id": "target", "target": target });
        octara::inspect::parse_guarded_request(window, &request.to_string()).unwrap().guard.unwrap()
    }

    #[component]
    fn removing_long_target() -> View {
        let shown = state("shown", || true);
        col()
            .size_full()
            .child(text(move || if shown.get() { "waiting" } else { "recognized" }).id("log"))
            .child(show(move || shown.get(), move || el().id("target").w(120).h(48).on_long_press(move || shown.set(false)).into()))
            .into()
    }

    #[test]
    fn repeated_double_clicks_wait_for_the_real_click_history_to_expire() {
        let mut window = window();
        let start = Instant::now();
        let mut first = Job::start(&mut window, "target", Kind::DoubleTap, start).unwrap_or_else(|r| panic!("{r:?}"));
        assert!(first.delivered());
        assert_eq!(window.app().now(), Duration::ZERO, "starting never advances app time");
        assert_eq!(window.kernel().text_of("log").as_deref(), Some("1,0,0"));
        assert!(first.poll(&mut window, start + Duration::from_millis(50)).is_none(), "wall time alone cannot fake the app clock");
        window.app().advance(Duration::from_millis(50));
        assert!(first.poll(&mut window, start + Duration::from_millis(50)).unwrap().ok);
        assert_eq!(window.kernel().text_of("log").as_deref(), Some("2,1,0"));

        let mut second =
            Job::start(&mut window, "target", Kind::DoubleTap, start + Duration::from_millis(50)).unwrap_or_else(|r| panic!("{r:?}"));
        assert!(!second.delivered(), "the preceding click run must expire first");
        window.app().advance(Duration::from_millis(501));
        assert!(second.poll(&mut window, start + Duration::from_millis(551)).is_none());
        assert!(second.delivered());
        window.app().advance(Duration::from_millis(50));
        assert!(second.poll(&mut window, start + Duration::from_millis(601)).unwrap().ok);
        assert_eq!(window.kernel().text_of("log").as_deref(), Some("4,2,0"));
    }

    #[test]
    fn a_late_second_click_does_not_claim_a_double_click_or_send_another_press() {
        let mut window = window();
        let start = Instant::now();
        let mut job = Job::start(&mut window, "target", Kind::DoubleTap, start).unwrap_or_else(|r| panic!("{r:?}"));
        window.app().advance(Duration::from_millis(501));
        let reply = job.poll(&mut window, start + Duration::from_millis(501)).unwrap();
        assert!(!reply.ok);
        assert!(job.delivered());
        assert_eq!(window.kernel().text_of("log").as_deref(), Some("1,0,0"));
    }

    #[test]
    fn a_completed_intervening_click_does_not_turn_a_double_into_an_unreported_triple() {
        let mut window = window();
        let start = Instant::now();
        let mut job = Job::start(&mut window, "target", Kind::DoubleTap, start).unwrap_or_else(|r| panic!("{r:?}"));
        window.tap("target").unwrap();
        assert_eq!(window.held_pointer(), None, "the competing click has already lifted");
        window.app().advance(Duration::from_millis(50));
        assert!(!job.poll(&mut window, start + Duration::from_millis(50)).unwrap().ok);
        assert_eq!(window.kernel().text_of("log").as_deref(), Some("2,1,0"), "the job must not send a third click");
    }

    #[test]
    fn replacing_a_target_does_not_receive_the_second_click() {
        let mut window = window();
        let start = Instant::now();
        let mut job = Job::start(&mut window, "target", Kind::DoubleTap, start).unwrap_or_else(|r| panic!("{r:?}"));
        window.app().hot_reload();
        window.app().advance(Duration::from_millis(50));
        let reply = job.poll(&mut window, start + Duration::from_millis(50)).unwrap();
        assert!(!reply.ok);
        assert_eq!(reply.code.as_deref(), Some(codes::STEP_FAILED.id));
        assert_eq!(window.kernel().text_of("log").as_deref(), Some("1,0,0"));
    }

    #[test]
    fn a_guarded_double_click_keeps_the_normal_recognizer_path() {
        let mut window = window();
        let start = Instant::now();
        let guard = guard(&window);
        let mut job = Job::start_guarded(&mut window, "target", Kind::DoubleTap, start, Some(guard)).unwrap();
        assert_eq!(window.kernel().text_of("log").as_deref(), Some("1,0,0"));
        window.app().advance(Duration::from_millis(50));
        let reply = job.poll(&mut window, start + Duration::from_millis(50)).unwrap();
        assert!(reply.ok, "{reply:?}");
        assert_eq!(reply.delivery, Some(Delivery::Delivered));
        assert_eq!(window.kernel().text_of("log").as_deref(), Some("2,1,0"));
        assert_eq!(window.held_pointer(), None);
    }

    #[test]
    fn a_guarded_wait_refuses_a_rebuilt_target_before_its_first_click() {
        let mut window = window();
        window.tap("target").unwrap();
        let start = Instant::now();
        let guard = guard(&window);
        let mut job = Job::start_guarded(&mut window, "target", Kind::DoubleTap, start, Some(guard)).unwrap();
        assert!(!job.delivered(), "the previous click run still has to expire");
        window.app().hot_reload();
        window.app().advance(Duration::from_millis(501));
        let reply = job.poll(&mut window, start + Duration::from_millis(501)).unwrap();
        assert_eq!(reply.code.as_deref(), Some(codes::STALE_TARGET.id));
        assert_eq!(reply.delivery, Some(Delivery::Refused));
        assert!(!job.delivered());
        assert_eq!(window.kernel().text_of("log").as_deref(), Some("1,0,0"));
    }

    #[test]
    fn a_guarded_second_click_preserves_the_first_clicks_delivery_after_rebuild() {
        let mut window = window();
        let start = Instant::now();
        let guard = guard(&window);
        let mut job = Job::start_guarded(&mut window, "target", Kind::DoubleTap, start, Some(guard)).unwrap();
        assert!(job.delivered());
        window.app().hot_reload();
        window.app().advance(Duration::from_millis(50));
        let reply = job.poll(&mut window, start + Duration::from_millis(50)).unwrap();
        assert_eq!(reply.code.as_deref(), Some(codes::STALE_TARGET.id));
        assert_eq!(reply.delivery, Some(Delivery::Delivered));
        assert!(job.delivered());
        assert_eq!(window.kernel().text_of("log").as_deref(), Some("1,0,0"));
    }

    #[test]
    fn a_recognized_guarded_long_press_may_remove_its_target() {
        let mut window = Window::headless(removing_long_target);
        let start = Instant::now();
        let guard = guard(&window);
        let mut job = Job::start_guarded(&mut window, "target", Kind::LongPress, start, Some(guard)).unwrap();
        let hold = window.tuning().long_press;
        window.app().advance(hold);
        assert!(!window.kernel().exists("target"));
        let reply = job.poll(&mut window, start + hold).unwrap();
        assert!(reply.ok, "{reply:?}");
        assert_eq!(reply.delivery, Some(Delivery::Delivered));
        assert_eq!(window.held_pointer(), None);
        assert_eq!(window.kernel().text_of("log").as_deref(), Some("recognized"));
        assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
    }

    #[test]
    fn long_press_recognition_and_cancel_only_release_the_owned_touch() {
        let mut window = window();
        let start = Instant::now();
        let mut job = Job::start(&mut window, "target", Kind::LongPress, start).unwrap_or_else(|r| panic!("{r:?}"));
        assert!(job.delivered());
        assert_eq!(window.app().now(), Duration::ZERO);
        assert!(window.held_pointer().is_some());
        window.app().advance(Duration::from_millis(500));
        assert!(job.poll(&mut window, start + Duration::from_millis(501)).unwrap().ok);
        assert_eq!(window.held_pointer(), None);
        assert_eq!(window.kernel().text_of("log").as_deref(), Some("0,0,1"));

        let cancelled =
            Job::start(&mut window, "target", Kind::LongPress, start + Duration::from_millis(501)).unwrap_or_else(|r| panic!("{r:?}"));
        cancelled.cancel(&mut window);
        assert_eq!(window.held_pointer(), None);
        let (_, point) = window.pointer_target("target").unwrap();
        window.event(InputEvent::Pointer(PointerEvent::touch(23, Phase::Down, point.0, point.1)));
        cancelled.cancel(&mut window);
        assert_eq!(window.held_pointer(), Some((23, PointerKind::Touch)), "an old job cannot cancel a person's new touch");
        window.event(InputEvent::Pointer(PointerEvent::touch(23, Phase::Cancel, point.0, point.1)));
    }
}

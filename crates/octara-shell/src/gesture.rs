//! A gesture in a real window. The host advances its wall clock and polls this job between
//! native events; this module never moves app time or blocks the event loop.

use octara::app::TargetGuard;
use octara::app::{InputEvent, Phase, PointerEvent, PointerKind, Window, WindowId};
use octara::diagnostics as codes;
use octara::inspect::{Delivery, PointerStep, Reply, Request, dispatch_pointer};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const MAX_WAIT: Duration = Duration::from_secs(30);
const DRAG_STEP: Duration = Duration::from_millis(16);
static NEXT_POINTER: AtomicU64 = AtomicU64::new(u64::MAX);

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    DoubleTap,
    LongPress,
    Drag { dx: f32, dy: f32 },
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
    Drag {
        pointer: u64,
        delta: (f32, f32),
        step: u8,
        last_wall: Instant,
        last_clock: Duration,
    },
}

pub(crate) struct Job {
    window: WindowId,
    point: (f32, f32),
    stage: Stage,
    deadline: Instant,
    delivered: bool,
    guard: TargetGuard,
    pub(crate) trace_epoch: Option<Instant>,
}

fn failed(why: impl Into<String>) -> Reply {
    Reply::fail(&codes::STEP_FAILED, why)
}

impl Job {
    #[cfg(test)]
    pub(crate) fn start(
        window: &mut Window,
        target: &str,
        kind: Kind,
        now: Instant,
        steps: &mut Vec<PointerStep>,
    ) -> Result<Self, Box<Reply>> {
        let line = octara::serde_json::json!({"op": "tap", "id": target}).to_string();
        let parsed = octara::inspect::parse_guarded_request(window, &line)?;
        Self::start_guarded(window, kind, now, parsed.guard.expect("a tap has a node target"), steps)
    }

    /// Start after the host has ticked and polled the app. A successful start can still be
    /// waiting for an earlier click run, so callers must use `delivered` for feedback.
    pub(crate) fn start_guarded(
        window: &mut Window,
        kind: Kind,
        now: Instant,
        guard: TargetGuard,
        steps: &mut Vec<PointerStep>,
    ) -> Result<Self, Box<Reply>> {
        let point = octara::inspect::guarded_point(window, &guard, Delivery::Refused)?;
        if let Kind::Drag { dx, dy } = kind
            && (!dx.is_finite() || !dy.is_finite() || !(point.0 + dx).is_finite() || !(point.1 + dy).is_finite())
        {
            return Err(Box::new(Reply {
                delivery: Some(Delivery::Refused),
                ..Reply::fail(&codes::BAD_REQUEST, "drag requires finite dx, dy and endpoint coordinates")
            }));
        }
        if window.held_pointer().is_some() {
            return Err(Box::new(failed("the window already has a pointer down")));
        }
        let tuning = window.tuning();
        let timing = match kind {
            Kind::DoubleTap => tuning.double_click,
            Kind::LongPress => tuning.long_press,
            Kind::Drag { .. } => Duration::ZERO,
        };
        if timing > MAX_WAIT
            || (matches!(kind, Kind::DoubleTap) && timing.is_zero())
            || (matches!(kind, Kind::DoubleTap | Kind::Drag { .. }) && (!tuning.mouse_slop.is_finite() || tuning.mouse_slop < 0.0))
        {
            return Err(Box::new(Reply::fail(
                &codes::BAD_REQUEST,
                "live gesture timing must fit within 30 seconds; double-click timing must be positive and its distance finite and non-negative",
            )));
        }
        let mut job =
            Self { window: window.id(), point, stage: Stage::First, deadline: now + MAX_WAIT, delivered: false, guard, trace_epoch: None };
        match kind {
            Kind::DoubleTap => {
                if window.pointer_click_cooldown().is_zero() {
                    job.first_click(window, now, steps)?;
                }
            }
            Kind::LongPress => {
                // Native touches have their own ids. Give each synthetic hold a distinct id;
                // cancellation never sends a mouse cancel or releases another held pointer.
                let pointer = NEXT_POINTER.fetch_sub(1, Ordering::Relaxed);
                dispatch_pointer(window, PointerEvent::touch(pointer, Phase::Down, point.0, point.1), steps);
                job.delivered = true;
                job.stage = Stage::Hold { pointer, until: now + timing };
            }
            Kind::Drag { dx, dy } => {
                let pointer = NEXT_POINTER.fetch_sub(1, Ordering::Relaxed);
                job.stage = Stage::Drag { pointer, delta: (dx, dy), step: 1, last_wall: now, last_clock: window.app().now() };
                dispatch_pointer(window, PointerEvent { id: pointer, ..PointerEvent::mouse(Phase::Move, point.0, point.1) }, steps);
                job.delivered = true;
                let point = job.current_point(window)?;
                if !(point.0 + dx).is_finite() || !(point.1 + dy).is_finite() {
                    return Err(Box::new(job.reply(Reply::fail(&codes::BAD_REQUEST, "drag endpoint must be finite"))));
                }
                if window.held_pointer().is_some() {
                    return Err(Box::new(job.reply(failed("another pointer went down before the drag started"))));
                }
                job.point = point;
                dispatch_pointer(window, PointerEvent { id: pointer, ..PointerEvent::mouse(Phase::Down, point.0, point.1) }, steps);
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

    pub(crate) fn deadline(&self, window: &Window, started: Instant) -> Instant {
        let clock = |at: Duration, after: Duration| started.checked_add(at.saturating_add(after)).unwrap_or(self.deadline);
        let phase = match self.stage {
            Stage::First => clock(window.app().now(), window.pointer_click_cooldown()),
            Stage::Second { first_wall, first_clock, gap, .. } => (first_wall + gap).max(clock(first_clock, gap)),
            Stage::Hold { until, .. } => until,
            Stage::Drag { last_wall, last_clock, .. } => (last_wall + DRAG_STEP).max(clock(last_clock, DRAG_STEP)),
        };
        phase.min(self.deadline)
    }

    fn reply(&self, mut reply: Reply) -> Reply {
        if self.delivered {
            reply.delivery = Some(Delivery::Delivered);
        } else if reply.code.as_deref() != Some(codes::APP_FAILED.id) {
            reply.delivery = Some(Delivery::Refused);
        }
        reply
    }

    /// Only releases the synthetic touch or mouse belonging to this job. A person's pointer
    /// or another job's pointer is never cancelled.
    pub(crate) fn cancel(&self, window: &mut Window, steps: Option<&mut Vec<PointerStep>>) {
        if window.id() != self.window || window.is_closed() {
            return;
        }
        let held = match self.stage {
            Stage::Hold { pointer, .. } => Some((pointer, PointerKind::Touch)),
            Stage::Drag { pointer, .. } => Some((pointer, PointerKind::Mouse)),
            _ => None,
        };
        if let Some((pointer, kind)) = held
            && window.held_pointer() == Some((pointer, kind))
        {
            let event = PointerEvent { id: pointer, kind, ..PointerEvent::mouse(Phase::Cancel, self.point.0, self.point.1) };
            if let Some(steps) = steps {
                dispatch_pointer(window, event, steps);
            } else {
                window.event(InputEvent::Pointer(event));
            }
        }
    }

    fn current_point(&self, window: &Window) -> Result<(f32, f32), Box<Reply>> {
        let delivery = if self.delivered { Delivery::Delivered } else { Delivery::Refused };
        octara::inspect::guarded_point(window, &self.guard, delivery)
    }

    fn click(&mut self, window: &mut Window, steps: &mut Vec<PointerStep>) -> Reply {
        let mut reply = octara::inspect::handle_guarded(window, Request::Tap { id: None }, Some(&self.guard));
        steps.append(&mut reply.pointer);
        self.delivered |= reply.delivery == Some(Delivery::Delivered);
        self.reply(reply)
    }

    fn first_click(&mut self, window: &mut Window, now: Instant, steps: &mut Vec<PointerStep>) -> Result<(), Box<Reply>> {
        let first_clock = window.app().now();
        let click_sequence = window.pointer_click_sequence().wrapping_add(1);
        let reply = self.click(window, steps);
        if !reply.ok {
            return Err(Box::new(reply));
        }
        let gap = window.tuning().double_click / 2;
        self.stage = Stage::Second { first_wall: now, first_clock, click_sequence, gap: gap.min(Duration::from_millis(50)) };
        Ok(())
    }

    /// The host ticks to actual elapsed time before calling this. `None` keeps waiting.
    pub(crate) fn poll(&mut self, window: &mut Window, now: Instant, steps: &mut Vec<PointerStep>) -> Option<Reply> {
        if window.id() == self.window
            && !window.is_closed()
            && let Stage::Hold { pointer, .. } = self.stage
            && window.pointer_long_pressed(pointer)
        {
            // The handler can itself remove the node or cover it with a menu. Recognition is
            // already complete; cancel only the spent touch without pressing a replacement.
            dispatch_pointer(window, PointerEvent::touch(pointer, Phase::Up, self.point.0, self.point.1), steps);
            return Some(self.reply(Reply::ok()));
        }
        let point = match self.current_point(window) {
            Ok(point) => point,
            Err(reply) => {
                self.cancel(window, Some(steps));
                return Some(*reply);
            }
        };
        if now >= self.deadline {
            self.cancel(window, Some(steps));
            return Some(self.reply(failed("the realtime gesture wait budget ended")));
        }
        match self.stage {
            Stage::First => {
                if window.held_pointer().is_some() {
                    return Some(self.reply(failed("another pointer went down while the double click was waiting")));
                }
                if window.pointer_click_cooldown().is_zero() {
                    self.point = point;
                    if let Err(reply) = self.first_click(window, now, steps) {
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
                Some(self.click(window, steps))
            }
            Stage::Hold { pointer, until } => {
                if window.held_pointer() != Some((pointer, PointerKind::Touch)) {
                    return Some(self.reply(failed("the held touch was cancelled or replaced before long-press recognition")));
                }
                let k = window.kernel();
                let at = self.point;
                let node = self.guard.node().expect("the hold is bound to a node");
                let inside = k.hit_test(at.0, at.1).is_some_and(|hit| k.is_within(hit, node));
                drop(k);
                if !inside {
                    self.cancel(window, Some(steps));
                    return Some(self.reply(failed("the held point no longer reaches its original target")));
                }
                if now >= until {
                    // The native loop has ticked beyond the hold duration. Without recognition
                    // lifting could become a short press; cancel instead of inventing success.
                    self.cancel(window, Some(steps));
                    return Some(self.reply(failed("the target did not recognize the held touch as a long press")));
                }
                None
            }
            Stage::Drag { pointer, delta, step, last_wall, last_clock } => {
                if window.held_pointer() != Some((pointer, PointerKind::Mouse)) {
                    return Some(self.reply(failed("the drag pointer was cancelled or replaced")));
                }
                if now.saturating_duration_since(last_wall) < DRAG_STEP || window.app().now().saturating_sub(last_clock) < DRAG_STEP {
                    return None;
                }
                // Keep the observed starting point. A dragged control can move without
                // changing its identity; following its centre would incorrectly add motion.
                let fraction = f32::from(step) / 4.0;
                let at = (self.point.0 + delta.0 * fraction, self.point.1 + delta.1 * fraction);
                let trace = self.trace_epoch.map(|epoch| (epoch, super::trace::InspectInput::start(window)));
                dispatch_pointer(window, PointerEvent { id: pointer, ..PointerEvent::mouse(Phase::Move, at.0, at.1) }, steps);
                if let Some((epoch, trace)) = trace {
                    trace.log(epoch, window, "framework", Some(step));
                }
                if step == 4 {
                    if let Err(reply) = self.current_point(window) {
                        self.cancel(window, Some(steps));
                        return Some(*reply);
                    }
                    if window.held_pointer() != Some((pointer, PointerKind::Mouse)) {
                        return Some(self.reply(failed("the drag pointer was cancelled or replaced before release")));
                    }
                    dispatch_pointer(window, PointerEvent { id: pointer, ..PointerEvent::mouse(Phase::Up, at.0, at.1) }, steps);
                    Some(self.reply(Reply::ok()))
                } else {
                    self.stage = Stage::Drag { pointer, delta, step: step + 1, last_wall: now, last_clock: window.app().now() };
                    None
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use octara::prelude::*;

    fn phases(steps: &[PointerStep]) -> Vec<Phase> {
        steps.iter().map(|step| step.phase).collect()
    }

    fn window() -> Window {
        Window::headless(|| {
            let presses = state("presses", || 0);
            let doubles = state("doubles", || 0);
            let longs = state("longs", || 0);
            col().size_full().child(text(move || format!("{},{},{}", presses.get(), doubles.get(), longs.get())).id("log")).child(
                el().id("target")
                    .w(120)
                    .h(48)
                    .on_press(move || presses.update(|n| *n += 1))
                    .on_double_press(move || doubles.update(|n| *n += 1))
                    .on_long_press(move || longs.update(|n| *n += 1)),
            )
        })
    }

    fn drag_window(replace_on_start: bool) -> Window {
        Window::headless(move || {
            let moved = state("moved", Vec::<f32>::new);
            let offset = state("offset", || 0.0f32);
            let ended = state("ended", || 0);
            let replaced = state("replaced", || false);
            let replacement_moves = state("replacement_moves", || 0);
            let disabled = state("disabled", || false);
            let modal = state("modal", || false);
            on_command("disable", move || disabled.set(true));
            on_command("modal", move || modal.set(true));
            let target = move |replacement| {
                el().id("target")
                    .absolute()
                    .top(40.0)
                    .w(200.0)
                    .h(60.0)
                    .style_with(move |style| style.left(offset.get()))
                    .disabled(move || disabled.get())
                    .on_drag(move |event| {
                        if replacement {
                            replacement_moves.update(|count| *count += 1);
                        }
                        match event.phase {
                            DragPhase::Start if replace_on_start && !replacement => replaced.set(true),
                            DragPhase::Start => offset.set(event.dx),
                            DragPhase::Move => {
                                moved.update(|moves| moves.push(event.dx));
                                offset.set(event.dx);
                            }
                            DragPhase::End => ended.update(|count| *count += 1),
                            DragPhase::Cancel => offset.set(0.0),
                        }
                    })
            };
            col()
                .size_full()
                .child(text(move || format!("{:?};{};{}", moved.get(), ended.get(), replacement_moves.get())).id("log"))
                .child(keyed(move || replaced.get(), move |replacement| target(*replacement)))
                .child(show(move || modal.get(), || el().role(Role::Dialog).modal(false, || {}).child(el().focusable().size_8())))
        })
    }

    #[test]
    fn a_live_drag_waits_for_real_wall_and_app_time_and_keeps_its_starting_point() {
        let mut window = drag_window(false);
        let start = Instant::now();
        let observed = guard(&window);
        let mut steps = Vec::new();
        let mut job = Job::start_guarded(&mut window, Kind::Drag { dx: 80.0, dy: 0.0 }, start, observed, &mut steps).unwrap();
        let owned = window.held_pointer().unwrap();
        assert_eq!(owned.1, PointerKind::Mouse);
        assert_ne!(owned.0, 0, "synthetic mouse ownership differs from the native mouse");
        assert_eq!(window.app().now(), Duration::ZERO);
        assert!(job.poll(&mut window, start + DRAG_STEP, &mut steps).is_none(), "wall time alone sends no move");
        assert_eq!(phases(&steps), [Phase::Move, Phase::Down]);
        assert_eq!(window.kernel().text_of("log").as_deref(), Some("[];0;0"));
        for step in 1..=4 {
            window.app().advance(DRAG_STEP);
            let clock = window.app().now();
            let reply = job.poll(&mut window, start + DRAG_STEP * step, &mut steps);
            assert_eq!(window.app().now(), clock, "the live job never advances app time");
            if step == 4 {
                let reply = reply.expect("four realtime moves finish the drag");
                assert!(reply.ok, "{reply:?}");
                assert_eq!(reply.delivery, Some(Delivery::Delivered));
            } else {
                assert!(reply.is_none());
            }
        }
        assert_eq!(phases(&steps), [Phase::Move, Phase::Down, Phase::Move, Phase::Move, Phase::Move, Phase::Move, Phase::Up]);
        assert_eq!(
            window.kernel().text_of("log").as_deref(),
            Some("[40.0, 60.0, 80.0];1;0"),
            "the first pointer move crosses slop and emits DragPhase::Start"
        );
        assert_eq!(window.held_pointer(), None);
        assert_eq!(window.app().now(), Duration::from_millis(64));
    }

    #[test]
    fn a_live_drag_stops_at_a_replacement_without_delivering_later_moves_or_release_to_it() {
        let mut window = drag_window(true);
        let start = Instant::now();
        let observed = guard(&window);
        let mut steps = Vec::new();
        let mut job = Job::start_guarded(&mut window, Kind::Drag { dx: 80.0, dy: 0.0 }, start, observed, &mut steps).unwrap();
        window.app().advance(DRAG_STEP);
        assert!(job.poll(&mut window, start + DRAG_STEP, &mut steps).is_none());
        window.app().advance(DRAG_STEP);
        let reply = job.poll(&mut window, start + DRAG_STEP * 2, &mut steps).unwrap();
        assert_eq!(reply.code.as_deref(), Some(codes::STALE_TARGET.id));
        assert_eq!(reply.delivery, Some(Delivery::Delivered));
        assert!(phases(&steps).starts_with(&[Phase::Move, Phase::Down, Phase::Move]));
        assert!(!phases(&steps).contains(&Phase::Up));
        assert_eq!(window.kernel().text_of("log").as_deref(), Some("[];0;0"));
        assert_eq!(window.held_pointer(), None, "only the original held pointer was cancelled");
    }

    #[test]
    fn a_live_drag_refuses_nonfinite_or_held_input_and_stops_if_disabled_or_modal_inert() {
        let mut window = drag_window(false);
        let now = Instant::now();
        for (dx, dy) in [(f32::NAN, 0.0), (0.0, f32::INFINITY)] {
            let mut steps = Vec::new();
            let reply = Job::start(&mut window, "target", Kind::Drag { dx, dy }, now, &mut steps).err().unwrap();
            assert_eq!(reply.code.as_deref(), Some(codes::BAD_REQUEST.id));
            assert_eq!(reply.delivery, Some(Delivery::Refused));
            assert_eq!(window.held_pointer(), None);
            assert!(steps.is_empty());
        }
        for command in ["disable", "modal"] {
            let mut window = drag_window(false);
            let mut steps = Vec::new();
            let mut job = Job::start(&mut window, "target", Kind::Drag { dx: 80.0, dy: 0.0 }, now, &mut steps).unwrap();
            window.invoke(command).unwrap();
            let reply = job.poll(&mut window, now + DRAG_STEP, &mut steps).unwrap();
            assert!(!reply.ok);
            assert_eq!(reply.delivery, Some(Delivery::Delivered));
            assert_eq!(window.held_pointer(), None);
            assert_eq!(window.kernel().text_of("log").as_deref(), Some("[];0;0"));
        }
        let (_, point) = window.pointer_target("target").unwrap();
        window.event(InputEvent::Pointer(PointerEvent::touch(23, Phase::Down, point.0, point.1)));
        let mut steps = Vec::new();
        assert!(Job::start(&mut window, "target", Kind::Drag { dx: 80.0, dy: 0.0 }, now, &mut steps).is_err());
        assert!(steps.is_empty());
        assert_eq!(window.held_pointer(), Some((23, PointerKind::Touch)));
        window.event(InputEvent::Pointer(PointerEvent::touch(23, Phase::Cancel, point.0, point.1)));
        let job = Job::start(&mut window, "target", Kind::Drag { dx: 80.0, dy: 0.0 }, now, &mut steps).unwrap();
        job.cancel(&mut window, Some(&mut steps));
        assert_eq!(phases(&steps), [Phase::Move, Phase::Down, Phase::Cancel]);
        window.event(InputEvent::Pointer(PointerEvent::touch(24, Phase::Down, point.0, point.1)));
        job.cancel(&mut window, Some(&mut steps));
        assert_eq!(phases(&steps), [Phase::Move, Phase::Down, Phase::Cancel]);
        assert_eq!(window.held_pointer(), Some((24, PointerKind::Touch)), "old cancellation leaves a person's new touch alone");
        window.event(InputEvent::Pointer(PointerEvent::touch(24, Phase::Cancel, point.0, point.1)));
    }

    fn guard(window: &Window) -> TargetGuard {
        let target = window.inspect().find("target").unwrap().target.clone();
        let request = octara::serde_json::json!({ "op": "double_tap", "id": "target", "target": target });
        octara::inspect::parse_guarded_request(window, &request.to_string()).unwrap().guard.unwrap()
    }

    #[ui]
    fn removing_long_target() -> View {
        let shown = state("shown", || true);
        col()
            .size_full()
            .child(text(move || if shown.get() { "waiting" } else { "recognized" }).id("log"))
            .child(show(move || shown.get(), move || el().id("target").w(120).h(48).on_long_press(move || shown.set(false))))
            .into()
    }

    #[test]
    fn repeated_double_clicks_wait_for_the_real_click_history_to_expire() {
        let mut window = window();
        let start = Instant::now();
        let mut first_steps = Vec::new();
        let mut first = Job::start(&mut window, "target", Kind::DoubleTap, start, &mut first_steps).unwrap_or_else(|r| panic!("{r:?}"));
        assert!(first.delivered());
        assert_eq!(window.app().now(), Duration::ZERO, "starting never advances app time");
        assert_eq!(window.kernel().text_of("log").as_deref(), Some("1,0,0"));
        assert!(
            first.poll(&mut window, start + Duration::from_millis(50), &mut first_steps).is_none(),
            "wall time alone cannot fake the app clock"
        );
        window.app().advance(Duration::from_millis(50));
        assert!(first.poll(&mut window, start + Duration::from_millis(50), &mut first_steps).unwrap().ok);
        assert_eq!(window.kernel().text_of("log").as_deref(), Some("2,1,0"));
        assert_eq!(phases(&first_steps), [Phase::Move, Phase::Down, Phase::Up, Phase::Move, Phase::Down, Phase::Up]);

        let mut second_steps = Vec::new();
        let mut second = Job::start(&mut window, "target", Kind::DoubleTap, start + Duration::from_millis(50), &mut second_steps)
            .unwrap_or_else(|r| panic!("{r:?}"));
        assert!(!second.delivered(), "the preceding click run must expire first");
        assert!(second_steps.is_empty());
        window.app().advance(Duration::from_millis(501));
        assert!(second.poll(&mut window, start + Duration::from_millis(551), &mut second_steps).is_none());
        assert!(second.delivered());
        assert_eq!(phases(&second_steps), [Phase::Move, Phase::Down, Phase::Up]);
        window.app().advance(Duration::from_millis(50));
        assert!(second.poll(&mut window, start + Duration::from_millis(601), &mut second_steps).unwrap().ok);
        assert_eq!(window.kernel().text_of("log").as_deref(), Some("4,2,0"));
        assert_eq!(phases(&second_steps), phases(&first_steps));
    }

    #[test]
    fn a_late_second_click_does_not_claim_a_double_click_or_send_another_press() {
        let mut window = window();
        let start = Instant::now();
        let mut steps = Vec::new();
        let mut job = Job::start(&mut window, "target", Kind::DoubleTap, start, &mut steps).unwrap_or_else(|r| panic!("{r:?}"));
        window.app().advance(Duration::from_millis(501));
        let reply = job.poll(&mut window, start + Duration::from_millis(501), &mut steps).unwrap();
        assert!(!reply.ok);
        assert!(job.delivered());
        assert_eq!(window.kernel().text_of("log").as_deref(), Some("1,0,0"));
        assert_eq!(phases(&steps), [Phase::Move, Phase::Down, Phase::Up]);
    }

    #[test]
    fn a_completed_intervening_click_does_not_turn_a_double_into_an_unreported_triple() {
        let mut window = window();
        let start = Instant::now();
        let mut steps = Vec::new();
        let mut job = Job::start(&mut window, "target", Kind::DoubleTap, start, &mut steps).unwrap_or_else(|r| panic!("{r:?}"));
        window.tap("target").unwrap();
        assert_eq!(window.held_pointer(), None, "the competing click has already lifted");
        window.app().advance(Duration::from_millis(50));
        assert!(!job.poll(&mut window, start + Duration::from_millis(50), &mut steps).unwrap().ok);
        assert_eq!(window.kernel().text_of("log").as_deref(), Some("2,1,0"), "the job must not send a third click");
        assert_eq!(phases(&steps), [Phase::Move, Phase::Down, Phase::Up]);
    }

    #[test]
    fn replacing_a_target_does_not_receive_the_second_click() {
        let mut window = window();
        let start = Instant::now();
        let mut steps = Vec::new();
        let mut job = Job::start(&mut window, "target", Kind::DoubleTap, start, &mut steps).unwrap_or_else(|r| panic!("{r:?}"));
        window.app().hot_reload();
        window.app().advance(Duration::from_millis(50));
        let reply = job.poll(&mut window, start + Duration::from_millis(50), &mut steps).unwrap();
        assert!(!reply.ok);
        assert_eq!(reply.code.as_deref(), Some(codes::STALE_TARGET.id));
        assert_eq!(window.kernel().text_of("log").as_deref(), Some("1,0,0"));
    }

    #[test]
    fn a_name_based_double_click_keeps_the_original_instance_after_rename() {
        let mut window = Window::headless(|| {
            let renamed = state("renamed", || false);
            let hit = state("hit", || "none");
            on_command("rename", move || renamed.set(true));
            col()
                .child(
                    el().id(move || if renamed.get() { "renamed".to_owned() } else { "target".to_owned() })
                        .size_12()
                        .on_double_press(move || hit.set("original")),
                )
                .child(show(move || renamed.get(), move || el().id("target").size_12().on_double_press(move || hit.set("new"))))
                .child(text(move || hit.get().to_owned()).id("hit"))
        });
        let now = Instant::now();
        let mut steps = Vec::new();
        let mut job = Job::start(&mut window, "target", Kind::DoubleTap, now, &mut steps).unwrap();
        window.invoke("rename").unwrap();
        let gap = Duration::from_millis(50);
        window.app().advance(gap);
        let reply = job.poll(&mut window, now + gap, &mut steps).unwrap();
        assert!(reply.ok, "{reply:?}");
        assert_eq!(window.kernel().text_of("hit").as_deref(), Some("original"));
    }

    #[test]
    fn a_guarded_double_click_keeps_the_normal_recognizer_path() {
        let mut window = window();
        let start = Instant::now();
        let guard = guard(&window);
        let mut steps = Vec::new();
        let mut job = Job::start_guarded(&mut window, Kind::DoubleTap, start, guard, &mut steps).unwrap();
        assert_eq!(window.kernel().text_of("log").as_deref(), Some("1,0,0"));
        window.app().advance(Duration::from_millis(50));
        let reply = job.poll(&mut window, start + Duration::from_millis(50), &mut steps).unwrap();
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
        let mut steps = Vec::new();
        let mut job = Job::start_guarded(&mut window, Kind::DoubleTap, start, guard, &mut steps).unwrap();
        assert!(!job.delivered(), "the previous click run still has to expire");
        window.app().hot_reload();
        window.app().advance(Duration::from_millis(501));
        let reply = job.poll(&mut window, start + Duration::from_millis(501), &mut steps).unwrap();
        assert_eq!(reply.code.as_deref(), Some(codes::STALE_TARGET.id));
        assert_eq!(reply.delivery, Some(Delivery::Refused));
        assert!(!job.delivered());
        assert!(steps.is_empty());
        assert_eq!(window.kernel().text_of("log").as_deref(), Some("1,0,0"));
    }

    #[test]
    fn a_guarded_second_click_preserves_the_first_clicks_delivery_after_rebuild() {
        let mut window = window();
        let start = Instant::now();
        let guard = guard(&window);
        let mut steps = Vec::new();
        let mut job = Job::start_guarded(&mut window, Kind::DoubleTap, start, guard, &mut steps).unwrap();
        assert!(job.delivered());
        window.app().hot_reload();
        window.app().advance(Duration::from_millis(50));
        let reply = job.poll(&mut window, start + Duration::from_millis(50), &mut steps).unwrap();
        assert_eq!(reply.code.as_deref(), Some(codes::STALE_TARGET.id));
        assert_eq!(reply.delivery, Some(Delivery::Delivered));
        assert!(job.delivered());
        assert_eq!(phases(&steps), [Phase::Move, Phase::Down, Phase::Up]);
        assert_eq!(window.kernel().text_of("log").as_deref(), Some("1,0,0"));
    }

    #[test]
    fn a_recognized_guarded_long_press_may_remove_its_target() {
        let mut window = Window::headless(removing_long_target);
        let start = Instant::now();
        let guard = guard(&window);
        let mut steps = Vec::new();
        let mut job = Job::start_guarded(&mut window, Kind::LongPress, start, guard, &mut steps).unwrap();
        let hold = window.tuning().long_press;
        window.app().advance(hold);
        assert!(!window.kernel().exists("target"));
        let reply = job.poll(&mut window, start + hold, &mut steps).unwrap();
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
        let mut steps = Vec::new();
        let mut job = Job::start(&mut window, "target", Kind::LongPress, start, &mut steps).unwrap_or_else(|r| panic!("{r:?}"));
        assert!(job.delivered());
        assert_eq!(window.app().now(), Duration::ZERO);
        assert!(window.held_pointer().is_some());
        window.app().advance(Duration::from_millis(500));
        assert!(job.poll(&mut window, start + Duration::from_millis(501), &mut steps).unwrap().ok);
        assert_eq!(window.held_pointer(), None);
        assert_eq!(window.kernel().text_of("log").as_deref(), Some("0,0,1"));
        assert_eq!(phases(&steps), [Phase::Down, Phase::Up]);

        let mut steps = Vec::new();
        let cancelled = Job::start(&mut window, "target", Kind::LongPress, start + Duration::from_millis(501), &mut steps)
            .unwrap_or_else(|r| panic!("{r:?}"));
        cancelled.cancel(&mut window, Some(&mut steps));
        assert_eq!(window.held_pointer(), None);
        assert_eq!(phases(&steps), [Phase::Down, Phase::Cancel]);
        let (_, point) = window.pointer_target("target").unwrap();
        window.event(InputEvent::Pointer(PointerEvent::touch(23, Phase::Down, point.0, point.1)));
        cancelled.cancel(&mut window, None);
        assert_eq!(window.held_pointer(), Some((23, PointerKind::Touch)), "an old job cannot cancel a person's new touch");
        window.event(InputEvent::Pointer(PointerEvent::touch(23, Phase::Cancel, point.0, point.1)));
    }
}

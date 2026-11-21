//! One window's redraw demand, presentation opportunity and drawable lifetime.

use octara::app::WindowId;
use octara::capture::{PaintFrame, PaintOrigin};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const CAPTURE_MAX_WAIT: Duration = Duration::from_millis(50);

#[derive(Clone, Copy)]
pub(crate) enum Opportunity {
    Immediate,
    Redraw,
    Timer(Instant),
    #[cfg(target_os = "macos")]
    DisplayLink {
        tick: super::pace_mac::TickTiming,
        handled_at: Instant,
        expires_at: Instant,
    },
}

impl Opportunity {
    pub(crate) fn valid(self, now: Instant) -> bool {
        self.deadline().is_none_or(|deadline| now < deadline)
    }

    pub(crate) fn deadline(self) -> Option<Instant> {
        match self {
            #[cfg(target_os = "macos")]
            Self::DisplayLink { expires_at, .. } => Some(expires_at),
            _ => None,
        }
    }

    pub(crate) fn target(self) -> Option<Instant> {
        match self {
            #[cfg(target_os = "macos")]
            Self::DisplayLink { tick, .. } => tick.target,
            _ => None,
        }
    }
}

/// Compares an eligible display frame with a capture whose snapshots are ready.
pub(crate) fn capture_precedes_frame(ready_at: Instant, opportunity: Opportunity, now: Instant) -> bool {
    let capture_deadline = ready_at + CAPTURE_MAX_WAIT;
    capture_deadline <= now || opportunity.deadline().is_some_and(|deadline| deadline <= now || capture_deadline < deadline)
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DisplayTick {
    Offered,
    Repeated,
    Rest,
}

/// Demand survives an expired opportunity and a failed draw. Starting to draw consumes
/// the current demand and opportunity, leaving any later requests for the next frame.
pub(crate) struct FrameSchedule {
    pending: bool,
    continuous: bool,
    opportunity: Option<Opportunity>,
    attempted: Option<Instant>,
    #[cfg(target_os = "macos")]
    display_target: Option<f64>,
}

impl FrameSchedule {
    fn new() -> Self {
        Self {
            pending: true,
            continuous: false,
            opportunity: None,
            attempted: None,
            #[cfg(target_os = "macos")]
            display_target: None,
        }
    }

    pub(crate) fn request(&mut self) {
        self.pending = true;
    }

    pub(crate) fn publish(&mut self, changed: bool, continuous: bool) {
        self.pending |= changed;
        self.continuous = continuous;
    }

    pub(crate) fn pending(&self) -> bool {
        self.pending
    }

    pub(crate) fn wants_frame(&self) -> bool {
        self.pending || self.continuous
    }

    pub(crate) fn continuous(&self) -> bool {
        self.continuous
    }

    pub(crate) fn attempted(&self) -> Option<Instant> {
        self.attempted
    }

    pub(crate) fn redraw(&mut self, now: Instant) {
        self.request();
        if self.opportunity(now).is_none() {
            self.retain(Opportunity::Redraw);
        }
    }

    pub(crate) fn retain(&mut self, opportunity: Opportunity) {
        self.opportunity = Some(opportunity);
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn display_tick(&mut self, opportunity: Opportunity) -> DisplayTick {
        if let Opportunity::DisplayLink { tick, .. } = opportunity
            && let Some(target) = tick.target_media_time
        {
            if self.display_target.is_some_and(|previous| target <= previous) {
                return DisplayTick::Repeated;
            }
            self.display_target = Some(target);
        }
        // Give a publication just after a quiet tick the rest of its presentation interval.
        // A second unused tick lets this window sleep.
        if !self.wants_frame() && self.opportunity.is_some() {
            self.pause();
            return DisplayTick::Rest;
        }
        self.retain(opportunity);
        DisplayTick::Offered
    }

    pub(crate) fn opportunity(&self, now: Instant) -> Option<Opportunity> {
        self.opportunity.filter(|opportunity| opportunity.valid(now))
    }

    pub(crate) fn begin_draw(&mut self, now: Instant) {
        self.pending = false;
        self.opportunity = None;
        self.attempted = Some(now);
    }

    pub(crate) fn retry(&mut self, now: Instant) {
        self.request();
        self.opportunity = None;
        self.attempted = Some(now);
    }

    pub(crate) fn pause(&mut self) {
        self.opportunity = None;
        #[cfg(target_os = "macos")]
        {
            self.display_target = None;
        }
    }

    pub(crate) fn restart(&mut self) {
        *self = Self { continuous: self.continuous, ..Self::new() };
    }

    pub(crate) fn cancel(&mut self) {
        self.pending = false;
        self.continuous = false;
        self.pause();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DrawableGeneration {
    pub(crate) surface: u64,
    pub(crate) device: u64,
}

impl DrawableGeneration {
    fn frame_number(self, paint: &PaintFrame) -> Option<u32> {
        let PaintOrigin::Submitted { frame, surface_generation, device_generation } = paint.origin else { return None };
        (self.surface == surface_generation && self.device == device_generation).then_some(frame)
    }
}

/// Identifies the request independently of the drawing available when it was issued.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Acquisition {
    window: WindowId,
    generation: DrawableGeneration,
    serial: u64,
}

#[cfg(target_os = "macos")]
impl Acquisition {
    pub(crate) fn window(self) -> WindowId {
        self.window
    }

    pub(crate) fn serial(self) -> u64 {
        self.serial
    }
}

struct InFlight {
    request: Acquisition,
    invalidated: bool,
}

pub(crate) struct Presentation {
    window: WindowId,
    pub(crate) schedule: FrameSchedule,
    in_flight: Option<InFlight>,
    submitted: Option<PaintFrame>,
}

impl Presentation {
    pub(crate) fn new(window: WindowId) -> Self {
        Self { window, schedule: FrameSchedule::new(), in_flight: None, submitted: None }
    }

    pub(crate) fn waiting(&self) -> bool {
        self.in_flight.is_some()
    }

    pub(crate) fn begin(&mut self, generation: DrawableGeneration) -> Option<Acquisition> {
        if self.waiting() {
            return None;
        }
        // Request identities also distinguish two host windows with the same app-local ID.
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let serial =
            NEXT.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1)).expect("drawable request identity exhausted");
        let request = Acquisition { window: self.window, generation, serial };
        self.in_flight = Some(InFlight { request, invalidated: false });
        Some(request)
    }

    /// Retires the matching request and tells the host whether its result belongs to the live
    /// surface. Release any discarded drawable before using the freed slot or reconfiguring.
    pub(crate) fn finish(&mut self, request: Acquisition, current: DrawableGeneration) -> bool {
        let Some(active) = self.in_flight.take_if(|active| active.request == request) else { return false };
        !active.invalidated && request.generation == current
    }

    /// The result is obsolete. Keep its slot occupied until the acquisition actually returns.
    pub(crate) fn invalidate(&mut self) {
        if let Some(active) = &mut self.in_flight {
            active.invalidated = true;
        }
    }

    pub(crate) fn clear_submission(&mut self) {
        self.submitted = None;
    }

    /// May describe an earlier drawing while a newer drawing is waiting for a drawable.
    pub(crate) fn submitted_paint(&self, current: DrawableGeneration) -> Option<&PaintFrame> {
        self.submitted.as_ref().filter(|paint| current.frame_number(paint).is_some())
    }

    /// Records a successful, admitted handoff. A repeat of the same drawing can produce a new
    /// submission, such as when the host redraws an exposed window.
    pub(crate) fn record_submission(&mut self, paint: PaintFrame, current: DrawableGeneration) -> bool {
        if paint.window != self.window.number() {
            return false;
        }
        let Some(frame) = current.frame_number(&paint) else { return false };
        if self.submitted.as_ref().is_some_and(|previous| {
            current.frame_number(previous).is_some_and(|number| frame <= number || paint.sample.revision < previous.sample.revision)
        }) {
            return false;
        }
        self.submitted = Some(paint);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use octara::capture::PaintSample;

    const GENERATION: DrawableGeneration = DrawableGeneration { surface: 3, device: 4 };

    fn window() -> WindowId {
        WindowId::from_number(1)
    }

    fn receipt(frame: u32, revision: u64, generation: DrawableGeneration) -> PaintFrame {
        PaintFrame {
            window: window().number(),
            sample: PaintSample { revision, sampled_at: Duration::from_millis(revision) },
            origin: PaintOrigin::Submitted { frame, surface_generation: generation.surface, device_generation: generation.device },
            effects: Vec::new(),
        }
    }

    #[cfg(target_os = "macos")]
    fn display(at: Instant) -> Opportunity {
        let target = at + Duration::from_micros(8_333);
        Opportunity::DisplayLink {
            tick: super::super::pace_mac::TickTiming { target: Some(target), ..Default::default() },
            handled_at: at,
            expires_at: target,
        }
    }

    #[test]
    fn a_capture_that_has_waited_50_ms_precedes_frames_due_now() {
        let ready = Instant::now();
        let before_deadline = ready + Duration::from_millis(49);
        let at_deadline = ready + Duration::from_millis(50);
        let later = ready + Duration::from_millis(51);
        for frame in [Opportunity::Immediate, Opportunity::Redraw, Opportunity::Timer(later)] {
            assert!(!capture_precedes_frame(ready, frame, before_deadline));
            assert!(capture_precedes_frame(ready, frame, at_deadline));
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_capture_precedes_an_expired_display_opportunity() {
        let ready = Instant::now();
        let frame = display(ready);
        assert!(!capture_precedes_frame(ready, frame, ready));
        assert!(capture_precedes_frame(ready, frame, frame.deadline().unwrap()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn an_earlier_capture_deadline_precedes_a_valid_display_opportunity() {
        let ready = Instant::now();
        let now = ready + Duration::from_millis(45);
        let frame = display(now);
        assert!(capture_precedes_frame(ready, frame, now));
        let same_deadline = frame.deadline().unwrap() - CAPTURE_MAX_WAIT;
        assert!(!capture_precedes_frame(same_deadline, frame, now));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_publication_just_after_a_tick_can_use_the_unused_opportunity() {
        let now = Instant::now();
        let early_tick = now + Duration::from_millis(8);
        let mut schedule = FrameSchedule::new();
        schedule.begin_draw(now);
        assert!(!schedule.wants_frame());

        assert_eq!(schedule.display_tick(display(early_tick)), DisplayTick::Offered);
        assert!(!schedule.wants_frame());
        let published = early_tick + Duration::from_millis(1);
        schedule.publish(true, false);
        schedule.redraw(published);
        assert!(schedule.wants_frame());
        assert_eq!(schedule.opportunity(published).unwrap().target(), display(early_tick).target());
        schedule.begin_draw(published);
        assert!(schedule.opportunity(published).is_none());

        let quiet = early_tick + Duration::from_millis(9);
        assert_eq!(schedule.display_tick(display(quiet)), DisplayTick::Offered);
        assert_eq!(schedule.display_tick(display(quiet + Duration::from_millis(9))), DisplayTick::Rest);
        assert!(!schedule.wants_frame() && schedule.opportunity(quiet).is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn late_drawable_keeps_demand_but_only_a_new_tick_can_admit_it() {
        let now = Instant::now();
        let late = now + Duration::from_millis(10);
        let mut a = Presentation::new(window());
        let mut b = Presentation::new(WindowId::from_number(2));
        assert_eq!(a.schedule.display_tick(display(now)), DisplayTick::Offered);
        let drawable = a.begin(GENERATION).unwrap();
        a.schedule.publish(true, true);
        assert!(a.waiting());

        assert_eq!(b.schedule.display_tick(display(late)), DisplayTick::Offered);
        assert!(b.schedule.opportunity(late).is_some());
        b.schedule.begin_draw(late);
        assert!(a.waiting());

        assert!(a.finish(drawable, GENERATION));
        assert!(a.schedule.wants_frame());
        assert!(a.schedule.opportunity(late).is_none());
        assert_eq!(a.schedule.display_tick(display(late)), DisplayTick::Offered);
        assert_eq!(a.schedule.opportunity(late).unwrap().target(), display(late).target());
        a.schedule.begin_draw(late);
        assert!(a.schedule.wants_frame(), "motion still needs the following display tick");
        assert!(a.schedule.opportunity(late).is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn one_native_target_cannot_admit_a_second_frame() {
        let now = Instant::now();
        let at = |clock, handled_at| Opportunity::DisplayLink {
            tick: super::super::pace_mac::TickTiming { target_media_time: Some(clock), ..Default::default() },
            handled_at,
            expires_at: now + Duration::from_millis(16),
        };
        let mut schedule = FrameSchedule::new();
        assert_eq!(schedule.display_tick(at(20.0, now)), DisplayTick::Offered);
        schedule.begin_draw(now);
        schedule.request();
        assert_eq!(schedule.display_tick(at(20.0, now + Duration::from_micros(10))), DisplayTick::Repeated);
        assert!(schedule.opportunity(now).is_none());
        assert!(schedule.wants_frame());
        assert_eq!(schedule.display_tick(at(20.008, now + Duration::from_millis(8))), DisplayTick::Offered);
    }

    #[test]
    fn a_request_during_recording_survives_the_earlier_submission() {
        let now = Instant::now();
        let mut presentation = Presentation::new(window());
        presentation.schedule.begin_draw(now);
        assert!(!presentation.schedule.pending());
        presentation.schedule.redraw(now);
        assert!(presentation.record_submission(receipt(1, 3, GENERATION), GENERATION));
        assert!(presentation.schedule.pending());
        assert!(matches!(presentation.schedule.opportunity(now), Some(Opportunity::Redraw)));

        presentation.schedule.begin_draw(now);
        presentation.schedule.retry(now);
        assert!(presentation.schedule.pending());
        assert!(presentation.schedule.opportunity(now).is_none());
    }

    struct DelayedHost {
        presentation: Presentation,
        latest: u64,
        frames: u32,
    }

    impl DelayedHost {
        fn pending(&self) -> bool {
            self.presentation.submitted_paint(GENERATION).map(|paint| paint.sample.revision) != Some(self.latest)
        }

        fn ready(&mut self, request: Acquisition, acquired: bool, submitted: bool) {
            if self.presentation.finish(request, GENERATION) && acquired && submitted {
                let frame = self.frames + 1;
                assert!(self.presentation.record_submission(receipt(frame, self.latest, GENERATION), GENERATION));
                self.frames = frame;
            }
        }
    }

    #[test]
    fn delayed_acquisition_consumes_the_latest_drawing_and_failures_keep_demand() {
        let mut host = DelayedHost { presentation: Presentation::new(window()), latest: 1, frames: 0 };
        for (acquired, submitted) in [(false, false), (true, false)] {
            let request = host.presentation.begin(GENERATION).unwrap();
            assert!(host.presentation.begin(GENERATION).is_none());
            host.ready(request, acquired, submitted);
            assert!(host.pending());
            assert_eq!(host.frames, 0);
        }

        let request = host.presentation.begin(GENERATION).unwrap();
        host.latest = 3;
        host.ready(request, true, true);
        assert!(!host.pending());
        host.latest = 4;
        assert!(host.pending());
        assert_eq!(host.presentation.submitted_paint(GENERATION), Some(&receipt(1, 3, GENERATION)));
    }

    #[test]
    fn an_invalidated_request_keeps_the_slot_until_its_result_returns() {
        let mut presentation = Presentation::new(window());
        let retired = presentation.begin(GENERATION).unwrap();
        presentation.invalidate();
        assert!(presentation.waiting());
        assert!(presentation.begin(GENERATION).is_none());
        assert!(!presentation.finish(retired, GENERATION));
        assert!(!presentation.waiting());

        let live = presentation.begin(GENERATION).unwrap();
        assert!(!presentation.finish(retired, GENERATION));
        assert!(presentation.waiting());
        assert!(presentation.finish(live, GENERATION));
    }

    #[test]
    fn resize_and_device_replacement_reject_stale_acquisitions() {
        let mut presentation = Presentation::new(window());
        for current in [DrawableGeneration { surface: 4, ..GENERATION }, DrawableGeneration { device: 5, ..GENERATION }] {
            let old = presentation.begin(GENERATION).unwrap();
            assert!(presentation.begin(current).is_none());
            assert!(!presentation.finish(old, current));
            let live = presentation.begin(current).unwrap();
            assert!(!presentation.finish(old, current));
            assert!(presentation.finish(live, current));
        }
    }

    #[test]
    fn a_closed_hosts_completion_cannot_retire_its_successors_request() {
        let late = Presentation::new(window()).begin(GENERATION).unwrap();
        let mut reopened = Presentation::new(window());
        let live = reopened.begin(GENERATION).unwrap();
        assert!(!reopened.finish(late, GENERATION));
        assert!(reopened.finish(live, GENERATION));
    }

    #[test]
    fn submission_requires_a_compatible_generation_and_forward_progress() {
        let mut presentation = Presentation::new(window());
        assert!(presentation.record_submission(receipt(6, 7, GENERATION), GENERATION));
        assert!(!presentation.record_submission(receipt(5, 8, GENERATION), GENERATION));
        assert!(!presentation.record_submission(receipt(7, 6, GENERATION), GENERATION));
        assert!(presentation.record_submission(receipt(7, 7, GENERATION), GENERATION));
        let mut rerecorded = receipt(8, 8, GENERATION);
        rerecorded.origin = PaintOrigin::Rerecorded { device_generation: Some(GENERATION.device) };
        assert!(!presentation.record_submission(rerecorded, GENERATION));

        let current = DrawableGeneration { device: 5, ..GENERATION };
        assert!(presentation.submitted_paint(current).is_none());
        assert!(presentation.record_submission(receipt(8, 7, current), current));
        assert!(!presentation.record_submission(receipt(9, 8, GENERATION), current));
        assert_eq!(presentation.submitted_paint(current), Some(&receipt(8, 7, current)));
        presentation.clear_submission();
        assert!(presentation.submitted_paint(current).is_none());
    }

    #[test]
    fn submission_rejects_a_receipt_from_another_window() {
        let mut presentation = Presentation::new(window());
        assert!(presentation.record_submission(receipt(1, 1, GENERATION), GENERATION));
        let mut foreign = receipt(2, 2, GENERATION);
        foreign.window = WindowId::from_number(2).number();
        assert!(!presentation.record_submission(foreign, GENERATION));
        assert_eq!(presentation.submitted_paint(GENERATION), Some(&receipt(1, 1, GENERATION)));
    }
}

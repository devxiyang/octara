//! A window's frame clock on macOS: the view's display link (macOS 14), on the main run loop,
//! following the screen the window is on. One queued wake delivers the newest timing.
//! The window's presentation schedule decides when to use that timing.
//!
//! As Rune found, Metal's drawable pool is no frame clock (acquiring a drawable may return at
//! once, and does while the layer presents with its transaction), so frames within a burst are
//! drawn on the link's ticks; a frame after rest is drawn at once by the shell. Low Power Mode
//! caps the panel at 60 Hz while the link reports the panel's rate, so ticks closer than a 60 Hz
//! frame are skipped then.

use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::NSView;
use objc2_foundation::{NSProcessInfo, NSRunLoop, NSRunLoopCommonModes};
use objc2_quartz_core::{CACurrentMediaTime, CADisplayLink};
use std::cell::Cell;
use std::time::{Duration, Instant};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

/// What a tick runs.
struct Tick {
    on_tick: Box<dyn Fn() -> bool>,
    queue: Cell<TickQueue>,
    trace: bool,
}

impl Tick {
    fn enqueue(&self, sample: impl FnOnce(TickTiming) -> TickTiming) {
        let mut queue = self.queue.get();
        let Some(wake) = queue.enqueue(sample) else { return };
        self.queue.set(queue);
        if wake && !(self.on_tick)() {
            let mut queue = self.queue.get();
            queue.acknowledge_wake();
            queue.take_timing();
            self.queue.set(queue);
        }
    }
}

#[derive(Clone, Copy)]
struct TickQueue {
    running: bool,
    /// An event sent to the host stays queued until the host takes it.
    wake_queued: bool,
    /// The newest timing the queued event can deliver. Pausing clears it.
    timing: Option<TickTiming>,
}

impl TickQueue {
    fn new() -> Self {
        Self { running: false, wake_queued: false, timing: None }
    }

    fn set_running(&mut self, running: bool) {
        self.running = running;
        if !running {
            self.timing = None;
        }
    }

    /// A callback needs a new wake only when the host has taken the previous one.
    fn enqueue(&mut self, sample: impl FnOnce(TickTiming) -> TickTiming) -> Option<bool> {
        if !self.running {
            return None;
        }
        self.timing = Some(sample(self.timing.unwrap_or_default()));
        Some(!std::mem::replace(&mut self.wake_queued, true))
    }

    fn acknowledge_wake(&mut self) -> bool {
        self.wake_queued = false;
        self.timing.is_some()
    }

    fn take_timing(&mut self) -> Option<TickTiming> {
        self.timing.take()
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct TickTiming {
    pub(crate) queued_at: Option<Instant>,
    pub(crate) callback_at: Option<Instant>,
    pub(crate) target: Option<Instant>,
    /// Display opportunity identity in Core Animation's clock, independent of conversion
    /// jitter when a display link repeats its target.
    pub(crate) target_media_time: Option<f64>,
    pub(crate) coalesced: u32,
}

impl TickTiming {
    fn sample(self, link: &CADisplayLink, trace: bool) -> Self {
        let media_target = link.targetTimestamp();
        let callback_at = Instant::now();
        // Core Animation's target and media clock share an epoch. Pair the media clock
        // with this Instant so the target can be compared with host submission timestamps.
        let offset = media_target - CACurrentMediaTime();
        let target = (media_target > 0.0)
            .then(|| Duration::try_from_secs_f64(offset.abs()).ok())
            .flatten()
            .and_then(|delta| if offset >= 0.0 { callback_at.checked_add(delta) } else { callback_at.checked_sub(delta) });
        Self {
            queued_at: self.queued_at.or(trace.then_some(callback_at)),
            callback_at: trace.then_some(callback_at),
            target,
            target_media_time: target.map(|_| media_target),
            coalesced: self.coalesced.saturating_add(u32::from(self.queued_at.is_some())),
        }
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraFrameClock"]
    #[ivars = Tick]
    struct Target;

    impl Target {
        #[unsafe(method(tick:))]
        fn tick(&self, link: &CADisplayLink) {
            let tick = self.ivars();
            tick.enqueue(|timing| timing.sample(link, tick.trace));
        }
    }
);

/// A window's display link.
pub(crate) struct DisplayLink {
    link: Retained<CADisplayLink>,
    target: Retained<Target>,
}

impl DisplayLink {
    /// The link of `window`'s view, initially paused. `on_tick` queues a frame event when it
    /// can and returns whether that event was sent.
    pub(crate) fn new(window: &Window, trace: bool, on_tick: impl Fn() -> bool + 'static) -> Option<DisplayLink> {
        let mtm = MainThreadMarker::new()?;
        let RawWindowHandle::AppKit(handle) = window.window_handle().ok()?.as_raw() else { return None };
        // SAFETY: winit's content view, alive as long as the window.
        let view: Retained<NSView> = unsafe { Retained::retain(handle.ns_view.as_ptr().cast()) }?;
        if !view.respondsToSelector(sel!(displayLinkWithTarget:selector:)) {
            return None;
        }
        let target = Target::alloc(mtm).set_ivars(Tick { on_tick: Box::new(on_tick), queue: Cell::new(TickQueue::new()), trace });
        let target: Retained<Target> = unsafe { msg_send![super(target), init] };
        // SAFETY: the target answers `tick:`, taking the link.
        let link = unsafe { view.displayLinkWithTarget_selector(&target, sel!(tick:)) };
        link.setPaused(true);
        // SAFETY: the main run loop, in the common modes (ticks go on while a window resizes).
        unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes) };
        Some(DisplayLink { link, target })
    }

    pub(crate) fn running(&self) -> bool {
        self.target.ivars().queue.get().running
    }

    pub(crate) fn has_queued_tick(&self) -> bool {
        self.target.ivars().queue.get().timing.is_some()
    }

    pub(crate) fn take_tick(&self) -> Option<TickTiming> {
        let tick = self.target.ivars();
        let mut queue = tick.queue.get();
        let timing = queue.take_timing()?;
        tick.queue.set(queue);
        Some(timing)
    }

    /// Consumes this link's queued host wake. Another window's scheduling pass may already
    /// have collected the timing it carried.
    pub(crate) fn acknowledge_wake(&self) -> bool {
        let tick = self.target.ivars();
        let mut queue = tick.queue.get();
        let pending = queue.acknowledge_wake();
        tick.queue.set(queue);
        pending
    }

    /// Ticks from the next refresh on.
    pub(crate) fn start(&self) {
        self.set_running(true);
    }

    pub(crate) fn stop(&self) {
        self.set_running(false);
    }

    fn set_running(&self, running: bool) {
        let tick = self.target.ivars();
        let mut queue = tick.queue.get();
        if queue.running != running {
            queue.set_running(running);
            tick.queue.set(queue);
            self.link.setPaused(!running);
        }
    }
}

impl Drop for DisplayLink {
    fn drop(&mut self) {
        self.link.invalidate();
    }
}

/// In Low Power Mode, keep at least one 60 Hz interval between submission handoffs.
pub(crate) fn tick_draws(last_submission: Option<Instant>) -> bool {
    let capped = NSProcessInfo::processInfo().isLowPowerModeEnabled();
    !capped || last_submission.is_none_or(|t| t.elapsed() >= Duration::from_micros(15_500))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesces_admitted_timing_into_one_wake() {
        let mut queue = TickQueue::new();
        queue.set_running(true);
        let first = Instant::now();
        let latest = first + Duration::from_millis(8);
        assert_eq!(queue.enqueue(|_| TickTiming { queued_at: Some(first), ..TickTiming::default() }), Some(true));
        assert_eq!(queue.enqueue(|timing| TickTiming { callback_at: Some(latest), ..timing }), Some(false));
        assert!(queue.acknowledge_wake());
        let timing = queue.take_timing().unwrap();
        assert_eq!(timing.queued_at, Some(first));
        assert_eq!(timing.callback_at, Some(latest));
        assert_eq!(queue.enqueue(|timing| timing), Some(true));
    }

    #[test]
    fn restart_can_reuse_an_old_wake_with_fresh_timing() {
        let mut queue = TickQueue::new();
        queue.set_running(true);
        let old = Instant::now();
        let fresh = old + Duration::from_millis(8);
        assert_eq!(queue.enqueue(|_| TickTiming { queued_at: Some(old), ..TickTiming::default() }), Some(true));
        queue.set_running(false);
        assert_eq!(queue.enqueue(|_| panic!("sampled a callback while paused")), None);

        queue.set_running(true);
        assert_eq!(queue.enqueue(|timing| TickTiming { queued_at: timing.queued_at.or(Some(fresh)), ..timing }), Some(false));
        assert!(queue.acknowledge_wake());
        assert_eq!(queue.take_timing().unwrap().queued_at, Some(fresh));
    }

    #[test]
    fn paused_wake_delivers_no_timing_and_can_be_replaced() {
        let mut queue = TickQueue::new();
        queue.set_running(true);
        assert_eq!(queue.enqueue(|timing| timing), Some(true));
        queue.set_running(false);
        assert_eq!(queue.enqueue(|_| panic!("sampled a callback while paused")), None);
        assert!(!queue.acknowledge_wake());
        assert!(queue.take_timing().is_none());

        queue.set_running(true);
        assert_eq!(queue.enqueue(|timing| timing), Some(true));
    }

    #[test]
    fn collecting_another_windows_timing_keeps_its_wake_queued() {
        let mut queue = TickQueue::new();
        queue.set_running(true);
        assert_eq!(queue.enqueue(|timing| timing), Some(true));
        assert!(queue.take_timing().is_some());
        assert_eq!(queue.enqueue(|timing| timing), Some(false));
        assert!(queue.acknowledge_wake());
        assert!(queue.take_timing().is_some());

        assert_eq!(queue.enqueue(|timing| timing), Some(true));
        assert!(queue.take_timing().is_some());
        assert!(!queue.acknowledge_wake());
    }

    #[test]
    fn failed_event_send_releases_the_slot_for_the_next_callback() {
        let mut queue = TickQueue::new();
        queue.set_running(true);
        let attempts = std::rc::Rc::new(Cell::new(0));
        let count = attempts.clone();
        let tick = Tick {
            on_tick: Box::new(move || {
                count.set(count.get() + 1);
                count.get() == 2
            }),
            queue: Cell::new(queue),
            trace: false,
        };
        tick.enqueue(|timing| timing);
        assert!(tick.queue.get().timing.is_none());
        tick.enqueue(|timing| timing);
        assert_eq!(attempts.get(), 2);
        assert!(tick.queue.get().timing.is_some());
    }
}

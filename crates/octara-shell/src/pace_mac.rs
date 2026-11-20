//! A window's frame clock on macOS: the view's display link (macOS 14), on the main run loop,
//! following the screen the window is on. It ticks for pending or continuous drawing and
//! pauses when the window is idle.
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
use objc2_quartz_core::CADisplayLink;
use std::cell::Cell;
use std::time::{Duration, Instant};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

/// What a tick runs.
struct Tick {
    on_tick: Box<dyn Fn() -> bool>,
    queued: Cell<bool>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraFrameClock"]
    #[ivars = Tick]
    struct Target;

    impl Target {
        #[unsafe(method(tick:))]
        fn tick(&self, _link: &CADisplayLink) {
            let tick = self.ivars();
            // One queued opportunity is enough. The event loop samples the clock when it
            // handles the event so delayed opportunities do not build a backlog of old frames.
            if !tick.queued.replace(true) && !(tick.on_tick)() {
                tick.queued.set(false);
            }
        }
    }
);

/// A window's display link.
pub(crate) struct DisplayLink {
    link: Retained<CADisplayLink>,
    target: Retained<Target>,
    running: Cell<bool>,
}

impl DisplayLink {
    /// The link of `window`'s view, initially paused. `on_tick` queues a frame event when it
    /// can and returns whether that event was sent.
    pub(crate) fn new(window: &Window, on_tick: impl Fn() -> bool + 'static) -> Option<DisplayLink> {
        let mtm = MainThreadMarker::new()?;
        let RawWindowHandle::AppKit(handle) = window.window_handle().ok()?.as_raw() else { return None };
        // SAFETY: winit's content view, alive as long as the window.
        let view: Retained<NSView> = unsafe { Retained::retain(handle.ns_view.as_ptr().cast()) }?;
        if !view.respondsToSelector(sel!(displayLinkWithTarget:selector:)) {
            return None;
        }
        let target = Target::alloc(mtm).set_ivars(Tick { on_tick: Box::new(on_tick), queued: Cell::new(false) });
        let target: Retained<Target> = unsafe { msg_send![super(target), init] };
        // SAFETY: the target answers `tick:`, taking the link.
        let link = unsafe { view.displayLinkWithTarget_selector(&target, sel!(tick:)) };
        link.setPaused(true);
        // SAFETY: the main run loop, in the common modes (ticks go on while a window resizes).
        unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes) };
        Some(DisplayLink { link, target, running: Cell::new(false) })
    }

    pub(crate) fn running(&self) -> bool {
        self.running.get()
    }

    pub(crate) fn take_tick(&self) -> bool {
        self.target.ivars().queued.replace(false) && self.running()
    }

    /// Ticks from the next refresh on.
    pub(crate) fn start(&self) {
        if !self.running.replace(true) {
            self.link.setPaused(false);
        }
    }

    pub(crate) fn stop(&self) {
        if self.running.replace(false) {
            self.link.setPaused(true);
        }
    }
}

impl Drop for DisplayLink {
    fn drop(&mut self) {
        self.link.invalidate();
    }
}

/// Whether a tick should draw, the last frame shown at `presented`: not closer than a 60 Hz
/// frame in Low Power Mode.
pub(crate) fn tick_draws(presented: Option<Instant>) -> bool {
    let capped = NSProcessInfo::processInfo().isLowPowerModeEnabled();
    !capped || presented.is_none_or(|t| t.elapsed() >= Duration::from_micros(15_500))
}

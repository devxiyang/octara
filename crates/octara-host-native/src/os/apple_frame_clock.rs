//! Display-link timing and refresh-rate policy shared by AppKit and UIKit.

use super::{DrawableAcquisition, FrameClockActivity, FrameTick};
use super::{PlatformEvent, PlatformSender};
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSView as DisplayView, NSWindowOcclusionState};
use objc2_foundation::{NSRunLoop, NSRunLoopCommonModes};
use objc2_quartz_core::{CACurrentMediaTime, CADisplayLink, CAFrameRateRange};
#[cfg(target_os = "ios")]
use objc2_ui_kit::UIView as DisplayView;
use octara::runtime::WindowId;
use std::cell::Cell;
#[cfg(target_os = "macos")]
use std::cell::RefCell;
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
mod core_video;
#[cfg(target_os = "macos")]
use self::core_video::VideoClock;

const IDLE_GRACE: Duration = Duration::from_millis(120);

#[derive(Clone, Copy)]
enum ClockState {
    Paused,
    Idle(Instant),
    Low,
    Interactive,
}

struct ClockIvars {
    view: Retained<DisplayView>,
    window: WindowId,
    proxy: PlatformSender,
    state: Cell<ClockState>,
    range: Cell<Option<CAFrameRateRange>>,
}

define_class!(
    // SAFETY: The link calls its target on the main run loop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraDisplayClockTarget"]
    #[ivars = ClockIvars]
    struct Target;

    unsafe impl NSObjectProtocol for Target {}

    impl Target {
        #[unsafe(method(displayTick:))]
        fn display_tick(&self, link: &CADisplayLink) {
            let clock = self.ivars();
            let now = Instant::now();
            match clock.state.get() {
                ClockState::Paused => return,
                ClockState::Idle(deadline) if now >= deadline => {
                    clock.state.set(ClockState::Paused);
                    link.setPaused(true);
                    return;
                }
                _ => {}
            }

            let target_time = link.targetTimestamp();
            let target = instant_from_media_time(now, target_time);
            let interval = Duration::try_from_secs_f64(target_time - link.timestamp())
                .ok()
                .filter(|interval| !interval.is_zero())
                .unwrap_or_else(|| Duration::from_secs_f64(1.0 / f64::from(maximum_frames_per_second(&clock.view))));
            // Read the current display capability for the requested range.
            clock.update_rate(link);
            clock.proxy.send_event(PlatformEvent::FrameClockTick { window: clock.window, tick: FrameTick { target, interval } });
        }
    }
);

impl ClockIvars {
    fn update_rate(&self, link: &CADisplayLink) {
        let state = self.state.get();
        if matches!(state, ClockState::Paused) {
            return;
        }
        let maximum = maximum_frames_per_second(&self.view);
        let next = if matches!(state, ClockState::Interactive) {
            CAFrameRateRange { minimum: 80.0_f32.min(maximum), maximum, preferred: maximum }
        } else {
            CAFrameRateRange { minimum: 24.0_f32.min(maximum), maximum: 40.0_f32.min(maximum), preferred: 30.0_f32.min(maximum) }
        };
        if self.range.get() != Some(next) {
            link.setPreferredFrameRateRange(next);
            self.range.set(Some(next));
        }
    }
}

fn instant_from_media_time(now: Instant, time: f64) -> Instant {
    let delta = time - CACurrentMediaTime();
    Duration::try_from_secs_f64(delta.abs())
        .ok()
        .and_then(|offset| if delta >= 0.0 { now.checked_add(offset) } else { now.checked_sub(offset) })
        .unwrap_or(now)
}

pub(super) struct DisplayClock {
    link: Retained<CADisplayLink>,
    target: Retained<Target>,
    shown: Cell<bool>,
    activity: Cell<FrameClockActivity>,
    #[cfg(target_os = "macos")]
    video: RefCell<Option<VideoClock>>,
}

impl DisplayClock {
    pub(super) fn new(view: &Retained<DisplayView>, window: WindowId, proxy: PlatformSender) -> Self {
        let this = Target::alloc(view.mtm()).set_ivars(ClockIvars {
            view: view.clone(),
            window,
            proxy,
            state: Cell::new(ClockState::Paused),
            range: Cell::default(),
        });
        // SAFETY: The allocated target has initialized ivars and NSObject as its superclass.
        let target: Retained<Target> = unsafe { msg_send![super(this), init] };
        let link = create_link(view, &target, sel!(displayTick:));
        link.setPaused(true);
        // SAFETY: The link and target are registered on their owning main run loop.
        unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes) };
        Self {
            link,
            target,
            shown: Cell::default(),
            activity: Cell::new(FrameClockActivity::Paused),
            #[cfg(target_os = "macos")]
            video: RefCell::default(),
        }
    }

    pub(super) fn externally_paced(&self) -> bool {
        self.shown.get()
    }

    pub(super) fn did_show(&self) {
        self.shown.set(true);
        #[cfg(target_os = "macos")]
        self.screen_changed();
        self.apply_activity(self.activity.get());
    }

    pub(super) fn set_activity(&self, activity: FrameClockActivity) {
        if self.activity.replace(activity) == activity {
            return;
        }
        if self.shown.get() {
            self.apply_activity(activity);
        }
    }

    #[cfg(target_os = "macos")]
    pub(super) fn screen_changed(&self) {
        if !self.shown.get() {
            return;
        }
        drop(self.video.take());
        let clock = self.target.ivars();
        if let Some(screen) = clock.view.window().and_then(|window| window.screen()) {
            self.video.replace(VideoClock::new(&screen, clock.window, clock.proxy.clone()));
        }
        self.update_driver();
    }

    fn apply_activity(&self, activity: FrameClockActivity) {
        let clock = self.target.ivars();
        let next = match activity {
            FrameClockActivity::Paused => ClockState::Paused,
            FrameClockActivity::Idle => match clock.state.get() {
                state @ (ClockState::Paused | ClockState::Idle(_)) => state,
                _ => ClockState::Idle(Instant::now() + IDLE_GRACE),
            },
            FrameClockActivity::Low => ClockState::Low,
            FrameClockActivity::Interactive => ClockState::Interactive,
        };
        clock.state.set(next);
        self.update_driver();
    }

    pub(super) fn update_driver(&self) {
        let clock = self.target.ivars();
        // Keep AppKit's low-rate and idle behavior. Core Video emits the full hardware cadence.
        #[cfg(target_os = "macos")]
        let video_running = {
            let interactive = matches!(clock.state.get(), ClockState::Interactive)
                && clock.view.window().is_some_and(|window| window.occlusionState().contains(NSWindowOcclusionState::Visible));
            self.video.borrow().as_ref().is_some_and(|video| video.set_running(interactive))
        };
        #[cfg(target_os = "ios")]
        let video_running = false;
        clock.update_rate(&self.link);
        let paused = video_running || matches!(clock.state.get(), ClockState::Paused);
        if self.link.isPaused() != paused {
            self.link.setPaused(paused);
        }
    }
}

impl Drop for DisplayClock {
    fn drop(&mut self) {
        self.link.invalidate();
    }
}

#[cfg(target_os = "macos")]
fn create_link(view: &DisplayView, target: &Target, selector: Sel) -> Retained<CADisplayLink> {
    // SAFETY: The selector takes the display link and is implemented by Target.
    unsafe { view.displayLinkWithTarget_selector(target, selector) }
}

#[cfg(target_os = "ios")]
fn create_link(_: &DisplayView, target: &Target, selector: Sel) -> Retained<CADisplayLink> {
    // SAFETY: The selector takes the display link and is implemented by Target.
    unsafe { CADisplayLink::displayLinkWithTarget_selector(target, selector) }
}

#[cfg(target_os = "macos")]
fn maximum_frames_per_second(view: &DisplayView) -> f32 {
    view.window().and_then(|window| window.screen()).map(|screen| screen.maximumFramesPerSecond()).filter(|rate| *rate > 0).unwrap_or(60)
        as f32
}

#[cfg(target_os = "ios")]
fn maximum_frames_per_second(view: &DisplayView) -> f32 {
    view.window()
        .and_then(|window| window.windowScene())
        .map(|scene| scene.screen().maximumFramesPerSecond())
        .filter(|rate| *rate > 0)
        .unwrap_or(60) as f32
}

pub(super) fn retry_on_clock(acquired: DrawableAcquisition, externally_paced: bool) -> DrawableAcquisition {
    match acquired {
        DrawableAcquisition::Retry if externally_paced => DrawableAcquisition::WaitingForClock,
        _ => acquired,
    }
}

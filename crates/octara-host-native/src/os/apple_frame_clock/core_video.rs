//! Native scanout timing for interactive AppKit windows.

// Both Core Video and CAMetalDisplayLink reached 144 Hz in the hardware probe.
// Core Video supplies timing independently of drawable acquisition.
#![allow(deprecated)]

use super::{FrameTick, PlatformEvent, PlatformSender, instant_from_media_time};
use block2::RcBlock;
use objc2_app_kit::NSScreen;
use objc2_core_foundation::{CFRetained, CFRunLoop, kCFRunLoopCommonModes};
use objc2_core_video::{CVDisplayLink, CVGetHostClockFrequency, CVTimeStamp, CVTimeStampFlags, kCVReturnSuccess};
use objc2_foundation::{NSNumber, ns_string};
use octara::runtime::WindowId;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

enum PendingTick {
    Stopped,
    Running(Option<FrameTick>),
}

struct TickWake {
    proxy: PlatformSender,
    window: WindowId,
    pending: Mutex<PendingTick>,
}

impl TickWake {
    fn push(self: &Arc<Self>, tick: FrameTick) {
        {
            let mut pending = self.pending.lock().unwrap();
            let PendingTick::Running(latest) = &mut *pending else { return };
            if latest.replace(tick).is_some() {
                return;
            }
        }

        // One main-loop wake carries the newest tick through tracking and modal modes.
        let Some(run_loop) = CFRunLoop::main() else { return };
        let wake = Arc::clone(self);
        let deliver = RcBlock::new(move || {
            let tick = {
                let mut pending = wake.pending.lock().unwrap();
                let PendingTick::Running(latest) = &mut *pending else { return };
                latest.take()
            };
            if let Some(tick) = tick {
                wake.proxy.send_event(PlatformEvent::FrameClockTick { window: wake.window, tick });
            }
        });
        // SAFETY: CFRunLoopPerformBlock is thread-safe and copies the block. Its
        // captures are thread-safe and the common-mode callback runs on the main thread.
        unsafe { run_loop.perform_block(kCFRunLoopCommonModes.map(AsRef::as_ref), Some(&deliver)) };
        run_loop.wake_up();
    }
}

pub(super) struct VideoClock {
    link: CFRetained<CVDisplayLink>,
    wake: Arc<TickWake>,
}

impl VideoClock {
    pub(super) fn new(screen: &NSScreen, window: WindowId, proxy: PlatformSender) -> Option<Self> {
        let number = screen.deviceDescription().objectForKey(ns_string!("NSScreenNumber"))?;
        let display = number.downcast::<NSNumber>().ok()?.unsignedIntValue();
        let mut raw = std::ptr::null_mut();
        // SAFETY: Core Video writes a retained link into this stack out-parameter.
        if unsafe { CVDisplayLink::create_with_cg_display(display, NonNull::from(&mut raw)) } != kCVReturnSuccess {
            return None;
        }
        // SAFETY: A successful create returns a +1 reference.
        let link = unsafe { CFRetained::from_raw(NonNull::new(raw)?) };
        let wake = Arc::new(TickWake { proxy, window, pending: Mutex::new(PendingTick::Stopped) });
        let callback = Arc::clone(&wake);
        let host_frequency = CVGetHostClockFrequency();
        let maximum = screen.maximumFramesPerSecond();
        let nominal = Duration::from_secs_f64(1.0 / if maximum > 0 { maximum as f64 } else { 60.0 });
        let handler = RcBlock::new(move |_, _, output: NonNull<CVTimeStamp>, _, _| {
            // SAFETY: Core Video supplies this timestamp for the duration of the callback.
            let output = unsafe { output.as_ref() };
            let flags = CVTimeStampFlags::from_bits_retain(output.flags);
            let now = Instant::now();
            let target = if flags.contains(CVTimeStampFlags::HostTimeValid) {
                instant_from_media_time(now, output.hostTime as f64 / host_frequency)
            } else {
                now
            };
            let interval = if flags.contains(CVTimeStampFlags::VideoRefreshPeriodValid) {
                Duration::try_from_secs_f64(output.videoRefreshPeriod as f64 / f64::from(output.videoTimeScale))
                    .ok()
                    .filter(|interval| !interval.is_zero())
                    .unwrap_or(nominal)
            } else {
                nominal
            };
            callback.push(FrameTick { target, interval });
            kCVReturnSuccess
        });
        // SAFETY: Core Video copies the block. Its captured state is synchronized
        // and does not borrow the link or any main-thread objects.
        if unsafe { link.set_output_handler(RcBlock::as_ptr(&handler)) } != kCVReturnSuccess {
            return None;
        }
        Some(Self { link, wake })
    }

    pub(super) fn set_running(&self, running: bool) -> bool {
        if running {
            {
                let mut pending = self.wake.pending.lock().unwrap();
                if matches!(*pending, PendingTick::Stopped) {
                    *pending = PendingTick::Running(None);
                }
            }
            if self.link.is_running() || self.link.start() == kCVReturnSuccess {
                return true;
            }
        }
        *self.wake.pending.lock().unwrap() = PendingTick::Stopped;
        if self.link.is_running() {
            self.link.stop();
        }
        false
    }
}

impl Drop for VideoClock {
    fn drop(&mut self) {
        self.set_running(false);
    }
}

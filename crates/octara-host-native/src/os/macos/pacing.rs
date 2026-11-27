//! Metal display-link pacing and background drawable acquisition.

use super::metal_frame::MetalFrame;
use crate::os::{DrawableAcquisition, surface_drawable};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_foundation::{NSRunLoop, NSRunLoopCommonModes};
use objc2_metal::MTLPixelFormat;
use objc2_quartz_core::{
    CACurrentMediaTime, CAMetalDisplayLink, CAMetalDisplayLinkDelegate, CAMetalDisplayLinkUpdate, CAMetalDrawable, CAMetalLayer,
};
use octara_render::{gpu::OutputColor, wgpu};
use std::cell::Cell;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TryRecvError, sync_channel};
use std::time::{Duration, Instant};

type NativeDrawable = Retained<ProtocolObject<dyn CAMetalDrawable>>;

struct ClockIvars {
    newest: Cell<Option<NativeDrawable>>,
    active: Cell<bool>,
    wake: Box<dyn Fn(Instant)>,
}

define_class!(
    // SAFETY: Callbacks run on the main run loop where the display link is registered.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraMetalDisplayClock"]
    #[ivars = ClockIvars]
    struct Target;

    unsafe impl NSObjectProtocol for Target {}

    unsafe impl CAMetalDisplayLinkDelegate for Target {
        #[unsafe(method(metalDisplayLink:needsUpdate:))]
        fn update(&self, _link: &CAMetalDisplayLink, update: &CAMetalDisplayLinkUpdate) {
            let clock = self.ivars();
            if clock.active.get() {
                let target = instant_from_media_time(update.targetPresentationTimestamp());
                clock.newest.replace(Some(update.drawable()));
                (clock.wake)(target);
            }
        }
    }
);

fn instant_from_media_time(time: f64) -> Instant {
    let now = Instant::now();
    let delta = time - CACurrentMediaTime();
    Duration::try_from_secs_f64(delta.abs())
        .ok()
        .and_then(|offset| if delta >= 0.0 { now.checked_add(offset) } else { now.checked_sub(offset) })
        .unwrap_or(now)
}

struct DisplayClock {
    link: Retained<CAMetalDisplayLink>,
    target: Retained<Target>,
}

impl DisplayClock {
    fn new(layer: &CAMetalLayer, mtm: MainThreadMarker, wake: impl Fn(Instant) + 'static) -> Self {
        let this = Target::alloc(mtm).set_ivars(ClockIvars { newest: Cell::default(), active: Cell::new(false), wake: Box::new(wake) });
        let target: Retained<Target> = unsafe { msg_send![super(this), init] };
        let link = CAMetalDisplayLink::initWithMetalLayer(CAMetalDisplayLink::alloc(), layer);
        link.setPaused(true);
        link.setDelegate(Some(ProtocolObject::from_ref(&*target)));
        // SAFETY: Both the link and its delegate belong to this main run loop.
        unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes) };
        Self { link, target }
    }

    fn set_active(&self, active: bool) {
        let clock = self.target.ivars();
        if clock.active.replace(active) != active {
            self.link.setPaused(!active);
            if !active {
                clock.newest.take();
            }
        }
    }

    fn take(&self) -> Option<NativeDrawable> {
        self.target.ivars().newest.take()
    }
}

impl Drop for DisplayClock {
    fn drop(&mut self) {
        self.link.invalidate();
    }
}

struct LayerRequest(Retained<CAMetalLayer>);

// SAFETY: The worker's sole operation on the retained layer is nextDrawable. The host waits
// until it finishes before changing layer configuration.
unsafe impl Send for LayerRequest {}

struct Acquired(Option<NativeDrawable>);

// SAFETY: The worker transfers its retained drawable by value. The receiving thread owns
// all subsequent access.
unsafe impl Send for Acquired {}

struct Worker {
    send: SyncSender<LayerRequest>,
    receive: Receiver<Acquired>,
    pending: bool,
}

impl Worker {
    fn spawn(delay: Duration, wake: Arc<dyn Fn(Instant) + Send + Sync>) -> std::io::Result<Self> {
        let (send, work) = sync_channel::<LayerRequest>(1);
        let (replies, receive) = sync_channel(1);
        std::thread::Builder::new().name("octara-metal-acquire".into()).spawn(move || {
            // A disconnected worker must wake the host so it can retire the outstanding request.
            let replies = std::mem::DropGuard::new(replies, |replies| {
                drop(replies);
                wake(Instant::now());
            });
            while let Ok(layer) = work.recv() {
                if !delay.is_zero() {
                    match work.recv_timeout(delay) {
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                        Ok(_) => unreachable!("a drawable request is already pending"),
                    }
                }
                let drawable = autoreleasepool(|_| layer.0.nextDrawable());
                drop(layer);
                if replies.try_send(Acquired(drawable)).is_err() {
                    break;
                }
                wake(Instant::now());
            }
        })?;
        Ok(Self { send, receive, pending: false })
    }

    fn request(&mut self, layer: &Retained<CAMetalLayer>) -> bool {
        if !self.pending {
            self.pending = self.send.try_send(LayerRequest(layer.clone())).is_ok();
        }
        self.pending
    }

    fn take(&mut self) -> Option<NativeDrawable> {
        let result = match self.receive.try_recv() {
            Ok(acquired) => acquired.0,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => None,
        };
        self.pending = false;
        result
    }
}

pub(super) struct Pacing {
    mtm: MainThreadMarker,
    wake: Arc<dyn Fn(Instant) + Send + Sync>,
    layer: Option<Retained<CAMetalLayer>>,
    clock: Option<DisplayClock>,
    worker: Option<Worker>,
    delay: Duration,
    frame_latency: u32,
    active: bool,
    allow_clock: bool,
    has_presented: bool,
    init_attempted: bool,
}

impl Pacing {
    pub(super) fn new(
        mtm: MainThreadMarker,
        allow_clock: bool,
        delay: Duration,
        frame_latency: u32,
        wake: impl Fn(Instant) + Send + Sync + 'static,
    ) -> Self {
        Self {
            mtm,
            wake: Arc::new(wake),
            layer: None,
            clock: None,
            worker: None,
            delay,
            frame_latency,
            active: false,
            allow_clock,
            has_presented: false,
            init_attempted: false,
        }
    }

    pub(super) fn externally_paced(&self) -> bool {
        self.clock.is_some()
    }

    pub(super) fn set_active(&mut self, active: bool) {
        self.active = active;
        if let Some(clock) = &self.clock {
            clock.set_active(active);
        }
    }

    pub(super) fn reset(&mut self) -> bool {
        self.clock = None;
        if let Some(worker) = &mut self.worker {
            worker.take();
            if worker.pending {
                return false;
            }
        }
        self.layer = None;
        self.init_attempted = false;
        true
    }

    pub(super) fn did_show(&mut self) {
        self.has_presented = true;
        self.start_clock();
    }

    fn start_clock(&mut self) {
        if !self.has_presented || !self.allow_clock || !self.delay.is_zero() || self.clock.is_some() {
            return;
        }
        let Some(layer) = self.layer.as_deref().filter(|layer| direct_target(layer)) else { return };
        let wake = self.wake.clone();
        let clock = self.clock.insert(DisplayClock::new(layer, self.mtm, move |target| wake(target)));
        clock.link.setPreferredFrameLatency(self.frame_latency as f32);
        clock.set_active(self.active);
    }

    pub(super) fn layer(&self) -> Option<&CAMetalLayer> {
        self.layer.as_deref()
    }

    pub(super) fn acquire(&mut self, surface: &Arc<wgpu::Surface<'static>>, color: OutputColor) -> DrawableAcquisition {
        if !self.init_attempted {
            self.init_attempted = true;
            self.layer = metal_layer(surface);
            self.start_clock();
            if self.clock.is_none() && self.layer.as_deref().is_some_and(direct_target) && self.worker.is_none() {
                self.worker = Worker::spawn(self.delay, self.wake.clone()).ok();
            }
        }
        let Some(layer) = &self.layer else { return surface_drawable(surface, color) };
        if !direct_target(layer) {
            return surface_drawable(surface, color);
        }
        let from_display_link = self.clock.is_some();
        let drawable = if let Some(clock) = &self.clock {
            clock.take()
        } else if let Some(worker) = &mut self.worker {
            let ready = worker.take();
            if ready.is_none() && !worker.request(layer) {
                self.worker = None;
                return surface_drawable(surface, color);
            }
            ready
        } else {
            return surface_drawable(surface, color);
        };
        let Some(drawable) = drawable else { return DrawableAcquisition::Retry };
        let size = layer.drawableSize();
        let size = (size.width as u32, size.height as u32);
        DrawableAcquisition::Ready(Box::new(MetalFrame::new(drawable, size, color, from_display_link)))
    }
}

fn metal_layer(surface: &wgpu::Surface<'static>) -> Option<Retained<CAMetalLayer>> {
    // SAFETY: The HAL guard keeps the surface alive while we retain its layer.
    let layer = unsafe { surface.as_hal::<wgpu::hal::api::Metal>() }?.render_layer().lock().clone();
    if !layer.allowsNextDrawableTimeout() {
        layer.setAllowsNextDrawableTimeout(true);
    }
    Some(layer)
}

fn direct_target(layer: &CAMetalLayer) -> bool {
    layer.pixelFormat() == MTLPixelFormat::BGRA8Unorm_sRGB && !layer.framebufferOnly()
}

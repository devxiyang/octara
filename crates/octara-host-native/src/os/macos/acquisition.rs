//! AppKit drawable acquisition, including the delayed test path.

#[cfg(feature = "wgpu")]
use super::wgpu_drawable::MetalFrame;
use crate::os::apple_metal::set_transactional;
use crate::os::{Drawable, DrawableAcquisition, NativeSurfaceSession};
use block2::RcBlock;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2::sel;
use objc2_metal::{MTLDrawable, MTLPixelFormat, MTLTexture};
use objc2_quartz_core::{CALayer, CAMetalDrawable, CAMetalLayer, CATransaction, kCAGravityResize, kCAGravityTopLeft};
use octara_gpu::SurfaceSession;
use octara_render::OutputColor;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TryRecvError, sync_channel};
use std::time::{Duration, Instant};

type NativeDrawable = Retained<ProtocolObject<dyn CAMetalDrawable>>;

// A hidden or disconnected display may never acknowledge the drawable.
const DISPLAY_WAIT_LIMIT: Duration = Duration::from_millis(80);

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

pub(super) struct Acquisition {
    wake: Arc<dyn Fn(Instant) + Send + Sync>,
    layer: Option<Retained<CAMetalLayer>>,
    worker: Option<Worker>,
    delay: Duration,
    shown: bool,
    init_attempted: bool,
    displaying: Option<(Instant, Arc<AtomicBool>)>,
}

impl Acquisition {
    pub(super) fn new(delay: Duration, wake: impl Fn(Instant) + Send + Sync + 'static) -> Self {
        Self { wake: Arc::new(wake), layer: None, worker: None, delay, shown: false, init_attempted: false, displaying: None }
    }

    pub(super) fn did_show(&mut self) {
        self.shown = true;
        if self.delay.is_zero() {
            self.worker = None;
        }
    }

    pub(super) fn reset(&mut self) -> bool {
        if let Some(worker) = &mut self.worker {
            worker.take();
            if worker.pending {
                return false;
            }
        }
        self.layer = None;
        self.init_attempted = false;
        self.displaying = None;
        true
    }

    pub(super) fn waiting_for_display(&self) -> bool {
        self.displaying.as_ref().is_some_and(|(started, done)| started.elapsed() < DISPLAY_WAIT_LIMIT && !done.load(Ordering::Acquire))
    }

    pub(super) fn did_submit_for_display(&mut self) {
        if let Some((started, _)) = &mut self.displaying {
            *started = Instant::now();
        }
    }

    pub(super) fn acquire(
        &mut self,
        surface: &NativeSurfaceSession,
        color: OutputColor,
        externally_paced: bool,
        transactional: bool,
        resizing: bool,
    ) -> DrawableAcquisition {
        if !self.init_attempted {
            self.init_attempted = true;
            self.layer = surface.metal_layer();
            if let Some(layer) = &self.layer {
                if !layer.allowsNextDrawableTimeout() {
                    layer.setAllowsNextDrawableTimeout(true);
                }
                // wgpu treats a hidden AppKit window as occluded. Acquire the first drawable
                // directly so we can present it before ordering the window onscreen.
                if (!self.shown || !self.delay.is_zero()) && self.worker.is_none() && direct_target(layer) {
                    self.worker = Worker::spawn(self.delay, self.wake.clone()).ok();
                }
            }
        }
        let Some(layer) = &self.layer else { return surface.acquire_with_transaction(color, transactional) };
        align_contents(layer, resizing || self.waiting_for_display());
        if externally_paced && resizing && direct_target(layer) {
            // Hold the drawable through native display acknowledgement, which wgpu's
            // surface texture does not expose. A subsequent configure can replace its storage.
            set_transactional(layer, transactional);
            let Some(drawable) = layer.nextDrawable() else { return DrawableAcquisition::WaitingForClock };
            let size = layer.drawableSize();
            let texture = drawable.texture();
            if texture.width() != size.width as usize || texture.height() != size.height as usize {
                return DrawableAcquisition::WaitingForClock;
            }
            if !drawable.respondsToSelector(sel!(addPresentedHandler:)) {
                return DrawableAcquisition::Ready(drawable_target(drawable, layer, color, transactional));
            }
            let done = Arc::new(AtomicBool::new(false));
            let callback_done = done.clone();
            let wake = self.wake.clone();
            let callback = RcBlock::new(move |_: std::ptr::NonNull<ProtocolObject<dyn MTLDrawable>>| {
                callback_done.store(true, Ordering::Release);
                wake(Instant::now());
            });
            // SAFETY: Metal copies the block; the atomic flag is safe on the callback thread.
            unsafe { drawable.addPresentedHandler(RcBlock::as_ptr(&callback)) };
            self.displaying = Some((Instant::now(), done));
            return DrawableAcquisition::Ready(drawable_target(drawable, layer, color, transactional));
        }
        if externally_paced {
            set_transactional(layer, transactional);
            return surface.acquire_with_transaction(color, transactional);
        }
        if let Some(worker) = self.worker.as_mut().filter(|_| direct_target(layer)) {
            if let Some(drawable) = worker.take() {
                set_transactional(layer, transactional);
                return DrawableAcquisition::Ready(drawable_target(drawable, layer, color, transactional));
            }
            if worker.request(layer) {
                return DrawableAcquisition::Retry;
            }
            self.worker = None;
        }
        set_transactional(layer, transactional);
        surface.acquire_with_transaction(color, transactional)
    }
}

#[cfg(not(feature = "wgpu"))]
fn drawable_target(drawable: NativeDrawable, _layer: &CAMetalLayer, color: OutputColor, transactional: bool) -> Box<Drawable> {
    Box::new(octara_gpu::MetalDrawableTarget::new(drawable, color, transactional))
}

#[cfg(feature = "wgpu")]
fn drawable_target(drawable: NativeDrawable, layer: &CAMetalLayer, color: OutputColor, transactional: bool) -> Box<Drawable> {
    let size = layer.drawableSize();
    Box::new(MetalFrame::new(drawable, (size.width as u32, size.height as u32), color, transactional))
}

pub(super) fn align_contents(layer: &CALayer, live_resize: bool) {
    // SAFETY: Core Animation exports both gravity constants for the process lifetime.
    let gravity = unsafe { if live_resize { kCAGravityTopLeft } else { kCAGravityResize } };
    if &*layer.contentsGravity() != gravity {
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        let _transaction = std::mem::DropGuard::new((), |()| CATransaction::commit());
        layer.setContentsGravity(gravity);
    }
}

fn direct_target(layer: &CAMetalLayer) -> bool {
    layer.pixelFormat() == MTLPixelFormat::BGRA8Unorm_sRGB && !layer.framebufferOnly()
}

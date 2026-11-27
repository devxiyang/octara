//! AppKit drawable acquisition, including the delayed test path.

use super::metal_frame::MetalFrame;
use crate::os::apple_metal::{metal_layer, set_transactional};
use crate::os::{DrawableAcquisition, surface_drawable};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_metal::MTLPixelFormat;
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};
use octara_render::{gpu::OutputColor, wgpu};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TryRecvError, sync_channel};
use std::time::{Duration, Instant};

type NativeDrawable = Retained<ProtocolObject<dyn CAMetalDrawable>>;

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
    init_attempted: bool,
}

impl Acquisition {
    pub(super) fn new(delay: Duration, wake: impl Fn(Instant) + Send + Sync + 'static) -> Self {
        Self { wake: Arc::new(wake), layer: None, worker: None, delay, init_attempted: false }
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
        true
    }

    pub(super) fn layer(&self) -> Option<&CAMetalLayer> {
        self.layer.as_deref()
    }

    pub(super) fn acquire(
        &mut self,
        surface: &Arc<wgpu::Surface<'static>>,
        color: OutputColor,
        externally_paced: bool,
        transactional: bool,
    ) -> DrawableAcquisition {
        if !self.init_attempted {
            self.init_attempted = true;
            self.layer = metal_layer(surface);
            if let Some(layer) = &self.layer {
                if !layer.allowsNextDrawableTimeout() {
                    layer.setAllowsNextDrawableTimeout(true);
                }
                if !externally_paced && self.worker.is_none() && direct_target(layer) {
                    self.worker = Worker::spawn(self.delay, self.wake.clone()).ok();
                }
            }
        }
        let Some(layer) = &self.layer else { return surface_drawable(surface, color) };
        if externally_paced {
            set_transactional(layer, transactional);
            return surface_drawable(surface, color);
        }
        if let Some(worker) = self.worker.as_mut().filter(|_| direct_target(layer)) {
            if let Some(drawable) = worker.take() {
                set_transactional(layer, transactional);
                let size = layer.drawableSize();
                let size = (size.width as u32, size.height as u32);
                return DrawableAcquisition::Ready(Box::new(MetalFrame::new(drawable, size, color, transactional)));
            }
            if worker.request(layer) {
                return DrawableAcquisition::Retry;
            }
            self.worker = None;
        }
        set_transactional(layer, transactional);
        surface_drawable(surface, color)
    }
}

fn direct_target(layer: &CAMetalLayer) -> bool {
    layer.pixelFormat() == MTLPixelFormat::BGRA8Unorm_sRGB && !layer.framebufferOnly()
}

//! Backend-neutral GPU work, completion and error reporting.

use crate::generation::{DeviceGeneration, DeviceNotice};
use octara_model::capture::{FrameReceipt, PaintReceipt};
use octara_model::identity::FrameStamp;
use octara_model::paint::DisplayList;
use octara_render::plan::{FrameProfile, GpuTimings};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Instant;

#[cfg(any(feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
use octara_render::gpu::{PendingPresentation as DefaultPresentation, RecordError};
#[cfg(all(not(feature = "wgpu"), any(target_os = "macos", target_os = "ios")))]
use octara_render_metal::MetalPresentation as DefaultPresentation;

#[derive(Debug)]
pub enum GpuError {
    Busy,
    Expired,
    WrongGeneration {
        expected: u64,
        actual: u64,
    },
    Stopped {
        may_have_submitted: bool,
    },
    DeviceLost {
        generation: u64,
        may_have_submitted: bool,
    },
    Drawable(String),
    #[cfg(any(feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
    Frame(RecordError),
    Recording(String),
    Submission(String),
    Readback(String),
}

impl fmt::Display for GpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => f.write_str("the GPU job capacity is full"),
            Self::Expired => f.write_str("the frame's presentation opportunity expired"),
            Self::WrongGeneration { expected, actual } => write!(f, "GPU generation {expected} cannot draw a frame for {actual}"),
            Self::Stopped { may_have_submitted } => write!(f, "the GPU executor stopped{}", outcome(*may_have_submitted)),
            Self::DeviceLost { generation, may_have_submitted } => {
                write!(f, "GPU device generation {generation} was lost{}", outcome(*may_have_submitted))
            }
            Self::Drawable(error) => write!(f, "cannot acquire the draw texture: {error}"),
            #[cfg(any(feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
            Self::Frame(error) => error.fmt(f),
            Self::Recording(error) => write!(f, "GPU recording failed: {error}"),
            Self::Submission(error) => write!(f, "GPU submission failed: {error}"),
            Self::Readback(error) => f.write_str(error),
        }
    }
}

impl std::error::Error for GpuError {}

fn outcome(may_have_submitted: bool) -> &'static str {
    if may_have_submitted { " with submission outcome unknown" } else { " before submission" }
}

pub struct DrawWork<T> {
    pub target: T,
    pub list: Arc<DisplayList>,
    pub scale: f32,
    pub stamp: FrameStamp,
    pub valid_until: Option<Instant>,
    pub report: Option<Box<dyn FnOnce(GpuTimings) + Send>>,
}

pub struct DrawnFrame<T, P = DefaultPresentation> {
    pub target: T,
    pub stamp: FrameStamp,
    pub effects: Vec<PaintReceipt>,
    pub profile: Option<FrameProfile>,
    pub presentation: P,
    pub(super) progress: FrameProgress,
}

impl<T, P> DrawnFrame<T, P> {
    pub fn gpu_complete(&self) -> bool {
        self.progress.gpu_complete()
    }

    /// Keep GPU completion observable after passing the drawable to the host.
    pub fn progress(&self) -> FrameProgress {
        self.progress.clone()
    }

    /// Attach the presentation time observed by the host when it accepts this frame.
    pub fn receipt(&self, presented_at_ns: Option<u64>) -> FrameReceipt {
        self.progress.receipt(presented_at_ns)
    }
}

#[derive(Clone)]
pub struct FrameProgress {
    stamp: FrameStamp,
    gpu_complete: Arc<AtomicBool>,
}

impl FrameProgress {
    pub(super) fn new(stamp: FrameStamp) -> Self {
        Self { stamp, gpu_complete: Arc::new(AtomicBool::new(false)) }
    }

    pub(super) fn complete(&self) {
        self.gpu_complete.store(true, Ordering::Release);
    }

    pub fn gpu_complete(&self) -> bool {
        self.gpu_complete.load(Ordering::Acquire)
    }

    pub fn receipt(&self, presented_at_ns: Option<u64>) -> FrameReceipt {
        FrameReceipt { stamp: self.stamp, gpu_complete: self.gpu_complete(), presented_at_ns }
    }
}

#[must_use = "keep the task alive to receive the GPU result"]
pub struct GpuTask<T> {
    receiver: Option<mpsc::Receiver<Result<T, GpuError>>>,
    pub(super) cancelled: Arc<AtomicBool>,
    generation: Arc<DeviceGeneration>,
}

impl<T> GpuTask<T> {
    pub fn try_take(&mut self) -> Option<Result<T, GpuError>> {
        let result = match self.receiver.as_ref()?.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err(self.generation.unavailable(false).unwrap_or(GpuError::Stopped { may_have_submitted: false }))
            }
        };
        self.receiver = None;
        Some(result)
    }
}

impl<T> Drop for GpuTask<T> {
    fn drop(&mut self) {
        if self.receiver.is_some() {
            self.cancelled.store(true, Ordering::Release);
            self.generation.cancel(&self.cancelled);
        }
    }
}

pub(super) struct Reply<T> {
    sender: Option<mpsc::Sender<Result<T, GpuError>>>,
    cancelled: Arc<AtomicBool>,
    generation: Arc<DeviceGeneration>,
    pub(super) may_have_submitted: bool,
}

impl<T> Reply<T> {
    pub(super) fn new(generation: Arc<DeviceGeneration>) -> (Self, GpuTask<T>) {
        let (sender, receiver) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let reply = Self { sender: Some(sender), cancelled: cancelled.clone(), generation: generation.clone(), may_have_submitted: false };
        (reply, GpuTask { receiver: Some(receiver), cancelled, generation })
    }

    pub(super) fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub(super) fn check_live(&self) -> Result<(), GpuError> {
        match self.generation.unavailable(self.may_have_submitted) {
            Some(error) => Err(error),
            None if self.cancelled() => Err(self.stopped()),
            None => Ok(()),
        }
    }

    pub(super) fn stopped(&self) -> GpuError {
        self.generation.unavailable(self.may_have_submitted).unwrap_or(GpuError::Stopped { may_have_submitted: self.may_have_submitted })
    }

    pub(super) fn finish(mut self, result: Result<T, GpuError>) {
        self.send(result);
    }

    fn send(&mut self, result: Result<T, GpuError>) {
        let sender = self.sender.take().expect("every GPU job has one reply");
        if !self.cancelled() && sender.send(result).is_ok() {
            self.generation.notify(DeviceNotice::WorkReady);
        }
    }
}

impl<T> Drop for Reply<T> {
    fn drop(&mut self) {
        if self.sender.is_some() {
            self.send(Err(self.stopped()));
        }
    }
}

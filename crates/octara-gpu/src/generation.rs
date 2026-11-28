//! Lifetime and capacity accounting for one native GPU device.

use crate::GpuError;
use octara_model::CacheLine;
#[cfg(any(feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
use octara_render::wgpu;
#[cfg(any(feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
use std::any::Any;
#[cfg(any(feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
use std::panic::resume_unwind;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

// One operation on the device, with room for its successor.
const MAX_RESOURCE_OPERATIONS: usize = 2;

#[derive(Debug)]
pub enum DeviceNotice {
    WorkReady,
    CapacityAvailable,
    Lost,
    Failed(String),
}

/// Wake the host for an event from the identified generation. Keep the callback short.
pub type DeviceNotifier = Arc<dyn Fn(u64, DeviceNotice) + Send + Sync>;

pub(super) trait DeviceEvents: Send + Sync {
    fn gpu_event(&self);
    fn cancel(&self, token: &Arc<AtomicBool>);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceCapacity {
    pub pending_submissions: usize,
    pub pending_upload_bytes: u64,
    pub readback_available: bool,
    pub readback_bytes: u64,
    pub resource_available: bool,
}

#[derive(Default)]
struct Work {
    submissions: usize,
    upload_bytes: u64,
    readback: Option<ReadbackAllocation>,
    resources: usize,
}

#[derive(Default)]
struct ReadbackAllocation {
    bytes: u64,
    on_gpu: bool,
}

enum LossEvidence {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    Metal,
    #[cfg(any(feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
    Wgpu((wgpu::DeviceLostReason, String)),
}

pub struct DeviceGeneration {
    number: CacheLine<u64>,
    loss: OnceLock<LossEvidence>,
    stopped: CacheLine<AtomicBool>,
    work: Mutex<Work>,
    inbox: OnceLock<Weak<dyn DeviceEvents>>,
    notifier: Option<DeviceNotifier>,
}

impl DeviceGeneration {
    pub fn new(number: u64, notifier: Option<DeviceNotifier>) -> Self {
        Self {
            number: CacheLine(number),
            loss: OnceLock::new(),
            stopped: CacheLine(AtomicBool::new(false)),
            work: Mutex::default(),
            inbox: OnceLock::new(),
            notifier,
        }
    }

    pub fn number(&self) -> u64 {
        self.number.0
    }

    pub fn successor(&self) -> Self {
        Self::new(self.number().checked_add(1).expect("device generation exhausted"), self.notifier.clone())
    }

    pub fn capacity(&self) -> DeviceCapacity {
        let work = self.work.lock().expect("GPU work accounting");
        DeviceCapacity {
            pending_submissions: work.submissions,
            pending_upload_bytes: work.upload_bytes,
            readback_available: work.readback.is_none(),
            readback_bytes: work.readback.as_ref().map_or(0, |readback| readback.bytes),
            resource_available: work.resources < MAX_RESOURCE_OPERATIONS,
        }
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }

    pub fn is_lost(&self) -> bool {
        self.loss.get().is_some()
    }

    /// Ignore callbacks from retired generations when deciding whether to replace the current device.
    #[cfg(any(feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
    pub fn loss_for(&self, number: u64) -> Option<&(wgpu::DeviceLostReason, String)> {
        match self.loss.get() {
            Some(LossEvidence::Wgpu(evidence)) if self.number() == number => Some(evidence),
            _ => None,
        }
    }

    pub(super) fn attach(&self, inbox: &Arc<impl DeviceEvents + 'static>) -> std::io::Result<()> {
        let inbox = Arc::downgrade(inbox);
        self.inbox.set(inbox).map_err(|_| std::io::Error::other("GPU generation already has an executor"))
    }

    pub(super) fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }

    pub(super) fn unavailable(&self, may_have_submitted: bool) -> Option<GpuError> {
        if self.is_lost() {
            Some(GpuError::DeviceLost { generation: self.number(), may_have_submitted })
        } else if self.is_stopped() {
            Some(GpuError::Stopped { may_have_submitted })
        } else {
            None
        }
    }

    pub(super) fn notify(&self, notice: DeviceNotice) {
        if let Some(notify) = &self.notifier {
            // An observer can run on a driver callback or on a dying worker.
            let _ = catch_unwind(AssertUnwindSafe(|| notify(self.number(), notice)));
        }
    }

    pub(super) fn gpu_event(&self) {
        if let Some(inbox) = self.inbox.get().and_then(Weak::upgrade) {
            inbox.gpu_event();
        }
    }

    pub(super) fn cancel(&self, token: &Arc<AtomicBool>) {
        if let Some(inbox) = self.inbox.get().and_then(Weak::upgrade) {
            inbox.cancel(token);
        }
    }

    fn wake_capacity(&self) {
        self.gpu_event();
        if !self.is_stopped() {
            self.notify(DeviceNotice::CapacityAvailable);
        }
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pub(super) fn mark_metal_lost(&self) {
        self.record_loss(LossEvidence::Metal);
    }

    fn record_loss(&self, evidence: LossEvidence) {
        if self.loss.set(evidence).is_ok() {
            self.gpu_event();
            if !self.is_stopped() {
                self.notify(DeviceNotice::Lost);
            }
        }
    }

    #[cfg(any(feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
    pub(super) fn watch_loss(self: &Arc<Self>, device: &wgpu::Device) {
        let generation = Arc::downgrade(self);
        device.set_device_lost_callback(move |reason, message| {
            if let Some(generation) = generation.upgrade() {
                generation.record_loss(LossEvidence::Wgpu((reason, message)));
            }
        });
    }

    /// Count the work until the driver runs the callback or discards it on teardown.
    #[cfg(any(feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
    pub(super) fn observe_submission(
        self: &Arc<Self>,
        queue: &wgpu::Queue,
        upload_bytes: u64,
        on_complete: impl FnOnce() + Send + 'static,
    ) {
        let lease = self.begin_submission(upload_bytes);
        queue.on_submitted_work_done(move || {
            on_complete();
            drop(lease);
        });
    }

    pub(super) fn begin_submission(self: &Arc<Self>, upload_bytes: u64) -> SubmissionLease {
        let mut work = self.work.lock().expect("GPU work accounting");
        work.submissions += 1;
        work.upload_bytes += upload_bytes;
        SubmissionLease { generation: self.clone(), upload_bytes }
    }

    #[cfg(any(test, feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
    pub(super) fn gpu_work_pending(&self) -> bool {
        let work = self.work.lock().expect("GPU work accounting");
        work.submissions != 0 || work.readback.as_ref().is_some_and(|readback| readback.on_gpu)
    }

    #[cfg(any(test, feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
    pub(super) fn reserve_resource(self: &Arc<Self>) -> Result<ResourceLease, GpuError> {
        let mut work = self.work.lock().expect("GPU work accounting");
        if let Some(error) = self.unavailable(false) {
            return Err(error);
        }
        if work.resources == MAX_RESOURCE_OPERATIONS {
            return Err(GpuError::Busy);
        }
        work.resources += 1;
        Ok(ResourceLease(self.clone()))
    }

    pub(super) fn reserve_readback(self: &Arc<Self>) -> Result<Arc<ReadbackLease>, GpuError> {
        let mut work = self.work.lock().expect("GPU work accounting");
        if let Some(error) = self.unavailable(false) {
            return Err(error);
        }
        if work.readback.is_some() {
            return Err(GpuError::Busy);
        }
        work.readback = Some(ReadbackAllocation::default());
        Ok(Arc::new(ReadbackLease(self.clone())))
    }

    /// wgpu can unwind when polling a lost device or using invalidated handles.
    /// Only suppress a matching driver panic after this generation reports loss.
    #[cfg(any(feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
    pub(super) fn run<T>(&self, operation: impl FnOnce() -> T) -> Option<T> {
        if self.is_lost() {
            return None;
        }
        match catch_unwind(AssertUnwindSafe(operation)) {
            Ok(value) => (!self.is_lost()).then_some(value),
            Err(payload) if self.is_lost() && is_wgpu_loss_panic(payload.as_ref()) => None,
            Err(payload) => resume_unwind(payload),
        }
    }
}

pub(super) struct SubmissionLease {
    generation: Arc<DeviceGeneration>,
    upload_bytes: u64,
}

impl Drop for SubmissionLease {
    fn drop(&mut self) {
        let mut work = self.generation.work.lock().expect("GPU work accounting");
        work.submissions -= 1;
        work.upload_bytes -= self.upload_bytes;
        drop(work);
        self.generation.wake_capacity();
    }
}

#[cfg(any(test, feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
pub(super) struct ResourceLease(Arc<DeviceGeneration>);

#[cfg(any(test, feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
impl Drop for ResourceLease {
    fn drop(&mut self) {
        self.0.work.lock().expect("GPU work accounting").resources -= 1;
        self.0.wake_capacity();
    }
}

/// One slot, held by both the buffer owner and the GPU until each is finished.
pub(super) struct ReadbackLease(Arc<DeviceGeneration>);

impl ReadbackLease {
    pub(super) fn allocated(&self, bytes: u64) {
        self.0.work.lock().expect("GPU work accounting").readback.as_mut().expect("the readback slot is reserved").bytes = bytes;
    }

    pub(super) fn gpu_completion(self: &Arc<Self>) -> impl FnOnce() + Send + 'static {
        self.set_gpu_pending(true);
        // Dropping an unsubmitted encoder also drops this callback.
        let hold = std::mem::DropGuard::new(self.clone(), |lease| {
            lease.set_gpu_pending(false);
            lease.0.gpu_event();
        });
        move || drop(hold)
    }

    fn set_gpu_pending(&self, pending: bool) {
        self.0.work.lock().expect("GPU work accounting").readback.as_mut().expect("the readback slot is reserved").on_gpu = pending;
    }
}

impl Drop for ReadbackLease {
    fn drop(&mut self) {
        let readback = self.0.work.lock().expect("GPU work accounting").readback.take().expect("the readback slot is reserved");
        debug_assert!(!readback.on_gpu);
        self.0.wake_capacity();
    }
}

#[cfg(any(feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
fn is_wgpu_loss_panic(payload: &(dyn Any + Send)) -> bool {
    let Some(message) = payload.downcast_ref::<String>() else { return false };
    if message.starts_with("Error in Device::poll:") {
        return true;
    }
    let error = message.strip_prefix("wgpu error:").or_else(|| message.strip_prefix("Octara frame submission failed:"));
    error.is_some_and(|error| error.trim() == "Out of Memory" || error.lines().any(|line| line.trim_end().ends_with(" is invalid")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readback_stays_reserved_until_buffer_and_gpu_finish() {
        let generation = Arc::new(DeviceGeneration::new(2, None));
        let lease = generation.reserve_readback().unwrap();
        lease.allocated(512);
        let queued = lease.gpu_completion();
        drop(lease);
        assert!(generation.gpu_work_pending());
        assert_eq!(generation.capacity().readback_bytes, 512);
        assert!(matches!(generation.reserve_readback(), Err(GpuError::Busy)));
        drop(queued);
        assert!(!generation.gpu_work_pending());
        assert!(generation.capacity().readback_available);
    }

    #[test]
    fn stopped_generations_do_not_admit_work_and_successors_have_clean_capacity() {
        let generation = Arc::new(DeviceGeneration::new(2, None));
        let one = generation.reserve_resource().unwrap();
        let two = generation.reserve_resource().unwrap();
        assert!(matches!(generation.reserve_resource(), Err(GpuError::Busy)));
        drop(one);
        assert!(generation.capacity().resource_available);
        generation.stop();
        drop(two);
        assert!(matches!(generation.reserve_resource(), Err(GpuError::Stopped { .. })));
        let next = generation.successor();
        assert_eq!(next.number(), 3);
        assert!(!next.is_stopped());
        assert!(next.capacity().resource_available);
    }
}

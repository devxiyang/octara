//! One Metal queue and encoder, owned by the device worker.

use super::capture::Capture;
use crate::executor::Reply;
use crate::generation::{DeviceEvents, DeviceNotice, ReadbackLease};
use crate::schedule::{Lane, Schedule};
use crate::{
    BackendCapabilities, CapturePixels, DeviceDrawnFrame, DeviceGeneration, DrawWork, DrawnFrame, FrameProgress, GpuDevice, GpuError,
    GpuTask, MetalDrawableTarget,
};
use block2::RcBlock;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLCommandBuffer, MTLCommandBufferError, MTLCommandBufferErrorDomain, MTLCommandBufferStatus, MTLCommandQueue,
    MTLCreateSystemDefaultDevice, MTLDevice,
};
use objc2_quartz_core::CAMetalDrawable;
use octara_model::capture::PaintSample;
use octara_model::paint::DisplayList;
use octara_render::plan::GpuTimings;
use octara_render_metal::{MetalPresentation, MetalRenderer};
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Condvar, Mutex, Weak, mpsc};
use std::thread;
use std::time::{Duration, Instant};

type Device = ProtocolObject<dyn MTLDevice>;
type Command = ProtocolObject<dyn MTLCommandBuffer>;
type Job = Box<dyn FnOnce(&mut Worker) + Send>;

pub struct MetalGpuDevice {
    device: Retained<Device>,
    generation: Arc<DeviceGeneration>,
    inbox: Arc<Inbox>,
    worker: Option<thread::JoinHandle<()>>,
}

impl MetalGpuDevice {
    /// Start a Metal device and encoder. A surface is only needed for on-screen drawing.
    pub fn new(generation: Arc<DeviceGeneration>, profile: bool, thread_setup: fn()) -> Result<Self, String> {
        let device = MTLCreateSystemDefaultDevice().ok_or("no system Metal device is available")?;
        let inbox = Arc::new(Inbox::default());
        generation.attach(&inbox).map_err(|error| error.to_string())?;
        let worker_device = device.clone();
        let worker_generation = generation.clone();
        let worker_inbox = inbox.clone();
        let (started, status) = mpsc::sync_channel(0);
        let worker = thread::Builder::new().name("octara-metal".into()).spawn(move || {
            let _ = catch_unwind(AssertUnwindSafe(thread_setup));
            let result = catch_unwind(AssertUnwindSafe(|| {
                let worker = autoreleasepool(|_| Worker::new(worker_device, worker_generation.clone(), profile));
                match worker {
                    Ok(worker) if started.send(Ok(())).is_ok() => worker.run(&worker_inbox),
                    Ok(_) => {}
                    Err(error) => {
                        let _ = started.send(Err(error));
                    }
                }
            }));
            worker_generation.stop();
            worker_inbox.close();
            if result.is_err() {
                worker_generation.notify(DeviceNotice::Failed("the Metal device worker panicked".into()));
            }
        });
        let worker = match worker {
            Ok(worker) => worker,
            Err(error) => {
                generation.stop();
                return Err(error.to_string());
            }
        };
        let device = Self { device, generation, inbox, worker: Some(worker) };
        match status.recv().unwrap_or_else(|_| Err("the Metal device worker stopped during startup".into())) {
            Ok(()) => Ok(device),
            Err(error) => {
                device.shutdown();
                Err(error)
            }
        }
    }

    pub(super) fn raw_device(&self) -> &Device {
        &self.device
    }

    pub(super) fn owns_surface_lease(&self, lease: &MetalSurfaceLease) -> bool {
        lease.inbox.ptr_eq(&Arc::downgrade(&self.inbox))
    }

    fn enqueue<T: Send + 'static>(
        &self,
        lane: Lane,
        run: impl FnOnce(&mut Worker, Reply<T>) + Send + 'static,
    ) -> Result<GpuTask<T>, GpuError> {
        if let Some(error) = self.generation.unavailable(false) {
            return Err(error);
        }
        let (reply, task) = Reply::new(self.generation.clone());
        let mut state = self.inbox.state.lock().expect("Metal device inbox");
        if state.closed {
            return Err(GpuError::Stopped { may_have_submitted: false });
        }
        state.jobs.push(
            lane,
            Instant::now(),
            QueuedJob {
                token: task.cancelled.clone(),
                run: Box::new(move |worker| {
                    if !reply.cancelled() {
                        run(worker, reply);
                    }
                }),
            },
        );
        drop(state);
        self.inbox.ready.notify_one();
        Ok(task)
    }

    fn stop(&self) {
        self.generation.stop();
        self.inbox.close();
    }
}

impl GpuDevice for MetalGpuDevice {
    type Drawable = MetalDrawableTarget;
    type SurfaceLease = MetalSurfaceLease;
    type Presentation = MetalPresentation;

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities { metal: true }
    }

    fn generation(&self) -> &Arc<DeviceGeneration> {
        &self.generation
    }

    fn draw(&self, work: DrawWork<Box<MetalDrawableTarget>>) -> Result<GpuTask<DeviceDrawnFrame<Self>>, GpuError> {
        let expected = self.generation.number();
        let actual = work.stamp.device_generation;
        if expected != actual {
            return Err(GpuError::WrongGeneration { expected, actual });
        }
        self.enqueue(Lane::Draw, move |worker, mut reply| {
            let result = worker.draw(work, &mut reply);
            reply.finish(result);
        })
    }

    fn drain_surfaces(&self) -> Result<GpuTask<MetalSurfaceLease>, GpuError> {
        let inbox = Arc::downgrade(&self.inbox);
        self.enqueue(Lane::Drain, move |worker, reply| {
            worker.wait_for_submitted();
            let result = reply.check_live().and_then(|()| {
                let strong = inbox.upgrade().ok_or_else(|| reply.stopped())?;
                strong.state.lock().expect("Metal device inbox").paused = true;
                Ok(MetalSurfaceLease { inbox })
            });
            reply.finish(result);
        })
    }

    fn accept_presentation(&self, presentation: MetalPresentation) -> Result<(), GpuError> {
        if let Some(error) = self.generation.unavailable(false) {
            return Err(error);
        }
        if presentation.is_empty() {
            return Ok(());
        }
        let mut state = self.inbox.state.lock().expect("Metal device inbox");
        if state.closed {
            return Err(GpuError::Stopped { may_have_submitted: false });
        }
        state.presentations.push_back(presentation);
        drop(state);
        self.inbox.ready.notify_one();
        Ok(())
    }

    fn capture(&self, list: Arc<DisplayList>, scale: f32, sample: Option<PaintSample>) -> Result<GpuTask<CapturePixels>, GpuError> {
        let lease = self.generation.reserve_readback()?;
        self.enqueue(Lane::Capture, move |worker, reply| worker.capture(list, scale, sample, lease, reply))
    }

    fn shutdown(mut self) {
        self.stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for MetalGpuDevice {
    fn drop(&mut self) {
        self.stop();
    }
}

#[must_use = "retain the lease throughout surface configuration"]
pub struct MetalSurfaceLease {
    inbox: Weak<Inbox>,
}

impl Drop for MetalSurfaceLease {
    fn drop(&mut self) {
        if let Some(inbox) = self.inbox.upgrade() {
            inbox.state.lock().expect("Metal device inbox").paused = false;
            inbox.ready.notify_one();
        }
    }
}

#[derive(Default)]
struct Inbox {
    state: Mutex<InboxState>,
    ready: Condvar,
}

#[derive(Default)]
struct InboxState {
    jobs: Schedule<QueuedJob>,
    presentations: VecDeque<MetalPresentation>,
    paused: bool,
    closed: bool,
}

struct QueuedJob {
    token: Arc<AtomicBool>,
    run: Job,
}

enum Work {
    Job(Job),
    Accept(MetalPresentation),
}

impl DeviceEvents for Inbox {
    fn gpu_event(&self) {
        // Synchronize with the queue's predicate check before signaling.
        drop(self.state.lock().expect("Metal device inbox"));
        self.ready.notify_one();
    }

    fn cancel(&self, token: &Arc<AtomicBool>) {
        let pending = self.state.lock().expect("Metal device inbox").jobs.remove(|job| Arc::ptr_eq(&job.token, token));
        self.ready.notify_one();
        drop(pending);
    }
}

impl Inbox {
    fn close(&self) {
        let abandoned = {
            let mut state = self.state.lock().expect("Metal device inbox");
            let abandoned = std::mem::take(&mut *state);
            state.closed = true;
            abandoned
        };
        self.ready.notify_one();
        drop(abandoned);
    }

    fn next(&self, generation: &DeviceGeneration) -> Option<Work> {
        let mut state = self.state.lock().expect("Metal device inbox");
        loop {
            if state.closed || generation.is_lost() {
                return None;
            }
            if !state.paused {
                if let Some(presentation) = state.presentations.pop_front() {
                    return Some(Work::Accept(presentation));
                }
                if let Some(job) = state.jobs.take(Instant::now(), generation.capacity().pending_submissions, false) {
                    return Some(Work::Job(job.run));
                }
            }
            state = self.ready.wait(state).expect("Metal device inbox");
        }
    }
}

struct Worker {
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    renderer: MetalRenderer,
    generation: Arc<DeviceGeneration>,
    last_command: Option<Retained<Command>>,
    profile: bool,
}

impl Worker {
    fn new(device: Retained<Device>, generation: Arc<DeviceGeneration>, profile: bool) -> Result<Self, String> {
        let queue = device.newCommandQueue().ok_or("Metal could not create the command queue")?;
        let renderer = MetalRenderer::new(device)?;
        Ok(Self { queue, renderer, generation, last_command: None, profile })
    }

    fn run(mut self, inbox: &Inbox) {
        while let Some(work) = inbox.next(&self.generation) {
            autoreleasepool(|_| match work {
                Work::Job(job) => job(&mut self),
                Work::Accept(presentation) => self.renderer.accept_presentation(presentation),
            });
        }
    }

    fn wait_for_submitted(&mut self) {
        if let Some(command) = self.last_command.take() {
            command.waitUntilCompleted();
        }
    }

    fn command(&self) -> Result<Retained<Command>, GpuError> {
        self.queue.commandBuffer().ok_or_else(|| GpuError::Submission("Metal could not create a command buffer".into()))
    }

    fn draw(
        &mut self,
        work: DrawWork<Box<MetalDrawableTarget>>,
        reply: &mut Reply<DeviceDrawnFrame<MetalGpuDevice>>,
    ) -> Result<DeviceDrawnFrame<MetalGpuDevice>, GpuError> {
        let DrawWork { mut target, list, scale, stamp, valid_until, report } = work;
        let live = |reply: &Reply<_>| {
            reply.check_live()?;
            if valid_until.is_some_and(|deadline| Instant::now() >= deadline) {
                return Err(GpuError::Expired);
            }
            Ok(())
        };
        live(reply)?;
        let texture = target.drawable().ok_or_else(|| GpuError::Drawable("the Metal drawable was already submitted".into()))?.texture();
        let command = self.command()?;
        let frame =
            self.renderer.encode(&command, &texture, &list, scale, target.color, Some(stamp.sample)).map_err(GpuError::Recording)?;
        let current = live(reply);
        let stages = current.is_ok().then(|| target.record_presentation(&command)).flatten();
        let progress = FrameProgress::new(stamp);
        let complete = progress.clone();
        let report = report.filter(|_| current.is_ok());
        let mut profile = frame.profile;
        profile.gpu_sampled = stages.is_some() || report.is_some();
        let upload = profile.atlas_upload_bytes + profile.instance_upload_bytes + profile.spatial_upload_bytes;
        let generation = self.generation.clone();
        let submission = generation.begin_submission(upload);
        on_completed(&command, move |command| {
            let succeeded = command.status() == MTLCommandBufferStatus::Completed;
            if succeeded {
                complete.complete();
            } else {
                report_draw_failure(&generation, command);
            }
            let timestamps = (stages.is_some() || report.is_some()).then(|| (command.GPUStartTime(), command.GPUEndTime()));
            drop(submission);
            if let Some((start, end)) = timestamps {
                if let Some(stages) = stages {
                    stages.completed(start, end);
                }
                if let Some(report) = report {
                    let span = (start > 0.0 && end >= start).then(|| Duration::try_from_secs_f64(end - start).ok()).flatten();
                    report(GpuTimings { passes: Vec::new(), span, truncated: false });
                }
            }
        });
        // An encoded frame updates retained backing and atlas state. Submit those commands
        // even if its presentation opportunity expired while the worker was encoding.
        reply.may_have_submitted = true;
        command.commit();
        self.last_command = Some(command);
        current?;
        Ok(DrawnFrame {
            target,
            stamp,
            effects: frame.receipts,
            profile: self.profile.then_some(profile),
            presentation: frame.presentation,
            progress,
        })
    }

    fn capture(
        &mut self,
        list: Arc<DisplayList>,
        scale: f32,
        sample: Option<PaintSample>,
        lease: Arc<ReadbackLease>,
        mut reply: Reply<CapturePixels>,
    ) {
        let result = (|| {
            reply.check_live()?;
            let command = self.command()?;
            let capture = Capture::new(&self.queue.device(), &list, scale).map_err(GpuError::Readback)?;
            let frame = self
                .renderer
                .encode(&command, &capture.texture, &list, scale, octara_render::OutputColor::SrgbStraight, sample)
                .map_err(GpuError::Readback)?;
            let storage = capture.encode_readback(&command, frame.receipts).map_err(GpuError::Readback)?;
            Ok((command, storage, frame.profile))
        })();
        let (command, storage, profile) = match result {
            Ok(recorded) => recorded,
            Err(error) => {
                reply.finish(Err(error));
                return;
            }
        };
        lease.allocated(storage.bytes() as u64);
        let readback_complete = lease.gpu_completion();
        let generation = self.generation.clone();
        let submission =
            generation.begin_submission(profile.atlas_upload_bytes + profile.instance_upload_bytes + profile.spatial_upload_bytes);
        reply.may_have_submitted = true;
        on_completed(&command, move |command| {
            let result = match completion_error(command) {
                Some(error) => {
                    if error.device_lost {
                        generation.mark_metal_lost();
                    }
                    Err(generation.unavailable(true).unwrap_or(GpuError::Readback(error.message)))
                }
                None => Ok(CapturePixels::new(storage.size(), move || storage.unpack(), lease)),
            };
            drop(readback_complete);
            drop(submission);
            reply.finish(result);
        });
        command.commit();
        self.last_command = Some(command);
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        autoreleasepool(|_| self.wait_for_submitted());
    }
}

fn on_completed(command: &Command, complete: impl FnOnce(&Command) + Send + 'static) {
    let complete = Mutex::new(Some(complete));
    let callback = RcBlock::new(move |command: NonNull<Command>| {
        let complete = complete.lock().expect("Metal completion callback").take();
        if let Some(complete) = complete {
            // SAFETY: Metal keeps the command buffer alive through the callback.
            let command = unsafe { command.as_ref() };
            let _ = catch_unwind(AssertUnwindSafe(|| complete(command)));
        }
    });
    // SAFETY: Metal copies the block. Its captured completion state is thread-safe.
    unsafe { command.addCompletedHandler(RcBlock::as_ptr(&callback)) };
}

struct CompletionError {
    message: String,
    device_lost: bool,
    not_permitted: bool,
}

fn completion_error(command: &Command) -> Option<CompletionError> {
    if command.status() == MTLCommandBufferStatus::Completed {
        return None;
    }
    let error = command.error();
    let code = error
        .as_ref()
        .filter(|error| {
            // SAFETY: Metal owns this error-domain constant for the lifetime of the process.
            &*error.domain() == unsafe { MTLCommandBufferErrorDomain }
        })
        .map(|error| MTLCommandBufferError(error.code() as usize));
    Some(CompletionError {
        message: error.map_or_else(|| "Metal command buffer did not complete".into(), |error| error.to_string()),
        device_lost: matches!(code, Some(MTLCommandBufferError::AccessRevoked | MTLCommandBufferError::DeviceRemoved)),
        not_permitted: code == Some(MTLCommandBufferError::NotPermitted),
    })
}

fn report_draw_failure(generation: &DeviceGeneration, command: &Command) {
    if let Some(error) = completion_error(command) {
        if error.device_lost {
            generation.mark_metal_lost();
        } else if !error.not_permitted && !generation.is_stopped() {
            generation.notify(DeviceNotice::Failed(error.message));
        }
    }
}

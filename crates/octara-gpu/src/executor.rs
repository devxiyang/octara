//! The native device worker and its host-facing job handles.

use crate::generation::{DeviceGeneration, DeviceNotice};
use crate::readback::CapturePixels;
use crate::schedule::{Lane, Schedule};
use octara_model::capture::{FrameReceipt, PaintReceipt, PaintSample};
use octara_model::identity::FrameStamp;
use octara_model::paint::DisplayList;
use octara_render::gpu::{
    CanvasUpdates, FrameInput, FrameProfile, FrameTarget, GpuTimings, OutputColor, PendingPresentation, Readback, RecordError,
    with_error_scopes,
};
use octara_render::{GpuRenderer, SurfaceRendererFactory, wgpu};
use std::collections::VecDeque;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak, mpsc};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub enum GpuError {
    Busy,
    Expired,
    WrongGeneration { expected: u64, actual: u64 },
    Stopped { may_have_submitted: bool },
    DeviceLost { generation: u64, may_have_submitted: bool },
    Drawable(String),
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

/// A host-owned drawable sent to the worker for texture access and returned on success.
pub trait DrawTarget: Send + 'static {
    fn texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<(wgpu::Texture, OutputColor), String>;
}

impl DrawTarget for (wgpu::Texture, OutputColor) {
    fn texture(&mut self, _: &wgpu::Device, _: &wgpu::Queue) -> Result<(wgpu::Texture, OutputColor), String> {
        Ok(self.clone())
    }
}

pub struct DrawWork<T> {
    pub target: T,
    pub list: Arc<DisplayList>,
    pub scale: f32,
    pub stamp: FrameStamp,
    pub valid_until: Option<Instant>,
    pub report: Option<Box<dyn FnOnce(GpuTimings) + Send>>,
}

pub struct DrawnFrame<T> {
    pub target: T,
    pub stamp: FrameStamp,
    pub effects: Vec<PaintReceipt>,
    pub profile: Option<FrameProfile>,
    pub presentation: PendingPresentation,
    progress: FrameProgress,
}

impl<T> DrawnFrame<T> {
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
    pub fn gpu_complete(&self) -> bool {
        self.gpu_complete.load(Ordering::Acquire)
    }

    pub fn receipt(&self, presented_at_ns: Option<u64>) -> FrameReceipt {
        FrameReceipt { stamp: self.stamp, gpu_complete: self.gpu_complete(), presented_at_ns }
    }
}

/// The retained canvases from one scene. Each job records at most one canvas update.
pub struct CanvasWork {
    list: Arc<DisplayList>,
    scale: f32,
    sample: PaintSample,
    owner: Weak<()>,
    updates: Option<CanvasUpdates>,
}

impl CanvasWork {
    pub fn new(list: Arc<DisplayList>, scale: f32, sample: PaintSample, owner: Weak<()>) -> Self {
        Self { list, scale, sample, owner, updates: None }
    }

    pub fn is_finished(&self) -> bool {
        self.owner.strong_count() == 0 || self.updates.as_ref().is_some_and(CanvasUpdates::is_empty)
    }
}

/// Access to the drained device. Dropping the lease resumes the executor.
#[must_use = "retain the lease throughout surface configuration"]
pub struct SurfaceLease {
    device: wgpu::Device,
    inbox: Weak<Inbox>,
}

impl SurfaceLease {
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn resume(self) {
        drop(self);
    }
}

impl Drop for SurfaceLease {
    fn drop(&mut self) {
        if let Some(inbox) = self.inbox.upgrade() {
            inbox.resume();
        }
    }
}

/// The native worker owns the renderer, queue admission, and driver polling.
pub struct DeviceExecutor {
    generation: Arc<DeviceGeneration>,
    inbox: Arc<Inbox>,
    worker: Option<thread::JoinHandle<()>>,
}

impl DeviceExecutor {
    /// Starts the device worker with the built-in renderer and an optional surface backend.
    pub fn spawn_for_device(
        device: wgpu::Device,
        queue: wgpu::Queue,
        generation: Arc<DeviceGeneration>,
        factory: Option<&SurfaceRendererFactory>,
    ) -> std::io::Result<Self> {
        Self::spawn_for_device_with_thread_setup(device, queue, generation, factory, || {})
    }

    /// Run host scheduling setup on the device worker before servicing GPU work.
    pub fn spawn_for_device_with_thread_setup(
        device: wgpu::Device,
        queue: wgpu::Queue,
        generation: Arc<DeviceGeneration>,
        factory: Option<&SurfaceRendererFactory>,
        setup: impl FnOnce() + Send + 'static,
    ) -> std::io::Result<Self> {
        let backend = factory.map(|factory| factory(&device));
        let mut renderer = GpuRenderer::with_device(device, queue);
        if let Some(backend) = backend {
            renderer = renderer.with_surface_renderer(backend);
        }
        Self::spawn_with_thread_setup(renderer, generation, setup)
    }

    pub fn spawn(renderer: GpuRenderer, generation: Arc<DeviceGeneration>) -> std::io::Result<Self> {
        Self::spawn_with_thread_setup(renderer, generation, || {})
    }

    fn spawn_with_thread_setup(
        mut renderer: GpuRenderer,
        generation: Arc<DeviceGeneration>,
        setup: impl FnOnce() + Send + 'static,
    ) -> std::io::Result<Self> {
        let inbox = Arc::new(Inbox::default());
        generation.attach(&inbox)?;
        generation.watch_loss(renderer.device());
        let profile_inbox = Arc::downgrade(&inbox);
        renderer.set_profile_wake(move || {
            if let Some(inbox) = profile_inbox.upgrade() {
                inbox.signal_gpu_event();
            }
        });
        let worker_generation = generation.clone();
        let worker_inbox = inbox.clone();
        let worker = thread::Builder::new().name("octara-device".into()).spawn(move || {
            let _ = catch_unwind(AssertUnwindSafe(setup));
            let result = catch_unwind(AssertUnwindSafe(|| Worker::new(renderer, &worker_generation, &worker_inbox).run()));
            let failure = match result {
                Ok(result) => result.err(),
                Err(_) => Some("the device executor panicked while servicing the GPU".into()),
            };
            worker_generation.stop();
            worker_inbox.close();
            if let Some(failure) = failure {
                worker_generation.notify(DeviceNotice::Failed(failure));
            }
        });
        match worker {
            Ok(worker) => Ok(Self { generation, inbox, worker: Some(worker) }),
            Err(error) => {
                generation.stop();
                Err(error)
            }
        }
    }

    pub fn generation(&self) -> &Arc<DeviceGeneration> {
        &self.generation
    }

    pub fn draw<T: DrawTarget>(&self, work: DrawWork<T>) -> Result<GpuTask<DrawnFrame<T>>, GpuError> {
        let expected = self.generation.number();
        if work.stamp.device_generation != expected {
            return Err(GpuError::WrongGeneration { expected, actual: work.stamp.device_generation });
        }
        self.enqueue(Lane::Draw, move |worker, mut reply| {
            let result = worker.draw(work, &mut reply);
            reply.finish(result);
        })
    }

    /// Pause after existing submissions drain. The host configures the surfaces under the lease.
    pub fn drain_surfaces(&self) -> Result<GpuTask<SurfaceLease>, GpuError> {
        self.enqueue(Lane::Drain, |worker, reply| worker.drain = Some(reply))
    }

    /// Put an accepted frame's texture history in the renderer before the next drawing.
    pub fn accept_presentation(&self, presentation: PendingPresentation) -> Result<(), GpuError> {
        if let Some(error) = self.generation.unavailable(false) {
            return Err(error);
        }
        self.inbox.accept(presentation)
    }

    /// Record a bounded resource operation and deliver its result after the GPU completes it.
    pub fn resource<T: Send + 'static>(
        &self,
        record: impl FnOnce(&wgpu::Device, &mut wgpu::CommandEncoder) -> Result<T, String> + Send + 'static,
    ) -> Result<GpuTask<T>, GpuError> {
        let lease = self.generation.reserve_resource()?;
        self.enqueue(Lane::Resource, move |worker, mut reply| {
            let result = worker.record_resource(record, &mut reply);
            match result {
                Ok(value) => {
                    let complete = Arc::new(AtomicBool::new(false));
                    let signal = complete.clone();
                    worker.generation.observe_submission(worker.renderer.queue(), 0, move || signal.store(true, Ordering::Release));
                    worker.resource = Some(Resource {
                        complete,
                        finish: Box::new(move || {
                            drop(lease);
                            reply.finish(Ok(value));
                        }),
                    });
                }
                Err(error) => {
                    drop(lease);
                    reply.finish(Err(error));
                }
            }
        })
    }

    pub fn capture(&self, list: Arc<DisplayList>, scale: f32, sample: Option<PaintSample>) -> Result<GpuTask<CapturePixels>, GpuError> {
        let lease = self.generation.reserve_readback()?;
        self.enqueue(Lane::Capture, move |worker, mut reply| {
            let generation = worker.generation;
            let result = generation.run(|| {
                reply.check_live()?;
                let inbox = Arc::downgrade(worker.inbox);
                let recorded = worker
                    .renderer
                    .record_capture_with_wake(&list, scale, sample, move || {
                        if let Some(inbox) = inbox.upgrade() {
                            inbox.signal_gpu_event();
                        }
                    })
                    .map_err(GpuError::Readback)?;
                lease.allocated(recorded.readback_bytes());
                reply.check_live()?;
                reply.may_have_submitted = true;
                recorded.on_submitted_work_done(lease.gpu_completion());
                recorded.submit().map_err(GpuError::Readback)
            });
            match result.unwrap_or_else(|| Err(reply.stopped())) {
                Ok(readback) => {
                    generation.observe_submission(worker.renderer.queue(), readback.upload_bytes(), || {});
                    worker.readback = Some(PendingCapture { readback, reply, lease });
                }
                Err(error) => reply.finish(Err(error)),
            }
        })
    }

    pub fn prepare_canvas(&self, work: CanvasWork) -> Result<GpuTask<(CanvasWork, bool)>, GpuError> {
        self.enqueue(Lane::Canvas, move |worker, mut reply| {
            let result = worker.prepare_canvas(work, &mut reply);
            reply.finish(result);
        })
    }

    /// Stop taking new work and release the queued jobs.
    pub fn stop(&self) {
        self.generation.stop();
        self.inbox.close();
    }

    /// Stop and join the GPU worker after the host event loop exits.
    pub fn shutdown(mut self) {
        self.stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }

    fn enqueue<T: Send + 'static>(
        &self,
        lane: Lane,
        run: impl FnOnce(&mut Worker<'_>, Reply<T>) + Send + 'static,
    ) -> Result<GpuTask<T>, GpuError> {
        if let Some(error) = self.generation.unavailable(false) {
            return Err(error);
        }
        let (reply, task) = Reply::new(self.generation.clone());
        let token = Some(task.cancelled.clone());
        self.inbox.enqueue(
            lane,
            token,
            Box::new(move |worker| {
                if !reply.cancelled() {
                    run(worker, reply);
                }
            }),
        )?;
        Ok(task)
    }
}

impl Drop for DeviceExecutor {
    fn drop(&mut self) {
        self.stop();
    }
}

#[must_use = "keep the task alive to receive the GPU result"]
pub struct GpuTask<T> {
    receiver: Option<mpsc::Receiver<Result<T, GpuError>>>,
    cancelled: Arc<AtomicBool>,
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

struct Reply<T> {
    sender: Option<mpsc::Sender<Result<T, GpuError>>>,
    cancelled: Arc<AtomicBool>,
    generation: Arc<DeviceGeneration>,
    may_have_submitted: bool,
}

impl<T> Reply<T> {
    fn new(generation: Arc<DeviceGeneration>) -> (Self, GpuTask<T>) {
        let (sender, receiver) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let reply = Self { sender: Some(sender), cancelled: cancelled.clone(), generation: generation.clone(), may_have_submitted: false };
        (reply, GpuTask { receiver: Some(receiver), cancelled, generation })
    }

    fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    fn check_live(&self) -> Result<(), GpuError> {
        match self.generation.unavailable(self.may_have_submitted) {
            Some(error) => Err(error),
            None if self.cancelled() => Err(self.stopped()),
            None => Ok(()),
        }
    }

    fn stopped(&self) -> GpuError {
        self.generation.unavailable(self.may_have_submitted).unwrap_or(GpuError::Stopped { may_have_submitted: self.may_have_submitted })
    }

    fn finish(mut self, result: Result<T, GpuError>) {
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

type Job = Box<dyn FnOnce(&mut Worker<'_>) + Send>;

struct QueuedJob {
    token: Option<Arc<AtomicBool>>,
    run: Job,
}

#[derive(Default)]
pub(super) struct Inbox {
    state: Mutex<InboxState>,
    wake: Condvar,
}

#[derive(Default)]
struct InboxState {
    jobs: Schedule<QueuedJob>,
    presentations: VecDeque<PendingPresentation>,
    gpu_event: bool,
    resume: bool,
    closed: bool,
}

enum Wake {
    Event,
    DriverPoll,
    Job(Job),
    Presentation(PendingPresentation),
    Resume,
    Close,
}

impl Inbox {
    fn enqueue(&self, lane: Lane, token: Option<Arc<AtomicBool>>, run: Job) -> Result<(), GpuError> {
        let mut state = self.state.lock().expect("GPU executor inbox");
        if state.closed {
            return Err(GpuError::Stopped { may_have_submitted: false });
        }
        state.jobs.push(lane, Instant::now(), QueuedJob { token, run });
        self.wake.notify_one();
        Ok(())
    }

    fn accept(&self, presentation: PendingPresentation) -> Result<(), GpuError> {
        let mut state = self.state.lock().expect("GPU executor inbox");
        if state.closed {
            return Err(GpuError::Stopped { may_have_submitted: false });
        }
        state.presentations.push_back(presentation);
        self.wake.notify_one();
        Ok(())
    }

    pub(super) fn signal_gpu_event(&self) {
        self.state.lock().expect("GPU executor inbox").gpu_event = true;
        self.wake.notify_one();
    }

    pub(super) fn cancel(&self, token: &Arc<AtomicBool>) {
        let mut state = self.state.lock().expect("GPU executor inbox");
        let pending = state.jobs.remove(|job| job.token.as_ref().is_some_and(|job| Arc::ptr_eq(job, token)));
        state.gpu_event = true;
        drop(state);
        self.wake.notify_one();
        drop(pending);
    }

    fn resume(&self) {
        self.state.lock().expect("GPU executor inbox").resume = true;
        self.wake.notify_one();
    }

    fn close(&self) {
        let abandoned = {
            let mut state = self.state.lock().expect("GPU executor inbox");
            let abandoned = std::mem::take(&mut *state);
            state.closed = true;
            abandoned
        };
        self.wake.notify_one();
        // Job and presentation drops may themselves signal the inbox.
        drop(abandoned);
    }

    fn next(&self, worker: &Worker<'_>, poll_at: Option<Instant>) -> Wake {
        let mut state = self.state.lock().expect("GPU executor inbox");
        loop {
            if state.closed || worker.generation.is_lost() {
                return Wake::Close;
            }
            if std::mem::take(&mut state.resume) {
                return Wake::Resume;
            }
            if std::mem::take(&mut state.gpu_event) {
                return Wake::Event;
            }
            let now = Instant::now();
            if poll_at.is_some_and(|at| at <= now) {
                return Wake::DriverPoll;
            }
            if !worker.configuring && worker.drain.is_none() {
                if let Some(presentation) = state.presentations.pop_front() {
                    return Wake::Presentation(presentation);
                }
                let capacity = worker.generation.capacity();
                if let Some(job) = state.jobs.take(now, capacity.pending_submissions, worker.resource.is_some()) {
                    return Wake::Job(job.run);
                }
            }
            state = match poll_at {
                Some(at) => self.wake.wait_timeout(state, at.saturating_duration_since(now)).expect("GPU executor inbox").0,
                None => self.wake.wait(state).expect("GPU executor inbox"),
            };
        }
    }
}

const INITIAL_POLL: Duration = Duration::from_millis(2);
const MAX_POLL: Duration = Duration::from_millis(16);

struct DriverPoll {
    interval: Duration,
    at: Option<Instant>,
}

impl Default for DriverPoll {
    fn default() -> Self {
        Self { interval: INITIAL_POLL, at: None }
    }
}

impl DriverPoll {
    fn deadline(&mut self, pending: bool, now: Instant) -> Option<Instant> {
        if pending {
            Some(*self.at.get_or_insert(now + self.interval))
        } else {
            self.reset();
            None
        }
    }

    fn reset(&mut self) {
        *self = Self::default();
    }

    fn polled(&mut self, now: Instant) {
        self.interval = (self.interval * 2).min(MAX_POLL);
        self.at = Some(now + self.interval);
    }
}

struct Resource {
    complete: Arc<AtomicBool>,
    finish: Box<dyn FnOnce() + Send>,
}

struct PendingCapture {
    readback: Readback,
    reply: Reply<CapturePixels>,
    lease: Arc<crate::generation::ReadbackLease>,
}

struct Worker<'a> {
    renderer: GpuRenderer,
    generation: &'a Arc<DeviceGeneration>,
    inbox: &'a Arc<Inbox>,
    drain: Option<Reply<SurfaceLease>>,
    configuring: bool,
    resource: Option<Resource>,
    readback: Option<PendingCapture>,
    profile_queued: bool,
}

impl<'a> Worker<'a> {
    fn new(renderer: GpuRenderer, generation: &'a Arc<DeviceGeneration>, inbox: &'a Arc<Inbox>) -> Self {
        Self { renderer, generation, inbox, drain: None, configuring: false, resource: None, readback: None, profile_queued: false }
    }

    fn run(mut self) -> Result<(), String> {
        let mut poll = DriverPoll::default();
        loop {
            let poll_at = poll.deadline(self.needs_poll(), Instant::now());
            let mut queue_empty = false;
            match self.inbox.next(&self, poll_at) {
                Wake::Close => return Ok(()),
                Wake::Resume => self.configuring = false,
                Wake::Event => {
                    // Queue/map callbacks have already made their results available.
                    poll.reset();
                    if self.drain.is_some() {
                        poll.at = Some(Instant::now());
                    }
                }
                Wake::DriverPoll => {
                    let generation = self.generation;
                    if let Some(status) = generation.run(|| self.renderer.device().poll(wgpu::PollType::Poll)) {
                        let status = status.map_err(|error| format!("cannot poll the GPU device: {error}"))?;
                        queue_empty = status.is_queue_empty();
                    }
                    poll.polled(Instant::now());
                }
                Wake::Presentation(presentation) => self.renderer.accept_presentation(presentation),
                Wake::Job(job) => {
                    job(&mut self);
                    if self.drain.is_some() {
                        poll.at = Some(Instant::now());
                    }
                }
            }
            self.collect();
            if queue_empty && self.generation.capacity().pending_submissions == 0 {
                self.finish_drain();
            }
        }
    }

    fn needs_poll(&self) -> bool {
        !self.configuring
            && (self.drain.is_some()
                || self.generation.gpu_work_pending()
                || self.resource.is_some()
                || self.readback.is_some()
                || self.renderer.profile_needs_poll())
    }

    fn collect(&mut self) {
        if self.configuring || self.generation.unavailable(false).is_some() {
            return;
        }
        // Check loss only after wgpu returns. A queue callback can precede a loss
        // notification during the same poll.
        if let Some(resource) = self.resource.take_if(|resource| resource.complete.load(Ordering::Acquire)) {
            (resource.finish)();
        }
        self.drain.take_if(|reply| reply.cancelled());
        self.readback.take_if(|pending| pending.reply.cancelled());
        if let Some(mapped) = self.readback.as_mut().and_then(|pending| pending.readback.take_mapped()) {
            let PendingCapture { reply, lease, .. } = self.readback.take().expect("the mapped capture was pending");
            reply.finish(mapped.map(|mapped| CapturePixels::new(mapped, lease)).map_err(GpuError::Readback));
        }
        if !self.profile_queued && self.renderer.profile_resolves_ready() {
            self.profile_queued = self.inbox.enqueue(Lane::Profile, None, Box::new(|worker| worker.resolve_profile())).is_ok();
        }
    }

    fn finish_drain(&mut self) {
        if let Some(reply) = self.drain.take() {
            if reply.check_live().is_err() {
                return;
            }
            self.configuring = true;
            reply.finish(Ok(SurfaceLease { device: self.renderer.device().clone(), inbox: Arc::downgrade(self.inbox) }));
        }
    }

    fn draw<T: DrawTarget>(&mut self, work: DrawWork<T>, reply: &mut Reply<DrawnFrame<T>>) -> Result<DrawnFrame<T>, GpuError> {
        let DrawWork { mut target, list, scale, stamp, valid_until, report } = work;
        let alive = || {
            if valid_until.is_some_and(|deadline| Instant::now() >= deadline) {
                return Err(GpuError::Expired);
            }
            Ok(())
        };
        let generation = self.generation;
        generation
            .run(|| {
                reply.check_live()?;
                alive()?;
                let (texture, color) = target.texture(self.renderer.device(), self.renderer.queue()).map_err(GpuError::Drawable)?;
                let encoder =
                    self.renderer.device().create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Octara window frame") });
                let input = FrameInput { list: &list, scale, sample: Some(stamp.sample) };
                let (recorded, profile) =
                    self.renderer.record(encoder, FrameTarget { texture: &texture, color }, input, report).map_err(GpuError::Frame)?;
                reply.check_live()?;
                alive()?;
                let effects = recorded.paint_receipts().to_vec();
                let upload_bytes = recorded.upload_bytes();
                reply.may_have_submitted = true;
                let (_, presentation) = recorded.submit_for_presentation();
                let gpu_complete = Arc::new(AtomicBool::new(false));
                let complete = gpu_complete.clone();
                generation.observe_submission(self.renderer.queue(), upload_bytes, move || complete.store(true, Ordering::Release));
                let progress = FrameProgress { stamp, gpu_complete };
                Ok(DrawnFrame { target, stamp, effects, profile, presentation, progress })
            })
            .unwrap_or_else(|| Err(reply.stopped()))
    }

    fn record_resource<T>(
        &mut self,
        record: impl FnOnce(&wgpu::Device, &mut wgpu::CommandEncoder) -> Result<T, String>,
        reply: &mut Reply<T>,
    ) -> Result<T, GpuError> {
        let generation = self.generation;
        generation
            .run(|| {
                reply.check_live()?;
                let device = self.renderer.device();
                let (recorded, error) = with_error_scopes(device, || {
                    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Octara resource") });
                    let value = record(device, &mut encoder)?;
                    Ok::<_, String>((encoder.finish(), value))
                });
                if let Some(error) = error {
                    return Err(GpuError::Recording(error.to_string()));
                }
                let (commands, value) = recorded.map_err(GpuError::Recording)?;
                reply.check_live()?;
                reply.may_have_submitted = true;
                self.renderer.submit_commands([commands]).map_err(|error| GpuError::Submission(error.to_string()))?;
                Ok(value)
            })
            .unwrap_or_else(|| Err(reply.stopped()))
    }

    fn prepare_canvas(&mut self, mut work: CanvasWork, reply: &mut Reply<(CanvasWork, bool)>) -> Result<(CanvasWork, bool), GpuError> {
        let generation = self.generation;
        generation
            .run(|| {
                reply.check_live()?;
                if work.is_finished() {
                    return Ok((work, false));
                }
                let updates = work.updates.get_or_insert_with(|| self.renderer.canvas_updates(&work.list, work.scale));
                let should_submit = || {
                    let current = work.owner.strong_count() != 0 && reply.check_live().is_ok();
                    reply.may_have_submitted = current;
                    current
                };
                let submitted = self
                    .renderer
                    .prepare_next_canvas(updates, work.sample.revision, should_submit)
                    .map_err(|error| GpuError::Submission(error.to_string()))?;
                if submitted {
                    generation.observe_submission(self.renderer.queue(), 0, || {});
                }
                Ok((work, submitted))
            })
            .unwrap_or_else(|| Err(reply.stopped()))
    }

    fn resolve_profile(&mut self) {
        self.profile_queued = false;
        if self.generation.unavailable(false).is_some() {
            return;
        }
        let result = self.generation.run(|| self.renderer.flush_profile_resolves());
        match result {
            Some(Ok(count)) if count != 0 => self.generation.observe_submission(self.renderer.queue(), 0, || {}),
            Some(Err(error)) => self.generation.notify(DeviceNotice::Failed(format!("cannot resolve GPU profile: {error}"))),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use octara_model::capture::{PaintOutcome, PaintReceipt};
    use octara_model::geometry::Size;
    use octara_model::identity::TreeId;
    use octara_model::theme::rgb;

    #[test]
    fn driver_poll_backs_off_only_while_work_is_pending() {
        let now = Instant::now();
        let mut poll = DriverPoll::default();
        assert_eq!(poll.deadline(false, now), None);
        assert_eq!(poll.deadline(true, now), Some(now + Duration::from_millis(2)));
        for millis in [4, 8, 16, 16] {
            poll.polled(now);
            assert_eq!(poll.deadline(true, now), Some(now + Duration::from_millis(millis)));
        }
        poll.reset();
        assert_eq!(poll.deadline(true, now), Some(now + Duration::from_millis(2)));
        assert_eq!(poll.deadline(false, now), None);
    }

    fn wait<T>(task: GpuTask<T>, wakes: &mpsc::Receiver<DeviceNotice>) -> T {
        wait_result(task, wakes).unwrap()
    }

    fn wait_result<T>(mut task: GpuTask<T>, wakes: &mpsc::Receiver<DeviceNotice>) -> Result<T, GpuError> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(result) = task.try_take() {
                return result;
            }
            wakes.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("the worker wakes the host after GPU work");
        }
    }

    #[test]
    fn surface_lease_drains_existing_work_and_excludes_new_gpu_jobs() {
        let Some(renderer) = GpuRenderer::new() else { return };
        let (wake, wakes) = mpsc::channel();
        let generation = Arc::new(DeviceGeneration::new(
            7,
            Some(Arc::new(move |_, event| {
                let _ = wake.send(event);
            })),
        ));
        let executor = DeviceExecutor::spawn(renderer, generation.clone()).unwrap();
        let (started, recording) = mpsc::channel();
        let first = executor
            .resource(move |_, _| {
                started.send(()).unwrap();
                Ok(21)
            })
            .unwrap();
        recording.recv_timeout(Duration::from_secs(5)).unwrap();
        let surface = wait(executor.drain_surfaces().unwrap(), &wakes);
        assert_eq!(wait(first, &wakes), 21);
        assert_eq!(generation.capacity().pending_submissions, 0);

        let (started, recording) = mpsc::channel();
        let second = executor
            .resource(move |_, _| {
                started.send(()).unwrap();
                Ok(42)
            })
            .unwrap();
        assert!(matches!(recording.recv_timeout(Duration::from_millis(30)), Err(mpsc::RecvTimeoutError::Timeout)));
        let list = Arc::new(DisplayList { size: Size::new(1.0, 1.0), background: rgb(0), parts: vec![] });
        let capture = executor.capture(list, 1.0, None).unwrap();
        assert!(!generation.capacity().readback_available);
        drop(capture);
        assert!(generation.capacity().readback_available);
        surface.resume();
        assert_eq!(wait(second, &wakes), 42);
        executor.shutdown();
    }

    #[test]
    fn draw_returns_a_stamped_target_with_independent_completion() {
        let Some(renderer) = GpuRenderer::new() else { return };
        let (wake, wakes) = mpsc::channel();
        let generation = Arc::new(DeviceGeneration::new(
            7,
            Some(Arc::new(move |_, event| {
                let _ = wake.send(event);
            })),
        ));
        let target = renderer.device().create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: 3, height: 2, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let executor = DeviceExecutor::spawn(renderer, generation).unwrap();
        let stamp = FrameStamp {
            tree: TreeId::new([0, 0], 1),
            frame: 1,
            surface_generation: 2,
            device_generation: 7,
            native_rev: 0,
            sample: PaintSample { revision: 5, sampled_at: Duration::from_millis(16) },
        };
        let list = Arc::new(DisplayList { size: Size::new(3.0, 2.0), background: rgb(0xff0000), parts: vec![] });
        let work = DrawWork { target: (target, OutputColor::SrgbStraight), list, scale: 1.0, stamp, valid_until: None, report: None };
        let frame = wait(executor.draw(work).unwrap(), &wakes);
        assert_eq!(frame.stamp, stamp);
        let progress = frame.progress();
        executor.accept_presentation(frame.presentation).unwrap();
        drop(frame.target);
        wait(executor.drain_surfaces().unwrap(), &wakes).resume();
        assert_eq!(progress.receipt(Some(12)), FrameReceipt { stamp, gpu_complete: true, presented_at_ns: Some(12) });
        executor.shutdown();
    }

    #[test]
    fn mapped_capture_releases_its_slot_after_unpacking_padded_rows() {
        let Some(renderer) = GpuRenderer::new() else { return };
        let (wake, wakes) = mpsc::channel();
        let generation = Arc::new(DeviceGeneration::new(
            7,
            Some(Arc::new(move |_, event| {
                let _ = wake.send(event);
            })),
        ));
        let executor = DeviceExecutor::spawn(renderer, generation.clone()).unwrap();
        let list = Arc::new(DisplayList { size: Size::new(3.0, 2.0), background: rgb(0xff0000), parts: vec![] });
        let capture = executor.capture(list.clone(), 1.0, None).unwrap();
        assert!(matches!(executor.capture(list.clone(), 1.0, None), Err(GpuError::Busy)));
        let pixels = wait(capture, &wakes);
        assert_eq!(pixels.size(), [3, 2]);
        assert!(!generation.capacity().readback_available);
        let result = pixels.finish().unwrap();
        assert_eq!(result.picture.pixel(2, 1), Some([255, 0, 0, 255]));
        assert_eq!(result.picture, octara_render::CpuRenderer::new().render(&list, 1.0));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !generation.capacity().readback_available {
            wakes.recv_timeout(deadline.saturating_duration_since(Instant::now())).unwrap();
        }
        executor.shutdown();
    }

    #[test]
    fn surface_renderer_records_capture_defers_or_reports_an_error() {
        use octara_render::{SurfaceFrame, SurfaceRenderer};

        struct Surface;

        impl SurfaceRenderer for Surface {
            fn record(&mut self, frame: SurfaceFrame<'_>) -> Result<Option<Vec<PaintReceipt>>, String> {
                let Some(sample) = frame.input.sample else {
                    return Ok(None);
                };
                if sample.revision == 3 {
                    return Err("surface paint failed".into());
                }
                assert_eq!(frame.input.scale, 2.0);
                assert_eq!(frame.target.color, OutputColor::SrgbStraight);
                let view = frame.target.texture.create_view(&Default::default());
                frame.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color { r: 1.0, g: 0.0, b: 1.0, a: 1.0 }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                Ok(Some(vec![PaintReceipt { source: Default::default(), outcome: PaintOutcome::Rendered }]))
            }
        }

        let Some(renderer) = GpuRenderer::new() else { return };
        let (wake, wakes) = mpsc::channel();
        let generation = Arc::new(DeviceGeneration::new(
            7,
            Some(Arc::new(move |_, event| {
                let _ = wake.send(event);
            })),
        ));
        let factory: SurfaceRendererFactory = Arc::new(|_| Box::new(Surface));
        let (setup, initialized) = mpsc::channel();
        let executor = DeviceExecutor::spawn_for_device_with_thread_setup(
            renderer.device().clone(),
            renderer.queue().clone(),
            generation,
            Some(&factory),
            move || setup.send(thread::current().name().unwrap_or_default().to_owned()).unwrap(),
        )
        .unwrap();
        assert_eq!(initialized.recv_timeout(Duration::from_secs(5)).unwrap(), "octara-device");
        let list = Arc::new(DisplayList { size: Size::new(3.0, 2.0), background: rgb(0xff0000), parts: vec![] });
        let sample = PaintSample { revision: 2, sampled_at: Duration::from_millis(16) };
        let capture = wait(executor.capture(list.clone(), 2.0, Some(sample)).unwrap(), &wakes).finish().unwrap();
        assert_eq!(capture.picture.pixel(5, 3), Some([255, 0, 255, 255]));
        assert_eq!(capture.effects, [PaintReceipt { source: Default::default(), outcome: PaintOutcome::Rendered }]);
        let fallback = wait(executor.capture(list.clone(), 2.0, None).unwrap(), &wakes).finish().unwrap();
        assert_eq!(fallback.picture.pixel(5, 3), Some([255, 0, 0, 255]));
        assert!(fallback.effects.is_empty());
        let failed = executor.capture(list, 2.0, Some(PaintSample { revision: 3, ..sample })).unwrap();
        assert!(
            matches!(wait_result(failed, &wakes), Err(GpuError::Readback(message)) if message == "cannot record surface: surface paint failed")
        );
        executor.shutdown();
    }
}

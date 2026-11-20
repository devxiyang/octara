use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

/// CPU timings and structural work recorded by a call to [`super::GpuRenderer::record_profiled`].
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameProfile {
    /// Display-list preparation and queued uploads, measured on the CPU.
    pub prepare: Duration,
    /// Command recording, including author drawing, its validation and late backdrop setup.
    pub encode: Duration,
    /// Draw calls in Octara's UI and layer passes, including composites and fallback draws.
    pub draw_batches: u32,
    /// Draw calls that use an author material pipeline.
    pub material_batches: u32,
    /// Render passes for the UI and opacity layers, including restarts after a backdrop.
    pub render_passes: u32,
    /// Area of the opacity layer attachments in device pixels, including allocation rounding.
    pub layer_pixels: u64,
    /// Area of the author canvas and backdrop output targets recorded this frame, in device pixels.
    pub canvas_pixels: u64,
    /// Area of backdrop inputs copied this frame, in device pixels.
    pub backdrop_copy_pixels: u64,
    /// Backdrop input textures allocated by successful operations this frame.
    pub backdrop_allocations: u32,
    /// Backdrop input textures reused by successful operations this frame.
    pub backdrop_reuses: u32,
    /// Largest cached backdrop-input capacity observed this frame, in descriptor bytes.
    pub backdrop_cache_peak_bytes: u64,
    /// Backdrop draws from a mount's last submitted output.
    pub retained_backdrops: u32,
    /// Canvas draws that reused a compatible submitted output without encoding local work.
    pub retained_canvases: u32,
    /// Timestamp commands were recorded. Submitting the frame starts an asynchronous readback.
    pub gpu_sampled: bool,
}

/// The GPU work measured by one timestamp pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuPassKind {
    /// A render pass on the caller's target.
    Ui,
    /// A render pass on an offscreen opacity layer.
    Layer,
    /// The target clear and author commands for a canvas.
    Canvas,
    /// The target clear for a canvas, measured with pass-level queries.
    CanvasClear,
    /// The copy of already drawn pixels into a backdrop input.
    BackdropCopy,
    /// The target clear and author commands for a backdrop.
    Backdrop,
    /// The target clear for a backdrop, measured with pass-level queries.
    BackdropClear,
    /// A render or compute pass named by its author.
    Author(&'static str),
}

impl GpuPassKind {
    /// A stable name for traces and measurement reports.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ui => "ui",
            Self::Layer => "layer",
            Self::Canvas => "canvas",
            Self::CanvasClear => "canvas_clear",
            Self::BackdropCopy => "backdrop_copy",
            Self::Backdrop => "backdrop",
            Self::BackdropClear => "backdrop_clear",
            Self::Author(name) => name,
        }
    }
}

/// A decoded duration for one GPU region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuPassTiming {
    pub kind: GpuPassKind,
    pub duration: Duration,
}

/// The timestamp results from one submitted frame.
#[derive(Debug)]
pub struct GpuTimings {
    /// The valid timing pairs in submission order. A kind can occur more than once.
    /// Author pass timings may overlap a surrounding canvas or backdrop timing.
    pub passes: Vec<GpuPassTiming>,
    /// Time between the renderer's first and last command markers, when supported. Queue
    /// uploads, caller commands and timestamp resolve/readback are outside the markers.
    pub span: Option<Duration>,
    /// The frame exceeded the limit on measured regions.
    pub truncated: bool,
}

const MAX_READBACKS: usize = 3;
const MAX_REGIONS: usize = 128;
const QUERIES: u32 = 2 * (MAX_REGIONS as u32 + 1);
const TICK_SIZE: u64 = std::mem::size_of::<u64>() as u64;

#[derive(Default)]
pub(super) struct TimestampPool {
    slots: Vec<Arc<TimestampSlot>>,
    deferred: Vec<DeferredResolve>,
    metal: Option<bool>,
}

impl TimestampPool {
    pub(super) fn pending(&self) -> bool {
        self.slots.iter().any(|slot| slot.busy.load(Ordering::Acquire))
    }

    pub(super) fn begin(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> Option<TimestampFrame> {
        let features = device.features();
        let period = f64::from(queue.get_timestamp_period());
        if !features.contains(wgpu::Features::TIMESTAMP_QUERY) || !period.is_finite() || period <= 0.0 {
            return None;
        }
        let slot = if let Some(slot) = self.slots.iter().find(|slot| !slot.busy.load(Ordering::Acquire)) {
            slot.clone()
        } else if self.slots.len() < MAX_READBACKS {
            let slot = Arc::new(TimestampSlot::new(device));
            self.slots.push(slot.clone());
            slot
        } else {
            return None;
        };
        slot.busy.store(true, Ordering::Relaxed);
        let lease = SlotLease(Arc::downgrade(&slot));
        let metal = *self.metal.get_or_insert_with(|| device.adapter_info().backend == wgpu::Backend::Metal);
        Some(TimestampFrame {
            slot,
            lease,
            period,
            metal,
            encoder_writes: features.contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS),
            kinds: Vec::new(),
            truncated: false,
        })
    }

    fn defer(&mut self, queue: &wgpu::Queue, resolve: wgpu::CommandBuffer, sample: PendingTimestamps) {
        // Metal's stage-boundary counters can return zero if their resolve is submitted before
        // the sampling work completes. See https://github.com/gfx-rs/wgpu/issues/9414.
        sample.frame.slot.ready_to_resolve.store(false, Ordering::Relaxed);
        let slot = Arc::downgrade(&sample.frame.slot);
        queue.on_submitted_work_done(move || {
            if let Some(slot) = slot.upgrade() {
                slot.ready_to_resolve.store(true, Ordering::Release);
            }
        });
        self.deferred.push(DeferredResolve { resolve, sample });
    }

    pub(super) fn poll(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        if !self.pending() {
            return false;
        }
        if device.poll(wgpu::PollType::Poll).is_err() {
            self.deferred.clear();
            self.slots.clear();
            return false;
        }
        let ready = self.deferred.extract_if(.., |entry| entry.sample.frame.slot.ready_to_resolve.load(Ordering::Acquire));
        for DeferredResolve { resolve, sample } in ready {
            queue.submit([resolve]);
            sample.map();
        }
        self.pending()
    }
}

struct DeferredResolve {
    resolve: wgpu::CommandBuffer,
    sample: PendingTimestamps,
}

struct TimestampSlot {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    busy: AtomicBool,
    ready_to_resolve: AtomicBool,
}

impl TimestampSlot {
    fn new(device: &wgpu::Device) -> Self {
        let buffer = |label, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: u64::from(QUERIES) * TICK_SIZE,
                usage,
                mapped_at_creation: false,
            })
        };
        Self {
            queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("octara frame timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: QUERIES,
            }),
            resolve: buffer("octara timestamp resolve", wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC),
            readback: buffer("octara timestamp readback", wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ),
            busy: AtomicBool::new(false),
            ready_to_resolve: AtomicBool::new(false),
        }
    }
}

// An abandoned frame releases its reservation when dropped. An in-flight map owns the same
// reservation until it unmaps, with a weak slot reference to avoid retaining its own buffer.
struct SlotLease(Weak<TimestampSlot>);

impl Drop for SlotLease {
    fn drop(&mut self) {
        if let Some(slot) = self.0.upgrade() {
            slot.busy.store(false, Ordering::Release);
        }
    }
}

pub(super) struct TimestampFrame {
    slot: Arc<TimestampSlot>,
    lease: SlotLease,
    period: f64,
    metal: bool,
    encoder_writes: bool,
    kinds: Vec<GpuPassKind>,
    truncated: bool,
}

impl TimestampFrame {
    pub(super) fn begin_span(&self, encoder: &mut wgpu::CommandEncoder) {
        if self.encoder_writes {
            encoder.write_timestamp(&self.slot.queries, 0);
        }
    }

    fn query_count(&self) -> u32 {
        2 * (self.kinds.len() as u32 + u32::from(self.encoder_writes))
    }

    fn reserve(&mut self, kind: GpuPassKind) -> Option<u32> {
        if self.kinds.len() == MAX_REGIONS {
            self.truncated = true;
            return None;
        }
        let start = self.query_count();
        self.kinds.push(kind);
        Some(start)
    }

    pub(super) fn pass(&mut self, kind: GpuPassKind) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        let start = self.reserve(kind)?;
        Some(wgpu::RenderPassTimestampWrites {
            query_set: &self.slot.queries,
            beginning_of_pass_write_index: Some(start),
            end_of_pass_write_index: Some(start + 1),
        })
    }

    pub(super) fn compute_pass(&mut self, kind: GpuPassKind) -> Option<wgpu::ComputePassTimestampWrites<'_>> {
        let start = self.reserve(kind)?;
        Some(wgpu::ComputePassTimestampWrites {
            query_set: &self.slot.queries,
            beginning_of_pass_write_index: Some(start),
            end_of_pass_write_index: Some(start + 1),
        })
    }

    pub(super) fn clear_pass(&mut self, kind: GpuPassKind) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        if self.encoder_writes {
            return None;
        }
        self.pass(kind)
    }

    /// Measure an encoder region when available, allowing nested pass measurements.
    pub(super) fn commands<T>(
        frame: Option<&mut Self>,
        encoder: &mut wgpu::CommandEncoder,
        kind: GpuPassKind,
        record: impl FnOnce(&mut wgpu::CommandEncoder, Option<&mut Self>) -> T,
    ) -> T {
        let Some(frame) = frame else {
            return record(encoder, None);
        };
        let start = if frame.encoder_writes { frame.reserve(kind) } else { None };
        if let Some(start) = start {
            encoder.write_timestamp(&frame.slot.queries, start);
        }
        let output = record(encoder, Some(frame));
        if let Some(start) = start {
            encoder.write_timestamp(&frame.slot.queries, start + 1);
        }
        output
    }

    pub(super) fn checkpoint(&self) -> (usize, bool) {
        (self.kinds.len(), self.truncated)
    }

    pub(super) fn discard_since(&mut self, (count, truncated): (usize, bool)) {
        self.kinds.truncate(count);
        self.truncated = truncated;
    }

    pub(super) fn resolve(
        self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        report: impl FnOnce(GpuTimings) + Send + 'static,
    ) -> Option<PendingTimestamps> {
        let count = self.query_count();
        if count == 0 {
            return None;
        }
        if self.encoder_writes {
            encoder.write_timestamp(&self.slot.queries, 1);
        }
        let mut deferred =
            self.metal.then(|| device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("octara timestamp resolve") }));
        let target = deferred.as_mut().unwrap_or(encoder);
        let bytes = u64::from(count) * TICK_SIZE;
        target.resolve_query_set(&self.slot.queries, 0..count, &self.slot.resolve, 0);
        target.copy_buffer_to_buffer(&self.slot.resolve, 0, &self.slot.readback, 0, Some(bytes));
        let resolve = deferred.map(wgpu::CommandEncoder::finish);
        Some(PendingTimestamps { frame: self, resolve, bytes, report: Mutex::new(Box::new(report)) })
    }
}

pub(super) struct PendingTimestamps {
    frame: TimestampFrame,
    resolve: Option<wgpu::CommandBuffer>,
    bytes: u64,
    // Keeps the renderer and recorded frame Sync while accepting a Send callback. The callback
    // is taken by value without locking.
    report: Mutex<Box<dyn FnOnce(GpuTimings) + Send>>,
}

impl PendingTimestamps {
    pub(super) fn submitted(mut self, pool: &mut TimestampPool, queue: &wgpu::Queue) {
        if let Some(resolve) = self.resolve.take() {
            pool.defer(queue, resolve, self);
        } else {
            self.map();
        }
    }

    fn map(self) {
        let Self { frame, bytes, report, .. } = self;
        let report = report.into_inner().expect("the callback is owned by the sample");
        let TimestampFrame { slot, lease, period, metal, encoder_writes, kinds, truncated } = frame;
        slot.readback.map_async(wgpu::MapMode::Read, ..bytes, move |result| {
            if result.is_err() {
                return;
            }
            let Some(slot) = lease.0.upgrade() else { return };
            let timings = slot.readback.get_mapped_range(..bytes).ok().map(|data| {
                let mut ticks = data.as_chunks::<8>().0.iter().map(|bytes| u64::from_ne_bytes(*bytes));
                let mut duration = || {
                    let (start, end) = (ticks.next()?, ticks.next()?);
                    if end == u64::MAX || (metal && (start == 0 || end == 0)) {
                        return None;
                    }
                    let nanos = end.checked_sub(start)? as f64 * period;
                    Duration::try_from_secs_f64(nanos / 1e9).ok()
                };
                let span = if encoder_writes { duration() } else { None };
                let passes = kinds.into_iter().filter_map(|kind| Some(GpuPassTiming { kind, duration: duration()? })).collect();
                GpuTimings { passes, span, truncated }
            });
            slot.readback.unmap();
            if let Some(timings) = timings {
                report(timings);
            }
            drop(lease);
        });
    }
}

pub(super) struct FrameMeasurement {
    pub(super) profile: FrameProfile,
    pub(super) timestamps: Option<TimestampFrame>,
    pub(super) canvas_encode: Duration,
}

impl FrameMeasurement {
    pub(super) fn pass(&mut self, kind: GpuPassKind) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        self.profile.render_passes += 1;
        self.timestamps.as_mut()?.pass(kind)
    }
}

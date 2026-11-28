use std::time::Duration;

/// CPU timings and structural work of a recorded frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameProfile {
    /// Display-list and resource preparation, measured on the CPU.
    pub prepare: Duration,
    /// The frame reused a submitted plan, its sorted instance buffers and their bindings.
    pub retained_prepare: bool,
    /// Fraction of the root attachment redrawn, from zero for reuse to one for a full draw.
    pub damage_area_ratio: f64,
    /// The frame reused submitted backing pixels outside its damage rectangle.
    pub partial_raster: bool,
    /// Bytes of instance data, clips and projection uniforms staged for submission.
    pub instance_upload_bytes: u64,
    /// Bytes of spatial offsets staged for submission.
    pub spatial_upload_bytes: u64,
    /// Command recording, including author drawing, its validation and late backdrop setup.
    pub encode: Duration,
    /// Draw calls in Octara's UI and layer passes, including composites and fallback draws.
    pub draw_batches: u32,
    /// Draw calls that use an author material pipeline.
    pub material_batches: u32,
    /// Render passes for the UI, opacity layers and output resolve, including opaque cores and backdrop restarts.
    pub render_passes: u32,
    /// Area of the opacity layer attachments in device pixels, including allocation rounding.
    pub layer_pixels: u64,
    /// Bytes of texture arrays referenced by this frame.
    pub atlas_bytes: u64,
    /// Texel bytes prepared for submission with this frame.
    pub atlas_upload_bytes: u64,
    /// Pixels copied or resolved into the caller's target, or zero for direct drawing.
    pub output_pixels: u64,
    /// Area of the author canvas, backdrop and transition output targets recorded this frame, in device pixels.
    pub canvas_pixels: u64,
    /// Area of backdrop inputs copied this frame, in device pixels.
    pub backdrop_copy_pixels: u64,
    /// Backdrop input textures allocated by successful operations this frame.
    pub backdrop_allocations: u32,
    /// Backdrop input textures reused by successful operations this frame.
    pub backdrop_reuses: u32,
    /// Largest cached backdrop-input capacity observed this frame, in descriptor bytes.
    pub backdrop_cache_peak_bytes: u64,
    /// Backdrop draws from a mount's last output accepted by the host.
    pub retained_backdrops: u32,
    /// Canvas draws that reused a compatible submitted output without encoding local work.
    pub retained_canvases: u32,
    /// Timestamp commands were recorded. Submitting the frame starts an asynchronous readback.
    pub gpu_sampled: bool,
}

/// The GPU work measured by one timestamp pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuPassKind {
    /// The root UI pass and its continuations.
    Ui,
    /// A render pass on an offscreen opacity layer.
    Layer,
    /// The pass that resolves intermediate scene pixels to the caller's target.
    Output,
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
    /// The target clear and author commands for a transition.
    Transition,
    /// The target clear for a transition, measured with pass-level queries.
    TransitionClear,
    /// A render or compute pass named by its author.
    Author(&'static str),
}

impl GpuPassKind {
    /// A stable name for traces and measurement reports.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ui => "ui",
            Self::Layer => "layer",
            Self::Output => "output",
            Self::Canvas => "canvas",
            Self::CanvasClear => "canvas_clear",
            Self::BackdropCopy => "backdrop_copy",
            Self::Backdrop => "backdrop",
            Self::BackdropClear => "backdrop_clear",
            Self::Transition => "transition",
            Self::TransitionClear => "transition_clear",
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

//! The GPU renderer: wgpu, instanced batches for each target, and author-defined paint programs.
//! Glyphs and paths are rasterized as by the reference renderer
//! ([`GlyphCache`], [`paths`]) and packed into an alpha atlas (masks; a path larger than a tile
//! takes several) and an RGBA atlas (color glyphs). When an atlas fills up, every atlas starts
//! over and the frame is built again with only what it draws.

use crate::glyphs::{GlyphCache, SUBPIXEL_STEPS};
use crate::paths::{self, MaskKey};
use crate::shape::{ClipNode, ClipStack};
use bytemuck::{Pod, Zeroable};
use octara_primitives::capture::{PaintFallback, PaintOutcome, PaintReceipt, PaintSample, PaintedPicture, Painter, Picture};
use octara_primitives::paint::{CustomPaint, DisplayList, GlyphRun, ImagePaint, Item, PaintProgram, PathPaint, Quad, Rect, Shadow};
use octara_primitives::theme::Color;
use rustc_hash::FxHashMap;
use std::alloc::Layout;
use std::ops::Range;
use std::sync::{Weak, mpsc};
use std::time::{Duration, Instant};

mod custom;
mod outputs;
mod profile;
pub use custom::{Backdrop, CanvasFrame, backdrop, canvas, material, retained_canvas};
use custom::{CanvasMode, CanvasTarget};
pub use outputs::PendingPresentation;
use outputs::{BackdropChange, BackdropHistory, CanvasCache, CanvasKey, OutputChanges};
use profile::{FrameMeasurement, PendingTimestamps, TimestampPool};
pub use profile::{FrameProfile, GpuPassKind, GpuPassTiming, GpuTimings};

const ATLAS: u32 = 2048;
/// The mask atlas's size where the device allows it (paths can be window-sized).
const MASK_ATLAS: u32 = 4096;
/// The largest piece of a path's mask in the atlas.
const TILE: u32 = 1024;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// No clip: a rect far larger than any window.
const NO_CLIP: [f32; 4] = [-1.0e6, -1.0e6, 2.0e6, 2.0e6];
const KIND_QUAD: u32 = 0;
const KIND_MASK: u32 = 1;
const KIND_COLOR: u32 = 2;
const KIND_SHADOW: u32 = 3;
const KIND_IMAGE: u32 = 4;
const KIND_LAYER: u32 = 5;
const KIND_PATH: u32 = 6;
const KIND_CANVAS: u32 = 7;
const GEOMETRY_SHADER: &str = include_str!("gpu/geometry.wgsl");

struct ReadbackLayout {
    row: u32,
    bytes: u64,
    rgba_len: usize,
}

fn readback_layout(width: u32, height: u32) -> Option<ReadbackLayout> {
    if width == 0 || height == 0 {
        return None;
    }
    let count = crate::pixels::count(width, height)?;
    let rgba_len = Layout::array::<[u8; 4]>(count).ok()?.size();
    let row = (u64::from(width) * 4).div_ceil(256) * 256;
    let row = u32::try_from(row).ok()?;
    let bytes = u64::from(row).checked_mul(u64::from(height))?;
    // The mapped buffer must also fit a Rust byte slice on this host.
    Layout::array::<u8>(usize::try_from(bytes).ok()?).ok()?;
    Some(ReadbackLayout { row, bytes, rgba_len })
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Inst {
    rect: [f32; 4],
    fill: [f32; 4],
    border_color: [f32; 4],
    border: [f32; 4],
    clip: [f32; 4],
    params: InstParams,
    /// Atlas coordinates, or the four author parameters for a material.
    uv: [f32; 4],
    /// A gradient's end color (straight); `fill` is its start.
    grad_color: [f32; 4],
    /// A gradient's angle (degrees), start and end fractions, and 1 if there is one.
    grad: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default, Pod, Zeroable)]
struct InstParams {
    /// Float bits alongside integer flags in a Uint32x4 vertex attribute.
    radius: f32,
    kind: u32,
    clip_head: u32,
    filled: u32,
}

impl Inst {
    fn visible_bounds(&self, window: &Rect) -> Rect {
        // Match the vertex shader's expansion for antialiasing and shadows.
        let grow = match self.params.kind {
            KIND_QUAD | KIND_IMAGE | KIND_CANVAS => 1.0,
            KIND_SHADOW => 3.0 * self.border[0] + 1.0,
            _ => 0.0,
        };
        let [x, y, w, h] = self.rect;
        let bounds = Rect::new(x - grow, y - grow, w + 2.0 * grow, h + 2.0 * grow);
        let [x, y, w, h] = self.clip;
        if w <= 0.0 || h <= 0.0 {
            return Rect::default();
        }
        // Rounded clips antialias past their nominal edge.
        let clip = Rect::new(x - 1.0, y - 1.0, w + 2.0, h + 2.0);
        bounds.intersect(&clip).intersect(window)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    size: [f32; 2],
    origin: [f32; 2],
}

/// A pass's pixel bounds in window coordinates, before rounding its allocation up.
#[derive(Clone, Copy, Default)]
struct PassBounds {
    origin: [u32; 2],
    size: [u32; 2],
}

impl PassBounds {
    fn rect(self) -> [f32; 4] {
        [self.origin[0] as f32, self.origin[1] as f32, self.size[0] as f32, self.size[1] as f32]
    }

    fn allocation(self, window: [u32; 2]) -> [u32; 2] {
        std::array::from_fn(|axis| self.size[axis].max(1).next_multiple_of(64).min(window[axis].max(1)))
    }
}

/// The instances drawn into one target: the window, or a layer composited at `opacity`.
#[derive(Default)]
struct Pass {
    insts: Vec<Inst>,
    /// Batches in painter order. Gaps use the ordinary pipeline and atlas bindings.
    special: Vec<Batch>,
    opacity: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Draw {
    Standard,
    Cutout,
    Layer(usize),
    Material(custom::Material),
    Canvas(usize),
    Backdrop(usize),
}

#[derive(Debug, PartialEq, Eq)]
struct Batch {
    instances: Range<usize>,
    draw: Draw,
}

impl Pass {
    fn push_special(&mut self, instance: Inst, draw: Draw) {
        let start = self.insts.len();
        self.insts.push(instance);
        if let Some(last) = self.special.last_mut()
            && last.instances.end == start
            && last.draw == draw
        {
            last.instances.end += 1;
        } else {
            self.special.push(Batch { instances: start..start + 1, draw });
        }
    }
}

struct CanvasRequest {
    pass: usize,
    instance: usize,
    paint: CanvasPaint,
    retained_mount: Option<Weak<()>>,
}

struct CanvasPaint {
    program: PaintProgram,
    fallback: Inst,
    fallback_source: octara_primitives::identity::NodeId,
    size: [u32; 2],
    params: [f32; 4],
    rect: Rect,
    radius: f32,
    sampled_at: Option<Duration>,
    receipt: usize,
}

struct BackdropRequest {
    paint: CanvasPaint,
    mount: Weak<()>,
    exiting: bool,
    target: usize,
}

#[derive(Default)]
struct PaintRequests {
    canvases: Vec<CanvasRequest>,
    backdrops: Vec<BackdropRequest>,
    receipts: Vec<PaintReceipt>,
}

#[derive(Clone, Copy)]
struct CopyAttachment {
    extent: [u32; 2],
    copyable: bool,
}

impl CopyAttachment {
    fn from_texture(texture: &wgpu::Texture, format: wgpu::TextureFormat) -> Self {
        Self {
            extent: [texture.width(), texture.height()],
            copyable: texture.usage().contains(wgpu::TextureUsages::COPY_SRC)
                && texture.format().remove_srgb_suffix() == format.remove_srgb_suffix()
                && texture.dimension() == wgpu::TextureDimension::D2
                && texture.sample_count() == 1
                && texture.mip_level_count() == 1
                && texture.depth_or_array_layers() == 1,
        }
    }
}

/// A cropped copy expressed in target and canvas coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BackdropRegion {
    origin: [u32; 2],
    size: [u32; 2],
    offset: [i32; 2],
}

impl BackdropRegion {
    fn crop(attachment: CopyAttachment, bounds: PassBounds, paint: &CanvasPaint) -> Option<Self> {
        if !attachment.copyable {
            return None;
        }
        let [x, y, _, _] = paint.fallback.rect;
        let [x, y] = [x.floor(), y.floor()];
        let [width, height] = paint.size;
        let canvas = Rect::new(x, y, width as f32, height as f32);
        let [cx, cy, cw, ch] = paint.fallback.clip;
        let [tx, ty, tw, th] = bounds.rect();
        let target = Rect::new(tx, ty, tw.min(attachment.extent[0] as f32), th.min(attachment.extent[1] as f32));
        let crop = canvas.intersect(&Rect::new(cx, cy, cw, ch)).intersect(&target);
        if crop.is_empty() {
            return None;
        }
        let [left, top] = [crop.x.floor(), crop.y.floor()];
        Some(Self {
            origin: [(left - tx) as u32, (top - ty) as u32],
            size: [(crop.right().ceil() - left) as u32, (crop.bottom().ceil() - top) as u32],
            offset: [(left - x) as i32, (top - y) as i32],
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BackdropInput {
    /// End the current pass, copy the painter prefix, then resume with Load.
    Prefix(BackdropRegion),
    Accepted,
    Unavailable,
}

struct PlannedBackdrop {
    request: BackdropRequest,
    input: BackdropInput,
    fallback: u32,
}

struct EncodedBackdrop {
    view: wgpu::TextureView,
    prefix_commands: Option<wgpu::CommandBuffer>,
    outcome: PaintOutcome,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Attachment {
    Output,
    Layer(usize),
}

struct TargetPlan {
    attachment: Attachment,
    batches: Range<usize>,
}

#[derive(Debug, PartialEq, Eq)]
struct PlannedBatch {
    draw: Draw,
    instances: Range<u32>,
}

struct FramePlan {
    targets: Vec<TargetPlan>,
    batches: Vec<PlannedBatch>,
    backdrops: Vec<PlannedBackdrop>,
}

impl FramePlan {
    fn new(
        passes: &[Pass],
        bounds: &[PassBounds],
        instance_offsets: &[u32],
        backdrops: Vec<BackdropRequest>,
        fallback_offset: u32,
        output: CopyAttachment,
    ) -> Self {
        let mut batches = Vec::new();
        let targets = passes
            .iter()
            .enumerate()
            .rev()
            .map(|(index, pass)| {
                let start = batches.len();
                let base = instance_offsets[index];
                let mut from = base;
                for batch in &pass.special {
                    let instances = base + batch.instances.start as u32..base + batch.instances.end as u32;
                    if from < instances.start {
                        batches.push(PlannedBatch { draw: Draw::Standard, instances: from..instances.start });
                    }
                    from = instances.end;
                    batches.push(PlannedBatch { draw: batch.draw, instances });
                }
                let end = base + pass.insts.len() as u32;
                if from < end {
                    batches.push(PlannedBatch { draw: Draw::Standard, instances: from..end });
                }
                let attachment = if index == 0 { Attachment::Output } else { Attachment::Layer(index) };
                TargetPlan { attachment, batches: start..batches.len() }
            })
            .collect();
        let window = bounds[0].size;
        let backdrops = backdrops
            .into_iter()
            .enumerate()
            .map(|(index, request)| {
                let target = bounds[request.target];
                let attachment =
                    if request.target == 0 { output } else { CopyAttachment { extent: target.allocation(window), copyable: true } };
                let input = if request.exiting {
                    BackdropInput::Accepted
                } else {
                    BackdropRegion::crop(attachment, target, &request.paint).map_or(BackdropInput::Unavailable, BackdropInput::Prefix)
                };
                PlannedBackdrop { request, input, fallback: fallback_offset + index as u32 }
            })
            .collect();
        Self { targets, batches, backdrops }
    }
}

struct PreparedFrame {
    plan: FramePlan,
    background: wgpu::Color,
    layer_views: Vec<wgpu::TextureView>,
    atlases: [wgpu::TextureView; 3],
    bind: wgpu::BindGroup,
    layer_binds: Vec<wgpu::BindGroup>,
    canvas_binds: Vec<wgpu::BindGroup>,
}

#[derive(Default)]
struct FrameRecording<'a> {
    before: Vec<wgpu::CommandBuffer>,
    /// Captures reuse cached canvases and accepted backdrop history without publishing outputs.
    outputs: Option<OutputChanges>,
    receipts: Vec<PaintReceipt>,
    measurement: Option<&'a mut FrameMeasurement>,
}

impl FrameRecording<'_> {
    fn pass(&mut self, kind: GpuPassKind) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        self.measurement.as_deref_mut()?.pass(kind)
    }
}

fn measure_draws(frame: &PreparedFrame, profile: &mut FrameProfile) {
    profile.draw_batches = frame.plan.batches.len() as u32;
    profile.material_batches = frame.plan.batches.iter().filter(|batch| matches!(batch.draw, Draw::Material(_))).count() as u32;
    profile.layer_pixels =
        frame.layer_views.iter().map(|view| u64::from(view.texture().width()) * u64::from(view.texture().height())).sum();
}

/// Size the inner layers first, then use their bounds in the parent's composites.
fn bound_passes(passes: &mut [Pass], size: [u32; 2]) -> Vec<PassBounds> {
    let window = Rect::new(0.0, 0.0, size[0] as f32, size[1] as f32);
    let mut bounds = vec![PassBounds::default(); passes.len()];
    bounds[0].size = size;
    for (index, pass) in passes.iter_mut().enumerate().rev() {
        for batch in &pass.special {
            if let Draw::Layer(layer) = batch.draw {
                pass.insts[batch.instances.start].rect = bounds[layer].rect();
            }
        }
        if index == 0 {
            break;
        }
        let (mut left, mut top, mut right, mut bottom) = (window.w, window.h, 0.0_f32, 0.0_f32);
        for inst in &pass.insts {
            let r = inst.visible_bounds(&window);
            if !r.is_empty() {
                left = left.min(r.x);
                top = top.min(r.y);
                right = right.max(r.right());
                bottom = bottom.max(r.bottom());
            }
        }
        if right > left && bottom > top {
            let (x, y) = (left.floor() as u32, top.floor() as u32);
            bounds[index] = PassBounds { origin: [x, y], size: [right.ceil() as u32 - x, bottom.ceil() as u32 - y] };
        }
    }
    bounds
}

/// A shelf packer for one atlas texture.
struct Shelf {
    size: u32,
    x: u32,
    y: u32,
    row: u32,
}

impl Shelf {
    fn new(size: u32) -> Shelf {
        Shelf { size, x: 0, y: 0, row: 0 }
    }
    fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if w > self.size || h > self.size {
            return None;
        }
        if self.x + w > self.size {
            self.x = 0;
            self.y += self.row;
            self.row = 0;
        }
        if self.y + h > self.size {
            return None;
        }
        let at = (self.x, self.y);
        self.x += w + 1;
        self.row = self.row.max(h + 1);
        Some(at)
    }
}

type AtlasKey = (u64, u32, u32, u32, u8, bool, Vec<i16>);

#[derive(Clone, Copy)]
struct Slot {
    x: u32,
    y: u32,
    left: i32,
    top: i32,
    width: u32,
    height: u32,
    color: bool,
}

/// A piece of a path's mask in the mask atlas: its atlas origin, its offset in the mask, its size.
#[derive(Clone, Copy)]
struct Tile {
    ax: u32,
    ay: u32,
    dx: u32,
    dy: u32,
    w: u32,
    h: u32,
}

fn straight(c: Color) -> [f32; 4] {
    c.components
}

/// A display list and the output geometry used to record it.
#[derive(Clone, Copy)]
pub struct FrameInput<'a> {
    pub list: &'a DisplayList,
    pub scale: f32,
    /// Output extent in device pixels.
    pub size: (u32, u32),
    /// The host's settled drawing revision and sample time, when available.
    pub sample: Option<PaintSample>,
}

/// A submitted offscreen frame and its pending pixel readback.
#[must_use = "check the readback for the finished picture"]
pub struct Readback {
    submission: wgpu::SubmissionIndex,
    pending: Option<PendingReadback>,
}

struct PendingReadback {
    buffer: wgpu::Buffer,
    mapping: mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
    layout: ReadbackLayout,
    width: u32,
    height: u32,
    bgra: bool,
    effects: Vec<PaintReceipt>,
}

impl Readback {
    /// Takes the mapped picture or error once available. The host drives mapping callbacks by
    /// polling the device or submitting more work. Returns `None` while pending or after a
    /// previous call consumed the result.
    pub fn try_finish(&mut self) -> Option<Result<PaintedPicture, String>> {
        let mapped = match self.pending.as_ref()?.mapping.try_recv() {
            Ok(result) => result.map_err(|error| format!("capture readback mapping failed: {error}")),
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(error) => Err(format!("capture readback callback did not report completion: {error}")),
        };
        let pending = self.pending.take().expect("the readback was pending");
        Some(mapped.and_then(|()| pending.paint()))
    }
}

impl PendingReadback {
    fn paint(self) -> Result<PaintedPicture, String> {
        let Self { buffer, layout, width, height, bgra, effects, .. } = self;
        let data = buffer.get_mapped_range(..).map_err(|e| format!("capture readback buffer access failed: {e}"))?;
        let mut rgba = Vec::with_capacity(layout.rgba_len);
        for line in data.chunks_exact(layout.row as usize) {
            let line = &line[..width as usize * 4];
            for p in line.as_chunks::<4>().0 {
                // Premultiplied to straight alpha.
                let a = p[3];
                let un = |v: u8| {
                    if a == 0 {
                        0
                    } else if a == 255 {
                        v
                    } else {
                        ((v as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8
                    }
                };
                let (r, b) = if bgra { (p[2], p[0]) } else { (p[0], p[2]) };
                rgba.extend_from_slice(&[un(r), un(p[1]), un(b), a]);
            }
        }
        drop(data);
        buffer.unmap();
        Ok(PaintedPicture { picture: Picture { width, height, rgba }, effects })
    }
}

/// A frame's recorded commands and the exclusive renderer borrow protecting its staged data.
///
/// Submit or drop this frame before using the renderer again. Dropping it discards the encoder.
/// Its uploads remain queued for the next submission, including atlas entries cached during
/// preparation.
#[must_use = "submit the recorded frame to execute its commands"]
pub struct RecordedFrame<'renderer> {
    before: Vec<wgpu::CommandBuffer>,
    encoder: wgpu::CommandEncoder,
    device: &'renderer wgpu::Device,
    queue: &'renderer wgpu::Queue,
    canvas_cache: &'renderer mut CanvasCache,
    backdrop_history: &'renderer BackdropHistory,
    changes: OutputChanges,
    revision: Option<u64>,
    receipts: Vec<PaintReceipt>,
    timestamp_pool: &'renderer mut TimestampPool,
    timestamps: Option<PendingTimestamps>,
}

impl RecordedFrame<'_> {
    /// Borrows the frame's encoder to append commands after the Octara draw, such as a readback
    /// copy. Leave the encoder in the frame for submission.
    pub fn encoder(&mut self) -> &mut wgpu::CommandEncoder {
        &mut self.encoder
    }

    /// Outcomes in display-list order for the visible author draws recorded in this frame.
    pub fn paint_receipts(&self) -> &[PaintReceipt] {
        &self.receipts
    }

    /// Finishes the encoder and submits the frame with its staged uploads. Retained canvases
    /// become reusable at submission. Use [`Self::submit_for_presentation`] to retain a frame's
    /// backdrop outputs for host acceptance.
    pub fn submit(self) -> wgpu::SubmissionIndex {
        self.submit_for_presentation().0
    }

    /// Submits the frame and returns its candidate backdrop outputs. Give the token to
    /// [`GpuRenderer::accept_presentation`] once the host accepts the frame.
    pub fn submit_for_presentation(self) -> (wgpu::SubmissionIndex, PendingPresentation) {
        let submit = || self.queue.submit(self.before.into_iter().chain([self.encoder.finish()]));
        let submission = if !self.changes.has_fresh() {
            submit()
        } else {
            let (submission, error) = custom::with_error_scopes(self.device, submit);
            if let Some(error) = error {
                panic!("Octara frame submission failed: {error}");
            }
            submission
        };
        let OutputChanges { canvases, backdrops } = self.changes;
        self.canvas_cache.submit(canvases, self.revision);
        let pending = self.backdrop_history.pending(backdrops, self.revision);
        if let Some(timestamps) = self.timestamps {
            timestamps.submitted(self.timestamp_pool, self.queue);
        }
        (submission, pending)
    }
}

fn render_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    shader: &wgpu::ShaderModule,
    fragment: &str,
    label: &str,
    blend: wgpu::BlendState,
) -> wgpu::RenderPipeline {
    let attributes = wgpu::vertex_attr_array![
        0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Uint32x4, 6 => Float32x4,
        7 => Float32x4, 8 => Float32x4
    ];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Inst>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &attributes,
            })],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format, blend: Some(blend), write_mask: wgpu::ColorWrites::ALL })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn color_pass<'encoder>(
    encoder: &'encoder mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
    timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
) -> wgpu::RenderPass<'encoder> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("octara"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
        })],
        timestamp_writes,
        ..Default::default()
    })
}

fn draw_batch(
    pass: &mut wgpu::RenderPass<'_>,
    pipeline: &wgpu::RenderPipeline,
    bind: &wgpu::BindGroup,
    projection: &[u32],
    instances: Range<u32>,
) {
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind, projection);
    pass.draw(0..6, instances);
}

/// Renders display lists with the GPU.
pub struct GpuRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    /// The same shader blending destination-out: clears toward transparent by coverage.
    cutout: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    globals: wgpu::Buffer,
    globals_stride: usize,
    clip_nodes: wgpu::Buffer,
    mask: wgpu::Texture,
    color: wgpu::Texture,
    images: wgpu::Texture,
    /// Layer targets, reused while their allocation covers the same size bucket.
    layers: Vec<wgpu::Texture>,
    /// A 1×1 texture bound where no layer is.
    no_layer: wgpu::Texture,
    /// Atlas positions by image identity and sampled version.
    image_slots: FxHashMap<(u64, u64), (u32, u32)>,
    shelves: [Shelf; 3],
    slots: FxHashMap<AtlasKey, Option<Slot>>,
    path_slots: FxHashMap<MaskKey, Vec<Tile>>,
    /// An atlas ran out of room while building this frame.
    overflow: bool,
    glyphs: GlyphCache,
    instances: Option<(wgpu::Buffer, u64)>,
    programs: custom::Programs,
    canvas_cache: CanvasCache,
    backdrop_history: BackdropHistory,
    timestamps: TimestampPool,
    format: wgpu::TextureFormat,
}

impl GpuRenderer {
    /// A renderer on the default adapter, or `None` when there is no usable GPU.
    pub fn new() -> Option<GpuRenderer> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
        Some(Self::with_device(device, queue, FORMAT))
    }

    /// A renderer on an existing device (a window's), drawing into textures of `format`. Use a
    /// non-sRGB format (e.g. `Bgra8Unorm`): colors are blended in sRGB-encoded space.
    pub fn with_device(device: wgpu::Device, queue: wgpu::Queue, format: wgpu::TextureFormat) -> GpuRenderer {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("octara"),
            source: wgpu::ShaderSource::Wgsl(concat!(include_str!("gpu/geometry.wgsl"), include_str!("shader.wgsl")).into()),
        });
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("octara"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<Globals>() as u64),
                    },
                    count: None,
                },
                texture_entry(1),
                texture_entry(2),
                texture_entry(3),
                texture_entry(4),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<ClipNode>() as u64),
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("octara"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let cutout_blend = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let pipeline =
            render_pipeline(&device, &pipeline_layout, format, &shader, "fs", "octara", wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING);
        let cutout = render_pipeline(
            &device,
            &pipeline_layout,
            format,
            &shader,
            "fs",
            "octara cutout",
            wgpu::BlendState { color: cutout_blend, alpha: cutout_blend },
        );
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("octara globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_stride = std::mem::size_of::<Globals>().next_multiple_of(device.limits().min_uniform_buffer_offset_alignment as usize);
        let clip_nodes = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("octara clips"),
            size: std::mem::size_of::<ClipNode>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mask_size = device.limits().max_texture_dimension_2d.min(MASK_ATLAS);
        let atlas = |format, label, size| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let mask = atlas(wgpu::TextureFormat::R8Unorm, "octara mask atlas", mask_size);
        let color = atlas(wgpu::TextureFormat::Rgba8Unorm, "octara color atlas", ATLAS);
        let images = atlas(wgpu::TextureFormat::Rgba8Unorm, "octara image atlas", ATLAS);
        let no_layer = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("octara no layer"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        GpuRenderer {
            device,
            queue,
            pipeline,
            cutout,
            layout,
            pipeline_layout,
            globals,
            globals_stride,
            clip_nodes,
            mask,
            color,
            images,
            layers: Vec::new(),
            no_layer,
            image_slots: FxHashMap::default(),
            shelves: [Shelf::new(mask_size), Shelf::new(ATLAS), Shelf::new(ATLAS)],
            slots: FxHashMap::default(),
            path_slots: FxHashMap::default(),
            overflow: false,
            glyphs: GlyphCache::default(),
            instances: None,
            programs: custom::Programs::default(),
            canvas_cache: CanvasCache::default(),
            backdrop_history: BackdropHistory::default(),
            timestamps: TimestampPool::default(),
            format,
        }
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Every atlas empty again.
    fn reset_atlases(&mut self) {
        for s in &mut self.shelves {
            *s = Shelf::new(s.size);
        }
        self.slots.clear();
        self.path_slots.clear();
        self.image_slots.clear();
    }

    fn upload(&self, texture: &wgpu::Texture, (x, y): (u32, u32), (w, h): (u32, u32), bpp: u32, data: &[u8], stride: u32) {
        let bytes_per_row = stride.checked_mul(bpp).expect("atlas upload row exceeds the pixel coordinate range");
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d { x, y, z: 0 }, aspect: wgpu::TextureAspect::All },
            data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bytes_per_row), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    }

    /// The tiles of a path's mask in the mask atlas, rasterizing and uploading it the first time.
    fn path_tiles(&mut self, p: &PathPaint, at: &paths::Placed) -> Option<Vec<Tile>> {
        if let Some(t) = self.path_slots.get(&at.key) {
            return Some(t.clone());
        }
        let mask = paths::rasterize(p, at);
        let mut tiles = Vec::new();
        for dy in (0..mask.height).step_by(TILE as usize) {
            for dx in (0..mask.width).step_by(TILE as usize) {
                let (w, h) = (TILE.min(mask.width - dx), TILE.min(mask.height - dy));
                let Some((ax, ay)) = self.shelves[0].alloc(w, h) else {
                    self.overflow = true;
                    return None;
                };
                let start = dy as usize * mask.width as usize + dx as usize;
                let end = start + (h - 1) as usize * mask.width as usize + w as usize;
                self.upload(&self.mask, (ax, ay), (w, h), 1, &mask.data[start..end], mask.width);
                tiles.push(Tile { ax, ay, dx, dy, w, h });
            }
        }
        self.path_slots.insert(at.key, tiles.clone());
        Some(tiles)
    }

    /// The recorded image version's slot, uploading its pixels on the first use.
    fn image_slot(&mut self, i: &ImagePaint) -> Option<(u32, u32, u32, u32)> {
        let f = i.image.frame();
        let shelf = &mut self.shelves[2];
        // Repacking cannot make room for an image that exceeds the atlas dimensions.
        if f.width == 0 || f.height == 0 || f.width > shelf.size || f.height > shelf.size {
            return None;
        }
        let key = (i.image.source().id(), f.version);
        if let Some(&(x, y)) = self.image_slots.get(&key) {
            return Some((x, y, f.width, f.height));
        }
        let Some(at) = shelf.alloc(f.width, f.height) else {
            self.overflow = true;
            return None;
        };
        self.upload(&self.images, at, (f.width, f.height), 4, &f.rgba, f.width);
        self.image_slots.insert(key, at);
        Some((at.0, at.1, f.width, f.height))
    }

    /// The glyph's place in an atlas, rasterizing and uploading it the first time.
    fn slot(&mut self, run: &GlyphRun, glyph: u32, size: f32, subpixel: u8) -> Option<Slot> {
        let key = (run.font.data.id(), run.font.index, glyph, size.to_bits(), subpixel, run.embolden, run.coords.clone());
        if let Some(s) = self.slots.get(&key) {
            return *s;
        }
        // A blank glyph is remembered as `None`; a full atlas is not (the frame starts over).
        let Some(img) = self.glyphs.get(&run.font, glyph, size, subpixel, run.embolden, &run.coords).cloned() else {
            self.slots.insert(key, None);
            return None;
        };
        let which = usize::from(img.color);
        let Some((x, y)) = self.shelves[which].alloc(img.width, img.height) else {
            self.overflow = true;
            return None;
        };
        let (texture, bpp) = if img.color { (&self.color, 4) } else { (&self.mask, 1) };
        self.upload(texture, (x, y), (img.width, img.height), bpp, &img.data, img.width);
        let slot = Some(Slot { x, y, left: img.left, top: img.top, width: img.width, height: img.height, color: img.color });
        self.slots.insert(key, slot);
        slot
    }

    /// The instances of each pass: the window's first, then one per layer (a layer's own layers
    /// come after it, so drawing the passes last to first draws every layer before its use). When
    /// an atlas fills up, every atlas starts over and the frame is built again (uploads land in
    /// order before the draw, so the second build's win); what still does not fit is not drawn.
    fn passes(&mut self, input: FrameInput<'_>) -> (Vec<Pass>, PaintRequests, ClipStack) {
        self.overflow = false;
        let passes = self.build_passes(input);
        if !self.overflow {
            return passes;
        }
        self.reset_atlases();
        let passes = self.build_passes(input);
        self.overflow = false;
        passes
    }

    fn build_passes(&mut self, input: FrameInput<'_>) -> (Vec<Pass>, PaintRequests, ClipStack) {
        let FrameInput { list, scale, size, sample } = input;
        let mut passes = vec![Pass::default()];
        let mut requests = PaintRequests::default();
        let mut open: Vec<usize> = vec![0];
        let mut clips = ClipStack::default();
        let window = Rect::new(0.0, 0.0, size.0 as f32, size.1 as f32);
        for item in &list.items() {
            let clip_state = clips.current();
            let clip = clip_state.bounds.map_or(NO_CLIP, |c| [c.x, c.y, c.w, c.h]);
            let clip_head = clip_state.head;
            let current = *open.last().expect("a pass");
            let out = &mut passes[current].insts;
            match item {
                Item::Shadow(s) => out.push(shadow(s, scale, clip, clip_head)),
                Item::Quad(q) => out.push(quad(q, scale, clip, clip_head)),
                Item::Custom(paint) => {
                    let fallback = quad(&paint.fallback, scale, clip, clip_head);
                    if fallback.visible_bounds(&window).is_empty() {
                        continue;
                    }
                    let mut instance = custom_quad(paint, &fallback);
                    let source = paint.fallback.source;
                    let receipt = requests.receipts.len();
                    requests
                        .receipts
                        .push(PaintReceipt { source, outcome: PaintOutcome::Fallback { reason: PaintFallback::UnsupportedProgram } });
                    if let Some(&material) = paint.program.downcast_ref::<custom::Material>() {
                        if self.programs.prepare_material(&self.device, &self.pipeline_layout, self.format, material, source) {
                            requests.receipts[receipt].outcome = PaintOutcome::Rendered;
                            passes[current].push_special(instance, Draw::Material(material));
                            continue;
                        }
                        requests.receipts[receipt].outcome = PaintOutcome::Fallback { reason: PaintFallback::PreparationFailed };
                    } else if let Some(canvas) = paint.program.downcast_ref::<custom::Canvas>() {
                        let Some(size) = custom::canvas_size(instance.rect, self.device.limits().max_texture_dimension_2d) else {
                            requests.receipts[receipt].outcome = PaintOutcome::Fallback { reason: PaintFallback::InvalidSize };
                            out.push(fallback);
                            continue;
                        };
                        instance.params.kind = KIND_CANVAS;
                        let retained = canvas.mode == CanvasMode::Retained;
                        let (rect, radius, sampled_at) = if retained {
                            let rect = paint.fallback.rect;
                            (Rect::new(0.0, 0.0, rect.w, rect.h), paint.fallback.radius, None)
                        } else {
                            let [x, y, w, h] = fallback.rect.map(|value| value / scale);
                            (Rect::new(x, y, w, h), fallback.params.radius / scale, sample.map(|sample| sample.sampled_at))
                        };
                        let entry = CanvasPaint {
                            program: paint.program.clone(),
                            fallback,
                            fallback_source: source,
                            size,
                            params: paint.params,
                            rect,
                            radius,
                            sampled_at,
                            receipt,
                        };
                        let draw = if canvas.mode == CanvasMode::Backdrop {
                            let index = requests.backdrops.len();
                            requests.backdrops.push(BackdropRequest {
                                paint: entry,
                                mount: paint.mount.downgrade(),
                                exiting: paint.exiting,
                                target: current,
                            });
                            Draw::Backdrop(index)
                        } else {
                            let index = requests.canvases.len();
                            let retained_mount = (retained && CanvasCache::can_retain(size)).then(|| paint.mount.downgrade());
                            requests.canvases.push(CanvasRequest { pass: current, instance: out.len(), paint: entry, retained_mount });
                            Draw::Canvas(index)
                        };
                        passes[current].push_special(instance, draw);
                        continue;
                    }
                    out.push(fallback);
                }
                Item::Image(i) => {
                    if let Some(at) = self.image_slot(i) {
                        out.push(image(i, at, scale, clip, clip_head));
                    }
                }
                Item::Glyphs(run) => {
                    let size = run.size * scale;
                    for g in run.placed() {
                        let (gx, gy) = (g.x * scale, g.y * scale);
                        let (ix, fx) = (gx.floor(), gx - gx.floor());
                        let subpixel = ((fx * SUBPIXEL_STEPS).floor() as u8).min(SUBPIXEL_STEPS as u8 - 1);
                        let Some(s) = self.slot(run, g.id, size, subpixel) else { continue };
                        let (ox, oy) = (ix as i32 + s.left, gy.round() as i32 - s.top);
                        out.push(Inst {
                            rect: [ox as f32, oy as f32, s.width as f32, s.height as f32],
                            fill: straight(run.color),
                            border_color: [0.0; 4],
                            border: [0.0; 4],
                            clip,
                            params: InstParams { kind: if s.color { KIND_COLOR } else { KIND_MASK }, clip_head, ..InstParams::default() },
                            uv: [s.x as f32, s.y as f32, 0.0, 0.0],
                            grad_color: [0.0; 4],
                            grad: [0.0; 4],
                        });
                    }
                }
                Item::Path(p) => {
                    let window = paths::window(size, clip_state.bounds.map(|c| (c.x, c.y, c.w, c.h)));
                    let Some(at) = paths::place(p, scale, window) else { continue };
                    let Some(tiles) = self.path_tiles(p, &at) else { continue };
                    let out = &mut passes[current].insts;
                    let b = p.bounds;
                    let (fill, grad_color, grad) = match p.gradient {
                        Some(g) => (straight(g.from.0), straight(g.to.0), [g.angle, g.from.1, g.to.1, 1.0]),
                        None => (straight(p.color), [0.0; 4], [0.0; 4]),
                    };
                    for t in tiles {
                        out.push(Inst {
                            rect: [(at.x + t.dx as i32) as f32, (at.y + t.dy as i32) as f32, t.w as f32, t.h as f32],
                            fill,
                            border_color: [0.0; 4],
                            border: [b.x * scale, b.y * scale, b.w * scale, b.h * scale],
                            clip,
                            params: InstParams { kind: KIND_PATH, clip_head, ..InstParams::default() },
                            uv: [t.ax as f32, t.ay as f32, 0.0, 0.0],
                            grad_color,
                            grad,
                        });
                    }
                }
                // An underlay view the platform shows: its hole, drawn destination-out.
                Item::Native { source, rect, radius, cut: true, .. } => {
                    let hole = Quad {
                        source: *source,
                        rect: *rect,
                        radius: *radius,
                        fill: Some(Color::BLACK),
                        gradient: None,
                        border: [0.0; 4],
                        border_color: Color::TRANSPARENT,
                    };
                    passes[current].push_special(quad(&hole, scale, clip, clip_head), Draw::Cutout);
                }
                // Otherwise its placeholder or snapshot, drawn before, shows.
                Item::Native { .. } => {}
                Item::PushClip { rect, radius, .. } => clips.push(*rect, *radius, scale),
                Item::PopClip => clips.pop(),
                Item::PushLayer { opacity, .. } => {
                    passes.push(Pass { opacity: *opacity, ..Pass::default() });
                    open.push(passes.len() - 1);
                }
                Item::PopLayer => {
                    if open.len() < 2 {
                        continue;
                    }
                    let layer = open.pop().expect("a layer");
                    let parent = *open.last().expect("a pass");
                    let opacity = passes[layer].opacity;
                    // The layer covers the target; what clips it was applied inside.
                    passes[parent].push_special(
                        Inst {
                            fill: [0.0, 0.0, 0.0, opacity],
                            clip: NO_CLIP,
                            params: InstParams { kind: KIND_LAYER, ..InstParams::default() },
                            ..Inst::zeroed()
                        },
                        Draw::Layer(layer),
                    );
                }
            }
        }
        (passes, requests, clips)
    }

    /// Records the display list into the given encoder, using `view` as a render attachment.
    ///
    /// The caller can record earlier commands into the encoder before passing it here. Preparation
    /// stages queue writes into the renderer's reusable buffers and atlases. These writes execute
    /// before all commands in the next queue submission, including any earlier encoder commands.
    /// Coordinate access to the shared queue so the returned frame is submitted or dropped before
    /// other work is staged or submitted. Finish any open debug groups first. Author canvases
    /// use separate command buffers in the same submission. A backdrop runs after its group's
    /// preceding draws. Accept the frame's [`PendingPresentation`] to keep its backdrop outputs
    /// for their mounts' exit animations.
    /// `size` is the view's extent in device pixels.
    pub fn record(
        &mut self,
        encoder: wgpu::CommandEncoder,
        dl: &DisplayList,
        scale: f32,
        view: &wgpu::TextureView,
        size: (u32, u32),
    ) -> RecordedFrame<'_> {
        self.record_sampled(encoder, view, FrameInput { list: dl, scale, size, sample: None }, None).0
    }

    /// Records a frame with CPU measurements and, when available, GPU timestamps. The rules for
    /// submission, caller commands and backdrop history are the same as for [`Self::record`].
    ///
    /// Enable `TIMESTAMP_QUERY` when creating the device for render and compute pass timestamps.
    /// Author passes opt in through [`CanvasFrame::begin_render_pass`] or
    /// [`CanvasFrame::begin_compute_pass`]. `TIMESTAMP_QUERY_INSIDE_ENCODERS` adds the frame's
    /// command span, full canvas and backdrop timings, and backdrop copies when supported.
    /// If the device lacks timestamps or all readback slots are occupied, CPU measurements
    /// still describe the frame. Dropping the frame discards its GPU sample.
    ///
    /// The callback receives the timing results after submission and a successful asynchronous
    /// buffer map. Call [`Self::poll_profile`] to advance pending samples. A failed map or a
    /// dropped renderer can discard a sample. Keep the callback short so it can run during a
    /// device poll or queue call.
    pub fn record_profiled(
        &mut self,
        encoder: wgpu::CommandEncoder,
        dl: &DisplayList,
        scale: f32,
        view: &wgpu::TextureView,
        size: (u32, u32),
        report: impl FnOnce(GpuTimings) + Send + 'static,
    ) -> (RecordedFrame<'_>, FrameProfile) {
        self.record_profiled_input(encoder, view, FrameInput { list: dl, scale, size, sample: None }, report)
    }

    /// Records a host frame with its settled drawing sample and optional profiling callback.
    pub fn record_sampled(
        &mut self,
        encoder: wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        input: FrameInput<'_>,
        report: Option<Box<dyn FnOnce(GpuTimings) + Send>>,
    ) -> (RecordedFrame<'_>, Option<FrameProfile>) {
        if let Some(report) = report {
            let (frame, profile) = self.record_profiled_input(encoder, view, input, report);
            (frame, Some(profile))
        } else {
            let recording = FrameRecording { outputs: Some(OutputChanges::default()), ..Default::default() };
            (self.record_frame(encoder, view, input, recording), None)
        }
    }

    /// Keeps a submitted frame's backdrop outputs for exit animations once the host accepts it.
    ///
    /// # Panics
    /// Panics if the token contains outputs from a different renderer.
    pub fn accept_presentation(&mut self, pending: PendingPresentation) {
        self.backdrop_history.accept(pending);
    }

    fn record_profiled_input(
        &mut self,
        mut encoder: wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        input: FrameInput<'_>,
        report: impl FnOnce(GpuTimings) + Send + 'static,
    ) -> (RecordedFrame<'_>, FrameProfile) {
        let mut measurement = FrameMeasurement {
            profile: FrameProfile::default(),
            timestamps: self.timestamps.begin(&self.device, &self.queue),
            canvas_encode: Duration::ZERO,
        };
        if let Some(timestamps) = &measurement.timestamps {
            timestamps.begin_span(&mut encoder);
        }
        let recording =
            FrameRecording { outputs: Some(OutputChanges::default()), measurement: Some(&mut measurement), ..Default::default() };
        let mut frame = self.record_frame(encoder, view, input, recording);
        frame.timestamps = measurement.timestamps.and_then(|timestamps| timestamps.resolve(frame.device, &mut frame.encoder, report));
        measurement.profile.gpu_sampled = frame.timestamps.is_some();
        (frame, measurement.profile)
    }

    /// Whether submitted frame timings are awaiting readback.
    pub fn profile_pending(&self) -> bool {
        self.timestamps.pending()
    }

    /// Advances timestamp readbacks without waiting for the GPU. This can submit separate
    /// query resolve commands. Returns whether a sample remains pending after this poll.
    pub fn poll_profile(&mut self) -> bool {
        self.timestamps.poll(&self.device, &self.queue)
    }

    fn record_frame(
        &mut self,
        mut encoder: wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        input: FrameInput<'_>,
        mut recording: FrameRecording<'_>,
    ) -> RecordedFrame<'_> {
        let preparing = recording.measurement.is_some().then(Instant::now);
        let attachment = CopyAttachment::from_texture(view.texture(), self.format);
        self.canvas_cache.prune();
        self.backdrop_history.prune();
        let frame = self.prepare_frame(input, attachment, &mut recording);
        let prepared = preparing.map(|start| {
            let end = Instant::now();
            (end, end.duration_since(start))
        });
        if !recording.before.is_empty() {
            recording.before.insert(0, encoder.finish());
            encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("octara drawing") });
        }
        encoder = self.encode_frame(encoder, &frame, view, input.scale, &mut recording);
        if let Some((encoding, prepare)) = prepared
            && let Some(measurement) = recording.measurement
        {
            measurement.profile.encode = encoding.elapsed() + measurement.canvas_encode;
            measurement.profile.prepare = prepare.saturating_sub(measurement.canvas_encode);
            measure_draws(&frame, &mut measurement.profile);
        }
        RecordedFrame {
            before: recording.before,
            encoder,
            device: &self.device,
            queue: &self.queue,
            canvas_cache: &mut self.canvas_cache,
            backdrop_history: &self.backdrop_history,
            changes: recording.outputs.unwrap_or_default(),
            revision: input.sample.map(|sample| sample.revision),
            receipts: recording.receipts,
            timestamp_pool: &mut self.timestamps,
            timestamps: None,
        }
    }

    fn prepare_canvases(
        &mut self,
        passes: &mut [Pass],
        requests: Vec<CanvasRequest>,
        scale: f32,
        recording: &mut FrameRecording<'_>,
    ) -> Vec<wgpu::TextureView> {
        self.programs.prune_canvases();
        if requests.is_empty() {
            self.programs.finish_canvases(0);
            return Vec::new();
        }
        let mut slots = Vec::with_capacity(requests.len());
        let mut views = Vec::with_capacity(requests.len());
        let mut transient_views = 0;
        recording.before.reserve(requests.len() + 1);
        // Preparation follows display-list order, including canvases inside child layers.
        for request in requests {
            let index = views.len();
            let retained = request.retained_mount.map(|mount| {
                let slot = mount.as_ptr().addr();
                (slot, mount, CanvasKey::new(&request.paint, scale))
            });
            if let Some((slot, _, key)) = &retained
                && let Some(image) = self.canvas_cache.get(*slot, key)
            {
                views.push(image.view.clone());
                slots.push(Some(index));
                recording.receipts[request.paint.receipt].outcome = PaintOutcome::Retained { revision: image.revision };
                if let Some(changes) = &mut recording.outputs {
                    changes.canvases.reuse(*slot);
                }
                if let Some(measurement) = recording.measurement.as_deref_mut() {
                    measurement.profile.retained_canvases += 1;
                }
                continue;
            }
            let target = match &retained {
                Some((slot, _, _)) => {
                    CanvasTarget::Working(self.canvas_cache.working(*slot, recording.outputs.as_ref().map(|changes| &changes.canvases)))
                }
                None => CanvasTarget::Transient(transient_views),
            };
            let product = self.programs.encode_canvas(&self.device, &request.paint, scale, target, recording.measurement.as_deref_mut());
            let outcome = match product {
                Ok((view, command)) => {
                    if let Some((_, mount, key)) = retained {
                        if let Some(changes) = &mut recording.outputs {
                            changes.canvases.record(mount, view.clone(), key);
                        }
                    } else {
                        transient_views += 1;
                    }
                    views.push(view);
                    recording.before.push(command);
                    slots.push(Some(index));
                    PaintOutcome::Rendered
                }
                Err(reason) => {
                    passes[request.pass].insts[request.instance] = request.paint.fallback;
                    slots.push(None);
                    PaintOutcome::Fallback { reason }
                }
            };
            recording.receipts[request.paint.receipt].outcome = outcome;
        }
        self.programs.finish_canvases(transient_views);
        for pass in passes {
            pass.special.retain_mut(|batch| {
                let Draw::Canvas(request) = batch.draw else {
                    return true;
                };
                let Some(view) = slots[request] else {
                    return false;
                };
                batch.draw = Draw::Canvas(view);
                true
            });
        }
        views
    }

    fn prepare_frame(&mut self, input: FrameInput<'_>, output: CopyAttachment, recording: &mut FrameRecording<'_>) -> PreparedFrame {
        let (mut passes, requests, clips) = self.passes(input);
        let PaintRequests { canvases, backdrops, receipts } = requests;
        recording.receipts = receipts;
        let canvas_views = self.prepare_canvases(&mut passes, canvases, input.scale, recording);
        let window = [input.size.0, input.size.1];
        let bounds = bound_passes(&mut passes, window);
        // One buffer holds every pass's instances.
        let mut all: Vec<Inst> = Vec::new();
        let mut instance_offsets = Vec::with_capacity(passes.len());
        for p in &passes {
            instance_offsets.push(all.len() as u32);
            all.extend_from_slice(&p.insts);
        }
        let backdrop_fallback_offset = all.len() as u32;
        all.extend(backdrops.iter().map(|request| request.paint.fallback));
        let plan = FramePlan::new(&passes, &bounds, &instance_offsets, backdrops, backdrop_fallback_offset, output);
        let bytes: &[u8] = bytemuck::cast_slice(&all);
        let need = (bytes.len() as u64).max(std::mem::size_of::<Inst>() as u64);
        if self.instances.as_ref().is_none_or(|(_, cap)| *cap < need) {
            let cap = need.next_power_of_two();
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("octara instances"),
                size: cap,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instances = Some((buffer, cap));
        }
        if !bytes.is_empty() {
            let (buffer, _) = self.instances.as_ref().expect("instance buffer");
            self.queue.write_buffer(buffer, 0, bytes);
        }
        let layer_count = passes.len() - 1;
        self.layers.truncate(layer_count);
        for (index, bounds) in bounds[1..].iter().enumerate() {
            let [width, height] = bounds.allocation(window);
            if self.layers.get(index).is_some_and(|t| t.width() == width && t.height() == height) {
                continue;
            }
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("octara layer"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            if let Some(slot) = self.layers.get_mut(index) {
                *slot = texture;
            } else {
                self.layers.push(texture);
            }
        }
        // Distinct projections in this submission, one aligned uniform range per target.
        let uniform_size = std::mem::size_of::<Globals>();
        let bytes_len = layer_count * self.globals_stride + uniform_size;
        if self.globals.size() < bytes_len as u64 {
            self.globals = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("octara globals"),
                size: bytes_len.next_power_of_two() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        let mut uniforms = vec![0; bytes_len];
        let sizes = std::iter::once(window).chain(self.layers.iter().map(|t| [t.width(), t.height()]));
        for (index, (bounds, extent)) in bounds.iter().zip(sizes).enumerate() {
            let globals = Globals { size: extent.map(|n| n as f32), origin: bounds.origin.map(|n| n as f32) };
            let at = index * self.globals_stride;
            uniforms[at..at + uniform_size].copy_from_slice(bytemuck::bytes_of(&globals));
        }
        self.queue.write_buffer(&self.globals, 0, &uniforms);
        let nodes = bytemuck::cast_slice(clips.nodes());
        let need = nodes.len() as u64;
        if need > self.clip_nodes.size() {
            let capacity = need.next_power_of_two().min(self.device.limits().max_storage_buffer_binding_size);
            self.clip_nodes = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("octara clips"),
                size: capacity,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !nodes.is_empty() {
            self.queue.write_buffer(&self.clip_nodes, 0, nodes);
        }
        let layer_views: Vec<wgpu::TextureView> = self.layers.iter().map(|t| t.create_view(&Default::default())).collect();
        let atlases = [&self.mask, &self.color, &self.images].map(|texture| texture.create_view(&Default::default()));
        let no_layer = self.no_layer.create_view(&Default::default());
        let bind_with = |view| self.bind_texture(&atlases, view);
        let bind = bind_with(&no_layer);
        let layer_binds: Vec<wgpu::BindGroup> = layer_views.iter().map(bind_with).collect();
        let canvas_binds = canvas_views.iter().map(bind_with).collect();
        let [r, g, b, a] = input.list.background.components;
        let background = wgpu::Color { r: (r * a) as f64, g: (g * a) as f64, b: (b * a) as f64, a: a as f64 };
        PreparedFrame { plan, background, layer_views, atlases, bind, layer_binds, canvas_binds }
    }

    fn bind_texture(&self, atlases: &[wgpu::TextureView; 3], layer: &wgpu::TextureView) -> wgpu::BindGroup {
        let [mask, color, images] = atlases;
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("octara"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.globals,
                        offset: 0,
                        size: wgpu::BufferSize::new(std::mem::size_of::<Globals>() as u64),
                    }),
                },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(mask) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(color) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(images) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(layer) },
                wgpu::BindGroupEntry { binding: 5, resource: self.clip_nodes.as_entire_binding() },
            ],
        })
    }

    fn encode_backdrop(
        &mut self,
        backdrop: &PlannedBackdrop,
        target: &wgpu::TextureView,
        scale: f32,
        recording: &mut FrameRecording<'_>,
    ) -> Result<EncodedBackdrop, PaintFallback> {
        let request = &backdrop.request;
        let region = match backdrop.input {
            BackdropInput::Prefix(region) => region,
            BackdropInput::Unavailable => return Err(PaintFallback::BackdropUnavailable),
            BackdropInput::Accepted => {
                let image = self.backdrop_history.get(&request.mount).ok_or(PaintFallback::HistoryUnavailable)?;
                if let Some(p) = recording.measurement.as_deref_mut() {
                    p.profile.retained_backdrops += 1;
                }
                return Ok(EncodedBackdrop {
                    view: image.view.clone(),
                    prefix_commands: None,
                    outcome: PaintOutcome::Retained { revision: image.revision },
                });
            }
        };
        let copy = custom::BackdropCopy::new(target.texture(), self.format, region);
        let working = recording.outputs.as_ref().and_then(|_| self.backdrop_history.take_working(&request.mount));
        let (view, command) = self.programs.encode_backdrop(
            &self.device,
            &request.paint,
            scale,
            working.as_ref().map(wgpu::TextureView::texture),
            copy,
            recording.measurement.as_deref_mut(),
        )?;
        if let Some(changes) = &mut recording.outputs {
            changes.backdrops.push(BackdropChange { mount: request.mount.clone(), view: view.clone() });
        }
        Ok(EncodedBackdrop { view, prefix_commands: Some(command), outcome: PaintOutcome::Rendered })
    }

    fn encode_frame(
        &mut self,
        mut encoder: wgpu::CommandEncoder,
        frame: &PreparedFrame,
        view: &wgpu::TextureView,
        scale: f32,
        recording: &mut FrameRecording<'_>,
    ) -> wgpu::CommandEncoder {
        let (buffer, _) = self.instances.as_ref().expect("instance buffer");
        let buffer = buffer.clone();
        for target_plan in &frame.plan.targets {
            let (target, clear, kind, index) = match target_plan.attachment {
                Attachment::Output => (view, frame.background, GpuPassKind::Ui, 0),
                Attachment::Layer(index) => (&frame.layer_views[index - 1], wgpu::Color::TRANSPARENT, GpuPassKind::Layer, index),
            };
            let mut pass = color_pass(&mut encoder, target, wgpu::LoadOp::Clear(clear), recording.pass(kind));
            if target_plan.batches.is_empty() {
                continue;
            }
            let projection = [u32::try_from(index * self.globals_stride).expect("pass uniform offset")];
            pass.set_vertex_buffer(0, buffer.slice(..));
            for batch in &frame.plan.batches[target_plan.batches.clone()] {
                let instances = batch.instances.clone();
                let (pipeline, bind) = match batch.draw {
                    Draw::Standard => (&self.pipeline, &frame.bind),
                    Draw::Cutout => (&self.cutout, &frame.bind),
                    Draw::Layer(layer) => (&self.pipeline, &frame.layer_binds[layer - 1]),
                    Draw::Canvas(view) => (&self.pipeline, &frame.canvas_binds[view]),
                    Draw::Material(material) => (self.programs.material_pipeline(material), &frame.bind),
                    Draw::Backdrop(index) => {
                        let backdrop = &frame.plan.backdrops[index];
                        let outcome = match self.encode_backdrop(backdrop, target, scale, recording) {
                            Ok(EncodedBackdrop { view, prefix_commands, outcome }) => {
                                if let Some(command) = prefix_commands {
                                    // The isolated copy and author commands run after the prefix is stored.
                                    drop(pass);
                                    recording.before.extend([encoder.finish(), command]);
                                    encoder = self
                                        .device
                                        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("octara drawing") });
                                    pass = color_pass(&mut encoder, target, wgpu::LoadOp::Load, recording.pass(kind));
                                    pass.set_vertex_buffer(0, buffer.slice(..));
                                }
                                let bind = self.bind_texture(&frame.atlases, &view);
                                draw_batch(&mut pass, &self.pipeline, &bind, &projection, instances);
                                outcome
                            }
                            Err(reason) => {
                                let fallback = backdrop.fallback;
                                draw_batch(&mut pass, &self.pipeline, &frame.bind, &projection, fallback..fallback + 1);
                                PaintOutcome::Fallback { reason }
                            }
                        };
                        recording.receipts[backdrop.request.paint.receipt].outcome = outcome;
                        continue;
                    }
                };
                draw_batch(&mut pass, pipeline, bind, &projection, instances);
            }
        }
        encoder
    }

    /// Renders offscreen and reads the pixels back (tests, screenshots, a window's captures), in
    /// straight-alpha sRGB RGBA8, regardless of the renderer's texture format. Each axis has a
    /// one-pixel minimum.
    ///
    /// # Panics
    /// If the dimensions or scale are invalid, the capture format is unsupported, or GPU readback
    /// fails. [`Painter::paint`] returns an error in these cases.
    pub fn render(&mut self, dl: &DisplayList, scale: f32) -> Picture {
        self.try_render(dl, scale, None).expect("GPU rendering failed").picture
    }

    fn try_render(&mut self, dl: &DisplayList, scale: f32, sample: Option<PaintSample>) -> Result<PaintedPicture, String> {
        let mut readback = self.start_render_with_receipts(dl, scale, sample)?;
        self.device
            .poll(wgpu::PollType::Wait { submission_index: Some(readback.submission.clone()), timeout: None })
            .map_err(|e| format!("capture GPU poll failed: {e}"))?;
        readback.try_finish().unwrap_or_else(|| Err("capture readback callback did not report completion".into()))
    }

    /// Submits an offscreen frame and starts mapping its pixel buffer. The readback owns its
    /// resources so this renderer can submit more frames while the GPU finishes it.
    pub fn start_render_with_receipts(&mut self, list: &DisplayList, scale: f32, sample: Option<PaintSample>) -> Result<Readback, String> {
        let bgra = match self.format {
            wgpu::TextureFormat::Rgba8Unorm => false,
            wgpu::TextureFormat::Bgra8Unorm => true,
            format => return Err(format!("capture requires RGBA8 or BGRA8 (renderer format is {format:?})")),
        };
        let (width, height) = crate::pixels::dimensions(list.size, scale)?;
        let limits = self.device.limits();
        if width > limits.max_texture_dimension_2d || height > limits.max_texture_dimension_2d {
            return Err(format!("capture {width}×{height} exceeds the GPU texture dimension limit {}", limits.max_texture_dimension_2d));
        }
        let layout = readback_layout(width, height).ok_or("capture readback exceeds addressable memory or row limits")?;
        if layout.bytes > limits.max_buffer_size {
            return Err(format!("capture readback exceeds the GPU buffer limit of {} bytes", limits.max_buffer_size));
        }
        // A device lost during capture may leave an invalid buffer for the readback map.
        let validation = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("octara offscreen"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("octara readback"),
            size: layout.bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let view = target.create_view(&Default::default());
        let encoder = self.device.create_command_encoder(&Default::default());
        let input = FrameInput { list, scale, size: (width, height), sample };
        let mut frame = self.record_frame(encoder, &view, input, FrameRecording::default());
        frame.encoder().copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(layout.row), rows_per_image: Some(height) },
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        let (mapped, mapping) = mpsc::channel();
        frame.encoder().map_buffer_on_submit(&buffer, wgpu::MapMode::Read, .., move |result| {
            let _ = mapped.send(result);
        });
        let effects = std::mem::take(&mut frame.receipts);
        let submission = frame.submit();
        if let Some(error) = pollster::block_on(validation.pop()) {
            return Err(format!("capture GPU validation failed: {error}"));
        }
        Ok(Readback { submission, pending: Some(PendingReadback { buffer, mapping, layout, width, height, bgra, effects }) })
    }
}

fn custom_quad(paint: &CustomPaint, fallback: &Inst) -> Inst {
    Inst {
        rect: fallback.rect,
        fill: [0.0, 0.0, 0.0, paint.opacity],
        clip: fallback.clip,
        params: InstParams { filled: 0, ..fallback.params },
        uv: paint.params,
        ..Inst::zeroed()
    }
}

fn image(i: &ImagePaint, (ax, ay, w, h): (u32, u32, u32, u32), scale: f32, clip: [f32; 4], clip_head: u32) -> Inst {
    let (r, b) = (i.rect, i.bounds);
    Inst {
        rect: [r.x * scale, r.y * scale, r.w * scale, r.h * scale],
        fill: [0.0, 0.0, 0.0, i.opacity],
        border_color: [0.0; 4],
        border: [b.x * scale, b.y * scale, b.w * scale, b.h * scale],
        clip,
        params: InstParams { radius: i.radius * scale, kind: KIND_IMAGE, clip_head, filled: 0 },
        uv: [ax as f32, ay as f32, w as f32, h as f32],
        grad_color: [0.0; 4],
        grad: [0.0; 4],
    }
}

fn shadow(s: &Shadow, scale: f32, clip: [f32; 4], clip_head: u32) -> Inst {
    let r = s.rect;
    Inst {
        rect: [r.x * scale, r.y * scale, r.w * scale, r.h * scale],
        fill: straight(s.color),
        border_color: [0.0; 4],
        border: [s.blur * 0.5 * scale, 0.0, 0.0, 0.0],
        clip,
        params: InstParams { radius: s.radius * scale, kind: KIND_SHADOW, clip_head, filled: 0 },
        uv: [0.0; 4],
        grad_color: [0.0; 4],
        grad: [0.0; 4],
    }
}

fn quad(q: &Quad, scale: f32, clip: [f32; 4], clip_head: u32) -> Inst {
    let r = q.rect;
    let (fill, grad_color, grad) = match (q.gradient, q.fill) {
        (Some(g), _) => (straight(g.from.0), straight(g.to.0), [g.angle, g.from.1, g.to.1, 1.0]),
        (None, f) => (f.map_or([0.0; 4], straight), [0.0; 4], [0.0; 4]),
    };
    let filled = q.fill.is_some() || q.gradient.is_some();
    Inst {
        rect: [r.x * scale, r.y * scale, r.w * scale, r.h * scale],
        fill,
        border_color: straight(q.border_color),
        border: q.border.map(|b| b * scale),
        clip,
        params: InstParams { radius: q.radius * scale, kind: KIND_QUAD, clip_head, filled: u32::from(filled) },
        uv: [0.0; 4],
        grad_color,
        grad,
    }
}

impl Painter for GpuRenderer {
    fn renderer(&self) -> &'static str {
        "gpu"
    }
    fn paint(&mut self, list: &DisplayList, scale: f32) -> Result<Picture, String> {
        self.try_render(list, scale, None).map(|painted| painted.picture)
    }

    fn paint_with_receipts(&mut self, list: &DisplayList, scale: f32, sample: Option<PaintSample>) -> Result<PaintedPicture, String> {
        self.try_render(list, scale, sample)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use octara_primitives::identity::NodeId;
    use octara_primitives::paint::{PaintMount, Part};
    use octara_primitives::theme::rgb;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn solid(rect: Rect, color: Color) -> Quad {
        Quad {
            source: NodeId::default(),
            rect,
            radius: 0.0,
            fill: Some(color),
            gradient: None,
            border: [0.0; 4],
            border_color: Color::TRANSPARENT,
        }
    }

    fn picture(items: impl IntoIterator<Item = Item>) -> DisplayList {
        DisplayList { size: (32.0, 24.0), background: Color::TRANSPARENT, parts: items.into_iter().map(Part::Item).collect() }
    }

    fn close(actual: [u8; 4], expected: [u8; 4]) {
        assert!(actual.into_iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 1), "pixel {actual:?}, expected {expected:?}");
    }

    fn device_target(device: &wgpu::Device, usage: wgpu::TextureUsages) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: 32, height: 24, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: usage | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
    }

    fn copy_sample(frame: &mut CanvasFrame<'_>, sample: &Backdrop<'_>, from: [u32; 2], to: [u32; 2], size: [u32; 2]) {
        let origin = |[x, y]: [u32; 2]| wgpu::Origin3d { x, y, z: 0 };
        frame.encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo { origin: origin(from), ..sample.view.texture().as_image_copy() },
            wgpu::TexelCopyTextureInfo { origin: origin(to), ..frame.target.texture().as_image_copy() },
            wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        );
    }

    fn program_box(rect: Rect, program: PaintProgram, params: [f32; 4]) -> CustomPaint {
        CustomPaint { fallback: solid(rect, rgb(0xff00ff)), program, params, opacity: 1.0, mount: PaintMount::default(), exiting: false }
    }

    fn profile_frame<'a>(
        gpu: &'a mut GpuRenderer,
        target: &wgpu::TextureView,
        list: &DisplayList,
        scale: f32,
        revision: u64,
    ) -> (RecordedFrame<'a>, FrameProfile) {
        let encoder = gpu.device().create_command_encoder(&Default::default());
        let size = (target.texture().width(), target.texture().height());
        let sample = Some(PaintSample { revision, sampled_at: Duration::from_millis(revision) });
        let (frame, profile) = gpu.record_sampled(encoder, target, FrameInput { list, scale, size, sample }, Some(Box::new(|_| {})));
        (frame, profile.expect("a profiled frame"))
    }

    fn retained_color(draws: Arc<AtomicUsize>) -> PaintProgram {
        retained_canvas(move |device| {
            let draws = draws.clone();
            let copy_source = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("retained canvas validation probe"),
                size: 4,
                usage: wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            move |frame: &mut CanvasFrame<'_>| {
                draws.fetch_add(1, Ordering::Relaxed);
                assert_eq!((frame.rect.x, frame.rect.y, frame.sampled_at), (0.0, 0.0, None));
                let [r, g, b, a] = frame.params.map(f64::from);
                if r < 0.0 {
                    frame.encoder.clear_buffer(&copy_source, 0, None);
                } else {
                    drop(color_pass(frame.encoder, frame.target, wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a }), None));
                }
            }
        })
    }

    #[test]
    fn retained_canvas_reuses_pixels_through_scroll_clip_and_group_opacity() {
        let Some(mut gpu) = GpuRenderer::new() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let draws = Arc::new(AtomicUsize::new(0));
        let mut paint = program_box(Rect::new(2.0, 3.0, 6.25, 7.25), retained_color(draws.clone()), [1.0, 0.0, 0.0, 1.0]);
        paint.fallback.radius = 0.5;
        let target = device_target(gpu.device(), wgpu::TextureUsages::empty()).create_view(&Default::default());
        let first = picture([Item::Custom(paint.clone())]);
        let (recorded, profile) = profile_frame(&mut gpu, &target, &first, 1.0, 10);
        assert_eq!(recorded.paint_receipts()[0].outcome, PaintOutcome::Rendered);
        assert_eq!((profile.canvas_pixels, profile.retained_canvases, profile.render_passes), (56, 0, 1));
        recorded.submit();

        let source = NodeId::default();
        let mut moved = picture([
            Item::PushClip { source, rect: Rect::new(13.0, 8.0, 6.0, 9.0), radius: 0.0 },
            Item::PushLayer { source, opacity: 0.5 },
        ]);
        moved.parts.push(Part::Drawing { items: vec![Item::Custom(paint.clone())].into(), offset: (10.0, 5.0), fade: 1.0 });
        moved.parts.extend([Part::Item(Item::PopLayer), Part::Item(Item::PopClip)]);
        let hit = PaintOutcome::Retained { revision: Some(10) };
        let (recorded, profile) = profile_frame(&mut gpu, &target, &moved, 1.0, 20);
        assert_eq!(recorded.paint_receipts()[0].outcome, hit);
        assert_eq!((profile.canvas_pixels, profile.retained_canvases, profile.render_passes), (0, 1, 2));
        recorded.submit();
        let capture = gpu.paint_with_receipts(&moved, 1.0, None).unwrap();
        assert_eq!(capture.effects[0].outcome, hit);
        assert_eq!(draws.load(Ordering::Relaxed), 1);
        close(capture.picture.pixel(15, 11), [255, 0, 0, 128]);
        assert_eq!(capture.picture.pixel(12, 11), [0; 4]);
        assert_eq!(capture.picture.pixel(4, 5), [0; 4]);

        let mut variants = vec![];
        paint.fallback.rect.w = 6.5; // The logical box changed while its pixel extent stayed 7×8.
        variants.push((paint.clone(), 1.0));
        paint.fallback.radius = 1.5;
        variants.push((paint.clone(), 1.0));
        variants.push((paint.clone(), 1.05)); // This scale also preserves the pixel extent.
        paint.params = [0.0, 1.0, 0.0, 1.0];
        variants.push((paint.clone(), 1.05));
        paint.mount = PaintMount::default();
        variants.push((paint.clone(), 1.05));
        paint.program = retained_color(draws.clone());
        variants.push((paint, 1.05));
        for (index, (paint, scale)) in variants.iter().enumerate() {
            let list = picture([Item::Custom(paint.clone())]);
            let (recorded, profile) = profile_frame(&mut gpu, &target, &list, *scale, 30 + index as u64);
            assert_eq!(recorded.paint_receipts()[0].outcome, PaintOutcome::Rendered, "changed input {index}");
            assert_eq!((profile.canvas_pixels, profile.retained_canvases), (56, 0));
            recorded.submit();
        }
        assert_eq!(draws.load(Ordering::Relaxed), 1 + variants.len());
    }

    #[test]
    fn retained_canvas_keeps_submitted_pixels_and_independent_outputs_for_duplicate_mounts() {
        let Some(mut gpu) = GpuRenderer::new() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let draws = Arc::new(AtomicUsize::new(0));
        let paint = program_box(Rect::new(2.0, 2.0, 8.0, 8.0), retained_color(draws.clone()), [1.0, 0.0, 0.0, 1.0]);
        let draw = |x: f32, w: f32, params| {
            let mut paint = CustomPaint { params, ..paint.clone() };
            paint.fallback.rect.x = x;
            paint.fallback.rect.w = w;
            Item::Custom(paint)
        };
        let red = [1.0, 0.0, 0.0, 1.0];
        let green = [0.0, 1.0, 0.0, 1.0];
        let blue = [0.0, 0.0, 1.0, 1.0];
        let submitted = picture([draw(2.0, 8.0, green)]);
        let device = gpu.device().clone();
        let target = device_target(&device, wgpu::TextureUsages::COPY_SRC);
        let view = target.create_view(&Default::default());
        for (revision, frame) in [(10, picture([draw(2.0, 8.0, red)])), (20, submitted.clone())] {
            profile_frame(&mut gpu, &view, &frame, 1.0, revision).0.submit();
        }
        let unsubmitted = picture([draw(2.0, 8.0, blue)]);
        let (recorded, _) = profile_frame(&mut gpu, &view, &unsubmitted, 1.0, 30);
        assert_eq!(recorded.paint_receipts()[0].outcome, PaintOutcome::Rendered);
        drop(recorded);
        let resized = picture([draw(2.0, 12.0, blue)]);
        let capture = gpu.paint_with_receipts(&resized, 1.0, None).unwrap();
        assert_eq!(capture.effects[0].outcome, PaintOutcome::Rendered);
        close(capture.picture.pixel(11, 4), [0, 0, 255, 255]);
        let (recorded, _) = profile_frame(&mut gpu, &view, &resized, 1.0, 40);
        assert_eq!(recorded.paint_receipts()[0].outcome, PaintOutcome::Rendered);
        drop(recorded);

        let failed = picture([draw(2.0, 8.0, [-1.0, 0.0, 0.0, 1.0])]);
        let (recorded, profile) = profile_frame(&mut gpu, &view, &failed, 1.0, 50);
        assert_eq!(recorded.paint_receipts()[0].outcome, PaintOutcome::Fallback { reason: PaintFallback::EncodingFailed });
        assert_eq!((profile.canvas_pixels, profile.retained_canvases), (0, 0));
        recorded.submit();
        let capture = gpu.paint_with_receipts(&submitted, 1.0, None).unwrap();
        assert_eq!(capture.effects[0].outcome, PaintOutcome::Retained { revision: Some(20) });
        close(capture.picture.pixel(4, 4), [0, 255, 0, 255]);
        assert_eq!(draws.load(Ordering::Relaxed), 6);

        // Both author draws precede the target pass. A shared working allocation would render
        // the last color at both positions when the renderer samples them.
        let duplicate = picture([draw(2.0, 8.0, blue), draw(16.0, 8.0, red)]);
        let (mut recorded, profile) = profile_frame(&mut gpu, &view, &duplicate, 1.0, 60);
        assert!(recorded.paint_receipts().iter().all(|receipt| receipt.outcome == PaintOutcome::Rendered));
        assert_eq!((profile.canvas_pixels, profile.retained_canvases), (128, 0));
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 8,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        for (x, offset) in [(4, 0), (18, 4)] {
            recorded.encoder().copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo { origin: wgpu::Origin3d { x, y: 4, z: 0 }, ..target.as_image_copy() },
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout { offset, bytes_per_row: None, rows_per_image: None },
                },
                wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            );
        }
        let submission = recorded.submit();
        readback.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
        device.poll(wgpu::PollType::Wait { submission_index: Some(submission), timeout: None }).unwrap();
        assert_eq!(&*readback.get_mapped_range(..).unwrap(), &[0, 0, 255, 255, 255, 0, 0, 255]);

        drop(submitted);
        drop(unsubmitted);
        drop(resized);
        drop(failed);
        drop(duplicate);
        drop(paint);
        gpu.render(&picture([]), 1.0);
        assert!(gpu.canvas_cache.is_empty());
    }

    #[test]
    fn frame_plan_orders_targets_and_resolves_backdrop_inputs() {
        let mut output = Pass::default();
        output.insts.push(Inst::zeroed());
        output.push_special(Inst::zeroed(), Draw::Layer(1));
        output.push_special(Inst::zeroed(), Draw::Backdrop(0));
        output.insts.push(Inst::zeroed());
        output.push_special(Inst::zeroed(), Draw::Backdrop(2));
        let mut layer = Pass::default();
        layer.insts.push(Inst::zeroed());
        layer.push_special(Inst::zeroed(), Draw::Backdrop(1));
        let bounds = [PassBounds { origin: [0, 0], size: [32, 24] }, PassBounds { origin: [5, 6], size: [5, 4] }];
        let requests = || {
            Vec::from([(0, false), (1, false), (0, true)].map(|(target, exiting)| BackdropRequest {
                paint: CanvasPaint {
                    program: PaintProgram::new(()),
                    fallback: Inst { rect: [2.25, 3.25, 14.0, 10.0], clip: [4.5, 5.25, 20.0, 15.0], ..Inst::zeroed() },
                    fallback_source: NodeId::default(),
                    size: [14, 10],
                    params: [0.0; 4],
                    rect: Rect::new(2.25, 3.25, 14.0, 10.0),
                    radius: 0.0,
                    sampled_at: None,
                    receipt: 0,
                },
                mount: Weak::new(),
                exiting,
                target,
            }))
        };
        let passes = [output, layer];
        let attachment = CopyAttachment { extent: [12, 10], copyable: true };
        let plan = FramePlan::new(&passes, &bounds, &[0, 5], requests(), 7, attachment);
        assert_eq!(plan.targets.iter().map(|target| target.attachment).collect::<Vec<_>>(), [Attachment::Layer(1), Attachment::Output]);
        assert_eq!(plan.targets[0].batches, 0..2);
        assert_eq!(plan.targets[1].batches, 2..7);
        assert_eq!(
            plan.batches,
            [
                PlannedBatch { draw: Draw::Standard, instances: 5..6 },
                PlannedBatch { draw: Draw::Backdrop(1), instances: 6..7 },
                PlannedBatch { draw: Draw::Standard, instances: 0..1 },
                PlannedBatch { draw: Draw::Layer(1), instances: 1..2 },
                PlannedBatch { draw: Draw::Backdrop(0), instances: 2..3 },
                PlannedBatch { draw: Draw::Standard, instances: 3..4 },
                PlannedBatch { draw: Draw::Backdrop(2), instances: 4..5 },
            ],
        );
        assert_eq!(plan.backdrops[0].input, BackdropInput::Prefix(BackdropRegion { origin: [4, 5], size: [8, 5], offset: [2, 2] }));
        assert_eq!(plan.backdrops[1].input, BackdropInput::Prefix(BackdropRegion { origin: [0, 0], size: [5, 4], offset: [3, 3] }));
        assert_eq!(plan.backdrops[2].input, BackdropInput::Accepted);
        assert_eq!(plan.backdrops[2].fallback, 9);
        let no_copy = FramePlan::new(&passes, &bounds, &[0, 5], requests(), 7, CopyAttachment { copyable: false, ..attachment });
        assert_eq!(no_copy.backdrops[0].input, BackdropInput::Unavailable);
        assert_eq!(no_copy.backdrops[1].input, plan.backdrops[1].input);
        assert_eq!(no_copy.backdrops[2].input, BackdropInput::Accepted);

        let plain = Pass { insts: vec![Inst::zeroed(); 4], ..Default::default() };
        let direct = FramePlan::new(&[plain], &bounds[..1], &[0], Vec::new(), 4, attachment);
        assert_eq!(direct.targets.len(), 1);
        assert_eq!(direct.targets[0].attachment, Attachment::Output);
        assert_eq!(direct.batches, [PlannedBatch { draw: Draw::Standard, instances: 0..4 }]);
    }

    #[test]
    fn nested_rounded_clips_keep_both_corners_in_layered_draws() {
        let source = NodeId::default();
        let rect = Rect::new(0.0, 0.0, 16.0, 16.0);
        let dl = |draw| {
            picture([
                Item::PushClip { source, rect, radius: 8.0 },
                Item::PushLayer { source, opacity: 0.5 },
                Item::PushClip { source, rect: Rect::new(1.0, 1.0, 12.0, 12.0), radius: 1.0 },
                draw,
                Item::PopClip,
                Item::PopLayer,
                Item::PopClip,
            ])
        };
        let assert_corners = |image: Picture| {
            // The outer circle removes this pixel even though the inner rounded rectangle covers it.
            assert_eq!(image.pixel(1, 1), [0; 4]);
            // The inner right edge removes this pixel even though the outer circle covers it.
            assert_eq!(image.pixel(13, 8), [0; 4]);
            // At (2.5, 2.5), the outer distance is sqrt(5.5² + 5.5²) − 8.
            close(image.pixel(2, 2), [255, 0, 0, 92]);
            close(image.pixel(8, 8), [255, 0, 0, 128]);
        };
        let red = Item::Quad(solid(rect, rgb(0xff0000)));
        assert_corners(crate::CpuRenderer::new().render(&dl(red.clone()), 1.0));
        let Some(mut gpu) = GpuRenderer::new() else {
            eprintln!("no GPU adapter: skipped GPU corner check");
            return;
        };
        assert_corners(gpu.render(&dl(red), 1.0));
        let program = material("fn material(uv: vec2f, params: vec4f) -> vec4f { return params; }");
        assert_corners(gpu.render(&dl(Item::Custom(program_box(rect, program, [1.0, 0.0, 0.0, 1.0]))), 1.0));
    }

    #[test]
    fn author_programs_and_profiles_preserve_renderer_send_and_sync() {
        fn assert_send_and_sync<T: Send + Sync>() {}
        assert_send_and_sync::<GpuRenderer>();
        assert_send_and_sync::<RecordedFrame<'static>>();
        assert_send_and_sync::<PendingPresentation>();
    }

    #[test]
    fn materials_batch_in_painter_order_with_per_instance_params() {
        let Some(mut gpu) = GpuRenderer::new() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        const SHADER: &str = "fn material(uv: vec2f, params: vec4f) -> vec4f { return params; }";
        let custom = |rect: Rect, program, params| Item::Custom(program_box(rect, program, params));
        let dl = picture([
            Item::PushClip { source: NodeId::default(), rect: Rect::new(2.0, 2.0, 17.0, 13.0), radius: 5.0 },
            custom(Rect::new(2.0, 2.0, 13.0, 12.0), material(SHADER), [1.0, 0.0, 0.0, 1.0]),
            custom(Rect::new(8.0, 2.0, 9.0, 13.0), material(SHADER), [0.0, 1.0, 0.0, 0.5]),
            Item::PopClip,
            Item::Quad(solid(Rect::new(10.0, 6.0, 4.0, 8.0), rgb(0x0000ff))),
            custom(Rect::new(20.0, 2.0, 8.0, 12.0), material(SHADER), [0.0, 0.5, 1.0, 1.0]),
            custom(Rect::new(20.0, 15.0, 8.0, 6.0), material("invalid WGSL"), [1.0; 4]),
            custom(Rect::new(2.0, 16.0, 8.0, 6.0), PaintProgram::new(()), [1.0; 4]),
        ]);
        let (passes, requests, _) = gpu.passes(FrameInput { list: &dl, scale: 1.0, size: (32, 24), sample: None });
        assert!(requests.canvases.is_empty() && requests.backdrops.is_empty());
        let material = *material(SHADER).downcast_ref::<custom::Material>().unwrap();
        assert_eq!(
            passes[0].special,
            [Batch { instances: 0..2, draw: Draw::Material(material) }, Batch { instances: 3..4, draw: Draw::Material(material) }]
        );
        let frame = gpu.render(&dl, 1.0);
        close(frame.pixel(2, 2), [0, 0, 0, 0]); // Rounded ancestor clip.
        close(frame.pixel(6, 8), [255, 0, 0, 255]);
        close(frame.pixel(8, 8), [127, 128, 0, 255]); // Blend in instance order.
        close(frame.pixel(12, 8), [0, 0, 255, 255]); // Ordinary quad covers both materials.
        close(frame.pixel(16, 8), [0, 255, 0, 128]); // Straight-alpha output.
        close(frame.pixel(22, 8), [0, 128, 255, 255]);
        close(frame.pixel(22, 18), [255, 0, 255, 255]); // Failed shader.
        close(frame.pixel(5, 18), [255, 0, 255, 255]); // Unrecognized back end.
    }

    #[test]
    fn canvas_uses_clip_group_opacity_frame_params_and_validation_fallback() {
        let Some(mut gpu) = GpuRenderer::new() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let preparations = Arc::new(AtomicUsize::new(0));
        let program = canvas({
            let preparations = preparations.clone();
            move |device| {
                preparations.fetch_add(1, Ordering::Relaxed);
                let copy_source = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("test canvas copy source"),
                    size: 4,
                    usage: wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                });
                move |frame: &mut CanvasFrame<'_>| {
                    assert_eq!(frame.size, [(21.0 * frame.scale) as u32; 2]);
                    if frame.params[3] < 0.0 {
                        frame.encoder.clear_buffer(&copy_source, 0, None);
                        return;
                    }
                    if frame.params[3] > 0.0 {
                        let [r, g, b, a] = frame.params.map(f64::from);
                        drop(frame.begin_render_pass(
                            "test_canvas_fill",
                            &wgpu::RenderPassDescriptor {
                                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                    view: frame.target,
                                    depth_slice: None,
                                    resolve_target: None,
                                    ops: wgpu::Operations {
                                        load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a }),
                                        store: wgpu::StoreOp::Store,
                                    },
                                })],
                                ..Default::default()
                            },
                        ));
                    }
                }
            }
        });
        let draw = |params: [f32; 4]| {
            let mut dl = picture([
                Item::PushClip { source: NodeId::default(), rect: Rect::new(3.0, 3.0, 16.0, 16.0), radius: 6.0 },
                Item::PushLayer { source: NodeId::default(), opacity: 0.5 },
                Item::Custom(CustomPaint {
                    fallback: solid(Rect::new(1.0, 1.0, 21.0, 21.0), rgb(0x00ff00)),
                    program: program.clone(),
                    params,
                    opacity: 1.0,
                    mount: PaintMount::default(),
                    exiting: false,
                }),
                Item::Quad(solid(Rect::new(10.0, 8.0, 4.0, 4.0), Color::BLACK)),
                Item::PopLayer,
                Item::PopClip,
                Item::Quad(solid(Rect::new(14.0, 7.0, 4.0, 6.0), rgb(0x0000ff))),
            ]);
            dl.background = Color::WHITE;
            dl
        };
        for scale in [1, 2] {
            let frame = gpu.render(&draw([0.5, 0.0, 0.0, 0.5]), scale as f32);
            let pixel = |x: u32, y: u32| frame.pixel(x * scale, y * scale);
            close(pixel(3, 3), [255; 4]); // Rounded clip corner, inside the canvas's own box.
            close(pixel(8, 10), [255, 191, 191, 255]); // Multiplication in premultiplied space.
            close(pixel(11, 10), [128, 128, 128, 255]); // The group's black quad fully covers the canvas.
            close(pixel(15, 10), [0, 0, 255, 255]); // Quad after the layer.
            let empty = gpu.render(&draw([0.0; 4]), scale as f32); // Reuses a cleared allocation.
            close(empty.pixel(8 * scale, 10 * scale), [255; 4]);
        }
        let failed = gpu.render(&draw([0.0, 0.0, 0.0, -1.0]), 2.0);
        close(failed.pixel(16, 20), [127, 255, 127, 255]);
        close(failed.pixel(30, 20), [0, 0, 255, 255]);
        assert_eq!(preparations.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn backdrops_read_the_painter_prefix_and_keep_surrounding_commands_in_order() {
        let Some(mut gpu) = GpuRenderer::new() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let blue = canvas(|_| {
            |frame: &mut CanvasFrame<'_>| {
                drop(color_pass(frame.encoder, frame.target, wgpu::LoadOp::Clear(wgpu::Color::BLUE), None));
            }
        });
        let transfer = backdrop(|_| {
            |frame: &mut CanvasFrame<'_>, sample: Backdrop<'_>| {
                assert_eq!((sample.size, sample.offset), ([14, 12], [0, 0]));
                let [x, y, tx, ty] = frame.params.map(|value| value as u32);
                copy_sample(frame, &sample, [x, y], [tx, ty], [1, 1]);
            }
        });
        let rect = Rect::new(0.0, 0.0, 14.0, 12.0);
        let dl = picture([
            Item::Custom(program_box(Rect::new(2.0, 2.0, 4.0, 4.0), blue, [0.0; 4])),
            Item::Custom(program_box(rect, transfer.clone(), [3.0, 3.0, 8.0, 7.0])),
            Item::Custom(program_box(rect, transfer, [8.0, 7.0, 10.0, 9.0])),
            Item::Quad(solid(Rect::new(2.0, 2.0, 4.0, 4.0), rgb(0xff0000))),
        ]);
        let device = gpu.device().clone();
        let texture = device_target(&device, wgpu::TextureUsages::COPY_SRC);
        let view = texture.create_view(&Default::default());
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 16,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let copy_pixel = |encoder: &mut wgpu::CommandEncoder, x, y, offset| {
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo { origin: wgpu::Origin3d { x, y, z: 0 }, ..texture.as_image_copy() },
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout { offset, bytes_per_row: None, rows_per_image: None },
                },
                wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            );
        };
        let mut encoder = device.create_command_encoder(&Default::default());
        copy_pixel(&mut encoder, 3, 3, 0);
        let mut frame = gpu.record(encoder, &dl, 1.0, &view, (32, 24));
        copy_pixel(frame.encoder(), 3, 3, 4);
        copy_pixel(frame.encoder(), 8, 7, 8);
        copy_pixel(frame.encoder(), 10, 9, 12);
        let submission = frame.submit();
        readback.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
        device.poll(wgpu::PollType::Wait { submission_index: Some(submission), timeout: None }).unwrap();
        let data = readback.get_mapped_range(..).unwrap();
        let expected = [[0; 4], [255, 0, 0, 255], [0, 0, 255, 255], [0, 0, 255, 255]];
        assert_eq!(&*data, expected.as_flattened());
    }

    #[test]
    fn backdrop_crops_to_nested_clips_and_the_resized_attachment() {
        let Some(mut gpu) = GpuRenderer::new() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let transfer = backdrop(|_| {
            |frame: &mut CanvasFrame<'_>, sample: Backdrop<'_>| {
                let scale = frame.scale as u32;
                let size = |pixels: [u32; 2]| pixels.map(|axis| axis * scale);
                assert_eq!(frame.size, size([21, 19]));
                assert_eq!(sample.size, size([frame.params[0] as u32, 13]));
                assert_eq!(sample.offset, size([6, 4]).map(|axis| axis as i32));
                copy_sample(frame, &sample, size([2, 4]), size([14, 9]), size([3, 3]));
                copy_sample(frame, &sample, size([2, 4]), size([6, 4]), size([3, 3]));
            }
        });
        for (width, scale, crop_width) in [(160.0, 1_u32, 15.0), (88.0, 2, 11.0)] {
            let mut dl = picture([
                Item::PushClip { source: NodeId::default(), rect: Rect::new(75.0, 11.0, 18.0, 20.0), radius: 0.0 },
                Item::PushLayer { source: NodeId::default(), opacity: 0.5 },
                Item::Quad(solid(Rect::new(74.0, 9.0, 21.0, 24.0), rgb(0x00ff00))),
                Item::PushClip { source: NodeId::default(), rect: Rect::new(77.0, 10.0, 15.0, 14.0), radius: 4.0 },
                Item::PushLayer { source: NodeId::default(), opacity: 0.5 },
                Item::Quad(solid(Rect::new(79.0, 15.0, 4.0, 4.0), rgb(0xff0000))),
                Item::Custom(program_box(Rect::new(71.25, 7.25, 21.0, 19.0), transfer.clone(), [crop_width, 0.0, 0.0, 0.0])),
                Item::Quad(solid(Rect::new(79.0, 15.0, 4.0, 4.0), rgb(0x0000ff))),
                Item::PopLayer,
                Item::PopClip,
                Item::PopLayer,
                Item::PopClip,
            ]);
            dl.size = (width, 64.0);
            dl.background = Color::WHITE;
            let frame = gpu.render(&dl, scale as f32);
            let pixel = |x, y| frame.pixel(x * scale, y * scale);
            close(pixel(74, 15), [255; 4]);
            close(pixel(76, 11), [127, 255, 127, 255]);
            close(pixel(80, 16), [127, 191, 191, 255]);
            close(pixel(86, 17), [191, 191, 127, 255]);
        }
    }

    #[test]
    fn backdrop_exit_replays_only_the_accepted_image_of_its_mount() {
        let Some(mut gpu) = GpuRenderer::new() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let draws = Arc::new(AtomicUsize::new(0));
        let transfer = backdrop({
            let draws = draws.clone();
            move |device| {
                let invalid_clear = device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: 4,
                    usage: wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                });
                let draws = draws.clone();
                move |frame: &mut CanvasFrame<'_>, sample: Backdrop<'_>| {
                    draws.fetch_add(1, Ordering::Relaxed);
                    if frame.params[0] < 0.0 {
                        frame.encoder.clear_buffer(&invalid_clear, 0, None);
                    } else {
                        copy_sample(frame, &sample, [0, 0], [0, 0], sample.size);
                    }
                }
            }
        });
        let mount = PaintMount::default();
        let replacement = PaintMount::default();
        let effect = |mount: PaintMount, x, exiting, params| {
            Item::Custom(CustomPaint { mount, exiting, ..program_box(Rect::new(x, 2.0, 8.0, 8.0), transfer.clone(), params) })
        };
        let live = |background, params| {
            let mut dl = picture([effect(mount.clone(), 2.0, false, params)]);
            dl.background = rgb(background);
            dl
        };
        let device = gpu.device().clone();
        let target = device_target(&device, wgpu::TextureUsages::COPY_SRC);
        let view = target.create_view(&Default::default());
        let encoder = || device.create_command_encoder(&Default::default());
        fn sampled(list: &DisplayList, revision: u64) -> FrameInput<'_> {
            FrameInput {
                list,
                scale: 1.0,
                size: (32, 24),
                sample: Some(PaintSample { revision, sampled_at: Duration::from_millis(revision) }),
            }
        }
        for (background, revision) in [(0xffff00, 10), (0xff0000, 20)] {
            let (frame, _) = gpu.record_sampled(encoder(), &view, sampled(&live(background, [0.0; 4]), revision), None);
            assert_eq!(frame.paint_receipts()[0].outcome, PaintOutcome::Rendered);
            let (_, pending) = frame.submit_for_presentation();
            gpu.accept_presentation(pending);
        }
        // The deferred frame can use the previous allocation. Subsequent frames must leave
        // those pixels available until the host accepts it.
        let (frame, _) = gpu.record_sampled(encoder(), &view, sampled(&live(0x00ff00, [0.0; 4]), 30), None);
        assert_eq!(frame.paint_receipts()[0].outcome, PaintOutcome::Rendered);
        let (_, pending) = frame.submit_for_presentation();
        let (frame, _) = gpu.record_sampled(encoder(), &view, sampled(&live(0x0000ff, [0.0; 4]), 31), None);
        assert_eq!(frame.paint_receipts()[0].outcome, PaintOutcome::Rendered);
        frame.submit();
        drop(gpu.record_sampled(encoder(), &view, sampled(&live(0xffff00, [0.0; 4]), 32), None));
        let capture = gpu.render(&live(0x0000ff, [0.0; 4]), 1.0);
        close(capture.pixel(5, 5), [0, 0, 255, 255]);
        let unsupported = device_target(&device, wgpu::TextureUsages::empty()).create_view(&Default::default());
        let (frame, _) = gpu.record_sampled(encoder(), &unsupported, sampled(&live(0x000000, [0.0; 4]), 40), None);
        assert_eq!(frame.paint_receipts()[0].outcome, PaintOutcome::Fallback { reason: PaintFallback::BackdropUnavailable });
        let (_, unavailable) = frame.submit_for_presentation();
        gpu.accept_presentation(unavailable);
        let mut failing = live(0x00ff00, [-1.0; 4]);
        failing.parts.push(Part::Item(Item::Quad(solid(Rect::new(8.0, 3.0, 1.0, 2.0), rgb(0x0000ff)))));
        let failure = gpu.render(&failing, 1.0);
        close(failure.pixel(5, 5), [255, 0, 255, 255]);
        close(failure.pixel(8, 3), [0, 0, 255, 255]);
        let (frame, _) = gpu.record_sampled(encoder(), &view, sampled(&failing, 50), None);
        assert_eq!(frame.paint_receipts()[0].outcome, PaintOutcome::Fallback { reason: PaintFallback::EncodingFailed });
        let (_, failed) = frame.submit_for_presentation();
        gpu.accept_presentation(failed);
        let offscreen = gpu.render(&picture([effect(mount.clone(), 34.0, false, [0.0; 4])]), 1.0);
        close(offscreen.pixel(31, 5), [0; 4]);

        let exiting = picture([
            Item::PushLayer { source: NodeId::default(), opacity: 0.5 },
            effect(mount.clone(), 2.0, true, [0.0; 4]),
            effect(replacement, 13.0, true, [0.0; 4]),
            Item::PopLayer,
        ]);
        let exit =
            gpu.paint_with_receipts(&exiting, 1.0, Some(PaintSample { revision: 60, sampled_at: Duration::from_millis(60) })).unwrap();
        close(exit.picture.pixel(5, 5), [255, 0, 0, 128]);
        close(exit.picture.pixel(16, 5), [255, 0, 255, 128]);
        assert_eq!(exit.effects[0].outcome, PaintOutcome::Retained { revision: Some(20) });
        assert_eq!(exit.effects[1].outcome, PaintOutcome::Fallback { reason: PaintFallback::HistoryUnavailable });
        gpu.accept_presentation(pending);
        let exit = gpu.paint_with_receipts(&exiting, 1.0, None).unwrap();
        close(exit.picture.pixel(5, 5), [0, 255, 0, 128]);
        assert_eq!(exit.effects[0].outcome, PaintOutcome::Retained { revision: Some(30) });
        assert_eq!(draws.load(Ordering::Relaxed), 8);
        drop(exiting);
        drop(failing);
        drop(mount);
        gpu.render(&picture([]), 1.0);
        assert!(gpu.backdrop_history.is_empty());
    }

    #[test]
    fn profiled_frames_measure_recorded_work_and_read_back_asynchronously() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())) else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let features = adapter.features() & (wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS);
        let Ok((device, queue)) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { required_features: features, ..Default::default() }))
        else {
            eprintln!("no GPU device: skipped");
            return;
        };
        let mut gpu = GpuRenderer::with_device(device.clone(), queue, FORMAT);
        let timestamps_supported = features.contains(wgpu::Features::TIMESTAMP_QUERY);
        if timestamps_supported {
            let mut held: Vec<_> = (0..3).map(|_| gpu.timestamps.begin(&device, &gpu.queue).unwrap()).collect();
            assert!(gpu.timestamps.begin(&device, &gpu.queue).is_none());
            drop(held.pop());
            assert!(gpu.timestamps.begin(&device, &gpu.queue).is_some());
            drop(held);
        }
        let view = device_target(&device, wgpu::TextureUsages::COPY_SRC).create_view(&Default::default());
        let size = (32, 24);
        let quad = |x, y| Item::Quad(solid(Rect::new(x, y, 4.0, 4.0), Color::WHITE));
        let material = material("fn material(uv: vec2f, params: vec4f) -> vec4f { return params; }");
        let render = |frame: &mut CanvasFrame<'_>, name| {
            drop(frame.begin_render_pass(
                name,
                &wgpu::RenderPassDescriptor {
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: frame.target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                    })],
                    ..Default::default()
                },
            ));
        };
        let canvas_program = canvas(move |device| {
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: None,
                source: wgpu::ShaderSource::Wgsl("@compute @workgroup_size(1) fn main() {}".into()),
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: None,
                layout: None,
                module: &shader,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            });
            move |frame: &mut CanvasFrame<'_>| {
                {
                    let mut pass = frame.begin_compute_pass("test_compute", &Default::default());
                    pass.set_pipeline(&pipeline);
                    pass.dispatch_workgroups(1, 1, 1);
                }
                render(frame, "test_render");
            }
        });
        let failing_canvas = canvas(move |device| {
            let source = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 4,
                usage: wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            move |frame: &mut CanvasFrame<'_>| {
                render(frame, "test_discarded");
                frame.encoder.clear_buffer(&source, 0, None);
            }
        });
        let backdrop =
            backdrop(|_| |frame: &mut CanvasFrame<'_>, sample: Backdrop<'_>| copy_sample(frame, &sample, [0, 0], [0, 0], sample.size));
        let material_box = |x| Item::Custom(program_box(Rect::new(x, 6.0, 4.0, 4.0), material.clone(), [1.0; 4]));
        let effect = program_box(Rect::new(3.0, 4.0, 8.0, 8.0), backdrop, [0.0; 4]);
        let dl = picture([
            quad(0.0, 0.0),
            Item::PushLayer { source: NodeId::default(), opacity: 0.5 },
            quad(10.0, 0.0),
            Item::PopLayer,
            material_box(0.0),
            material_box(5.0),
            Item::Custom(program_box(Rect::new(2.0, 2.0, 4.0, 4.0), canvas_program, [0.0; 4])),
            Item::Custom(effect.clone()),
            quad(0.0, 0.0),
            Item::Custom(program_box(Rect::new(2.0, 2.0, 4.0, 4.0), failing_canvas, [0.0; 4])),
        ]);
        let encoder = || device.create_command_encoder(&Default::default());
        let (discard, _) = gpu.record_profiled(encoder(), &dl, 1.0, &view, size, |_| panic!("discarded frame reported a GPU sample"));
        drop(discard);
        assert!(!gpu.profile_pending());

        let (send, receive) = std::sync::mpsc::channel();
        let (frame, profile) = gpu.record_profiled(encoder(), &dl, 1.0, &view, size, move |sample| send.send(sample).unwrap());
        assert_eq!((profile.draw_batches, profile.material_batches, profile.render_passes), (7, 1, 3));
        assert_eq!((profile.layer_pixels, profile.canvas_pixels, profile.backdrop_copy_pixels), (768, 80, 64));
        assert_eq!(profile.retained_backdrops, 0);
        assert_eq!(profile.gpu_sampled, timestamps_supported);
        let (_, pending) = frame.submit_for_presentation();
        gpu.accept_presentation(pending);
        let deadline = Instant::now() + Duration::from_secs(5);
        while gpu.poll_profile() {
            assert!(Instant::now() < deadline, "timestamp mapping did not complete");
            std::thread::sleep(Duration::from_millis(1));
        }
        if profile.gpu_sampled {
            let sample = receive.try_recv().expect("submitted frame reported GPU timings");
            eprintln!("{:?} GPU timings: {sample:?}", adapter.get_info().backend);
            let kinds: Vec<_> = sample.passes.iter().map(|pass| pass.kind).collect();
            assert_eq!(kinds.iter().filter(|&&kind| kind == GpuPassKind::Ui).count(), 2);
            assert!(kinds.contains(&GpuPassKind::Layer));
            if features.contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS) {
                assert_eq!(
                    kinds,
                    [
                        GpuPassKind::Canvas,
                        GpuPassKind::Author("test_compute"),
                        GpuPassKind::Author("test_render"),
                        GpuPassKind::Layer,
                        GpuPassKind::Ui,
                        GpuPassKind::BackdropCopy,
                        GpuPassKind::Backdrop,
                        GpuPassKind::Ui
                    ]
                );
                assert!(sample.span.is_some());
            } else {
                assert_eq!(
                    kinds,
                    [
                        GpuPassKind::CanvasClear,
                        GpuPassKind::Author("test_compute"),
                        GpuPassKind::Author("test_render"),
                        GpuPassKind::Layer,
                        GpuPassKind::Ui,
                        GpuPassKind::BackdropClear,
                        GpuPassKind::Ui
                    ]
                );
                assert!(sample.span.is_none());
            }
            assert!(!sample.truncated);
        } else {
            assert!(receive.try_recv().is_err());
        }

        let exit = picture([Item::Custom(CustomPaint { exiting: true, ..effect })]);
        let (frame, profile) = gpu.record_profiled(encoder(), &exit, 1.0, &view, size, |_| {});
        assert_eq!((profile.retained_backdrops, profile.backdrop_copy_pixels, profile.canvas_pixels), (1, 0, 0));
        drop(frame);
        assert!(!gpu.profile_pending());
    }

    #[test]
    fn nested_layers_bound_their_painted_pixels_instead_of_the_window() {
        let window = [1600, 1200];
        let composite = Inst { params: InstParams { kind: KIND_LAYER, ..InstParams::default() }, clip: NO_CLIP, ..Inst::zeroed() };
        let quad = Inst { rect: [370.5, 460.25, 28.0, 18.0], clip: NO_CLIP, ..Inst::zeroed() };
        let shadow = Inst {
            rect: [400.25, 480.5, 21.0, 12.0],
            clip: [404.0, 480.0, 10.0, 10.0],
            border: [2.0, 0.0, 0.0, 0.0],
            params: InstParams { kind: KIND_SHADOW, ..InstParams::default() },
            ..Inst::zeroed()
        };
        let mut passes = [
            Pass { insts: vec![composite], special: vec![Batch { instances: 0..1, draw: Draw::Layer(1) }], ..Pass::default() },
            Pass { insts: vec![quad, composite], special: vec![Batch { instances: 1..2, draw: Draw::Layer(2) }], ..Pass::default() },
            Pass { insts: vec![shadow], ..Pass::default() },
        ];
        let bounds = bound_passes(&mut passes, window);
        assert_eq!(bounds[0].size, window);
        assert_eq!(bounds[1].rect(), [369.0, 459.0, 46.0, 32.0]);
        assert_eq!(bounds[2].rect(), [403.0, 479.0, 12.0, 12.0]);
        assert_eq!(passes[0].insts[0].rect, bounds[1].rect());
        assert_eq!(passes[1].insts[1].rect, bounds[2].rect());
        assert_eq!(bounds[1].allocation(window), [64, 64]);
        assert_eq!(bounds[2].allocation(window), [64, 64]);
    }

    #[test]
    fn readback_layout_aligns_rows_and_checks_allocation_without_allocating() {
        assert!(readback_layout(0, 1).is_none());
        assert!(readback_layout(1, 0).is_none());
        let layout = readback_layout(65, 3).unwrap();
        assert_eq!((layout.row, layout.bytes, layout.rgba_len), (512, 1536, 65 * 3 * 4));
        assert!(readback_layout(u32::MAX / 4, 1).is_none(), "row padding must fit bytes_per_row");
        assert!(readback_layout(u32::MAX, u32::MAX).is_none());
        if usize::BITS == 64 {
            let layout = readback_layout(65536, 65536).unwrap();
            assert_eq!(layout.row, 262144);
            assert_eq!(layout.bytes, 1_u64 << 34);
            assert_eq!(layout.rgba_len, usize::try_from(1_u64 << 34).unwrap());
        } else {
            assert!(readback_layout(65536, 65536).is_none());
        }
    }

    #[test]
    fn offscreen_readbacks_keep_their_own_pixels_and_receipts() {
        let Some(mut gpu) = GpuRenderer::new() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let paint = program_box(Rect::new(0.0, 0.0, 4.0, 4.0), PaintProgram::new(()), [0.0; 4]);
        let source = paint.fallback.source;
        let red = DisplayList { background: rgb(0xff0000), ..picture([Item::Custom(paint)]) };
        let blue = DisplayList { background: rgb(0x0000ff), ..picture([]) };
        let mut first = gpu.start_render_with_receipts(&red, 1.0, None).unwrap();
        let mut second = gpu.start_render_with_receipts(&blue, 2.0, None).unwrap();

        gpu.device().poll(wgpu::PollType::Wait { submission_index: Some(second.submission.clone()), timeout: None }).unwrap();
        let blue = second.try_finish().expect("second buffer mapped").unwrap();
        assert_eq!((blue.picture.width, blue.picture.height), (64, 48));
        assert_eq!(blue.picture.pixel(63, 47), [0, 0, 255, 255]);
        assert!(blue.effects.is_empty());
        let red = first.try_finish().expect("first buffer mapped").unwrap();
        assert_eq!((red.picture.width, red.picture.height), (32, 24));
        assert_eq!(red.picture.pixel(31, 23), [255, 0, 0, 255]);
        assert_eq!(red.effects, [PaintReceipt { source, outcome: PaintOutcome::Fallback { reason: PaintFallback::UnsupportedProgram } }]);
        assert!(first.try_finish().is_none());
        assert!(second.try_finish().is_none());
    }

    #[test]
    fn atlas_refuses_oversized_images_without_wrapping_or_moving_the_shelf() {
        let mut shelf = Shelf::new(ATLAS);
        assert_eq!(shelf.alloc(1, 1), Some((0, 0)));
        assert_eq!(shelf.alloc(u32::MAX, 1), None);
        assert_eq!(shelf.alloc(1, u32::MAX), None);
        assert_eq!(shelf.alloc(1, 1), Some((2, 0)));
    }
}

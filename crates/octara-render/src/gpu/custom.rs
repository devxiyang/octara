use super::GpuPassKind;
use super::WORKING_FORMAT;
use super::pipeline::{Phase, SHARED_SHADER, render_pipeline};
use super::profile::{FrameMeasurement, Instant, TimestampFrame};
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use crate::plan::paint::canvas_size;
use crate::plan::{BackdropRegion, CanvasMode, CanvasPaint, CanvasProgram, Material};
use octara_model::capture::PaintFallback;
use octara_model::geometry::Rect;
use octara_model::identity::NodeId;
use octara_model::paint::{PaintInput, PaintProgram};
use rustc_hash::FxHashMap;
use std::any::Any;
use std::collections::VecDeque;
use std::future::Future;
use std::sync::Weak;
use std::time::Duration;

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub mod local;

/// A fragment program for an element's rounded box. Define
/// `fn material(uv: vec2f, params: vec4f) -> vec4f` in WGSL. The top-left of the box is
/// `(0, 0)`, and the bottom-right is `(1, 1)`. Return straight-alpha linear-light RGBA with sRGB primaries.
///
/// Octara supplies antialiasing, clipping and compositing. Instances of the same shader
/// can be batched with different parameter values. An invalid shader uses the box's
/// ordinary paint.
pub fn material(wgsl: &'static str) -> PaintProgram {
    PaintProgram::new(Material::new(wgsl))
}

/// A canvas draw records render, compute or copy commands into a local GPU target. The
/// renderer prepares the callback once for a shared program on its device. Each visible
/// draw receives the frame's size, scale and parameters.
///
/// The renderer clears the target to transparent before invoking the callback. Write
/// premultiplied linear-light RGBA with sRGB primaries to it. Preparation and encoding validation errors use the
/// box's ordinary paint. The renderer submits the recorded commands with the rest of the frame,
/// so the resources they use must stay unmapped and undestroyed through submission.
/// A draw can be omitted or replayed, including for a capture. Record drawing and scratch work
/// here. Publish application data as a new [`PaintInput`] version before it can affect a draw.
pub fn canvas<C>(prepare: impl Fn(&wgpu::Device) -> C + Send + Sync + 'static) -> PaintProgram
where
    C: for<'a> Fn(&mut CanvasFrame<'a>) + wgpu::WasmNotSendSync + 'static,
{
    local_canvas(prepare, CanvasMode::Volatile)
}

/// A canvas whose pixels can be reused after a compatible output is submitted.
///
/// Draw deterministic pixels for the frozen input, local box size and resolved radius,
/// pixel extent, scale, and device. The target is `Rgba16Float`, the box origin is `(0, 0)`,
/// and `sampled_at` is `None`. Publish a new input when author data changes. Use [`canvas`] when drawing
/// depends on window coordinates, frame time, or undeclared state.
///
/// The renderer may prepare the local pixels before a window target is available, and reuse a
/// mount's last compatible submitted pixels. Position, clipping and opacity are applied during
/// composition. Cache misses, eviction and device replacement can invoke the callback again.
/// Drawing and validation otherwise follow [`canvas`].
pub fn retained_canvas<C>(prepare: impl Fn(&wgpu::Device) -> C + Send + Sync + 'static) -> PaintProgram
where
    C: for<'a> Fn(&mut CanvasFrame<'a>) + wgpu::WasmNotSendSync + 'static,
{
    local_canvas(prepare, CanvasMode::Retained)
}

fn local_canvas<C>(prepare: impl Fn(&wgpu::Device) -> C + Send + Sync + 'static, mode: CanvasMode) -> PaintProgram
where
    C: for<'a> Fn(&mut CanvasFrame<'a>) + wgpu::WasmNotSendSync + 'static,
{
    let prepare: Box<CanvasPrepare> = Box::new(move |device| {
        let draw = prepare(device);
        Box::new(move |frame: &mut CanvasFrame<'_>, _: CanvasInput<'_>| draw(frame))
    });
    PaintProgram::new(CanvasProgram::new(mode, prepare))
}

/// A canvas that can sample already-drawn pixels in its current compositing group. The
/// renderer copies the canvas pixel grid, cropped to the clip bounds and target, at this
/// item's position in the display list. An isolated opacity group starts with transparent pixels.
/// Preparation, output, clipping and validation use the same conventions as [`canvas`].
/// When a window attachment cannot be copied, the scene is drawn to a readable intermediate
/// attachment and resolved to the window after composition.
/// An exiting mount replays its last output accepted by the host or uses the ordinary paint
/// if none exists.
pub fn backdrop<C>(prepare: impl Fn(&wgpu::Device) -> C + Send + Sync + 'static) -> PaintProgram
where
    C: for<'a> Fn(&mut CanvasFrame<'a>, Backdrop<'a>) + wgpu::WasmNotSendSync + 'static,
{
    let prepare: Box<CanvasPrepare> = Box::new(move |device| {
        let draw = prepare(device);
        Box::new(move |frame: &mut CanvasFrame<'_>, input: CanvasInput<'_>| {
            let CanvasInput::Backdrop(sample) = input else { unreachable!("backdrop input") };
            draw(frame, sample);
        })
    });
    PaintProgram::new(CanvasProgram::new(CanvasMode::Backdrop, prepare))
}

/// Composes the host-accepted old image of a local region with its current image.
/// Preparation, encoding and the transparent target follow [`canvas`]. The target is
/// a separate `Rgba16Float` texture over the output bounds in `frame.rect`. Write
/// premultiplied linear-light RGBA to it. The renderer shows the current region when the
/// old image is unavailable, the transition has settled, or the callback fails validation.
pub fn transition<C>(prepare: impl Fn(&wgpu::Device) -> C + Send + Sync + 'static) -> PaintProgram
where
    C: for<'a> Fn(&mut CanvasFrame<'a>, Transition<'a>) + wgpu::WasmNotSendSync + 'static,
{
    let prepare: Box<CanvasPrepare> = Box::new(move |device| {
        let draw = prepare(device);
        Box::new(move |frame: &mut CanvasFrame<'_>, input: CanvasInput<'_>| {
            let CanvasInput::Transition(images) = input else { unreachable!("transition inputs") };
            draw(frame, images);
        })
    });
    PaintProgram::new(CanvasProgram::new(CanvasMode::Transition, prepare))
}

/// The old and current region images and sampled progress of a [`transition`] draw.
#[derive(Clone, Copy)]
pub struct Transition<'a> {
    /// The accepted region image fixed when this transition started.
    pub old: TransitionImage<'a>,
    /// The current region image produced for this draw.
    pub new: TransitionImage<'a>,
    /// Progress sampled on the application's clock, in `[0, 1)` while running.
    pub progress: f32,
}

/// A region image scoped to one [`transition`] draw. Coordinates outside its bounds
/// represent transparent pixels. Each image has its own bounds and scale.
#[derive(Clone, Copy)]
pub struct TransitionImage<'a> {
    /// Sampled `Rgba16Float` view of premultiplied linear-light pixels.
    /// Use the view as a read-only binding within this draw.
    pub view: &'a wgpu::TextureView,
    /// The top-left corner in window device pixels at this image's `scale`.
    pub origin: [u32; 2],
    /// Width and height in device pixels. Use this extent to bound samples. An empty
    /// current region has a zero extent and a transparent backing view.
    pub size: [u32; 2],
    /// Device pixels per logical window pixel when this image was drawn.
    pub scale: f32,
}

/// The pixels available to a [`backdrop`] callback in this draw.
pub struct Backdrop<'a> {
    /// Sampling or loading this view returns premultiplied linear-light RGBA. The view may use
    /// sRGB storage with hardware decoding. Use it as a read-only binding within the callback.
    pub view: &'a wgpu::TextureView,
    /// The width and height of the sampled region in device pixels.
    pub size: [u32; 2],
    /// The sampled region's top-left, relative to the box's top-left rounded down to a device pixel.
    pub offset: [i32; 2],
}

/// The resources and sampled parameters available to an author canvas for one draw.
pub struct CanvasFrame<'a> {
    pub device: &'a wgpu::Device,
    pub encoder: &'a mut wgpu::CommandEncoder,
    /// Single-sampled `Rgba16Float`, with render attachment, storage, sampling and copy
    /// usages. The renderer owns this view and its texture. Use them within the callback.
    pub target: &'a wgpu::TextureView,
    /// Width and height in pixels, rounded up from the laid-out size at `scale`.
    pub size: [u32; 2],
    pub scale: f32,
    /// The resolved box in logical window coordinates, or local coordinates for a retained canvas.
    pub rect: Rect,
    /// The box's resolved corner radii in logical pixels.
    pub radius: [f32; 4],
    /// The App clock sampled by a published frame, when available. Retained canvases receive `None`.
    pub sampled_at: Option<Duration>,
    /// The drawing's parameters and optional immutable author data, fixed when it was published.
    pub input: &'a PaintInput,
    timestamps: Option<&'a mut TimestampFrame>,
}

impl CanvasFrame<'_> {
    /// Format for render and storage bindings that write the canvas target.
    pub const FORMAT: wgpu::TextureFormat = WORKING_FORMAT;

    /// Begins a render pass and requests a timestamp pair under `name` when GPU profiling is
    /// active. Use a stable name to identify this pass in reports. The pass still records when
    /// timing is unavailable, and timestamp writes provided in the descriptor take precedence.
    pub fn begin_render_pass(&mut self, name: &'static str, desc: &wgpu::RenderPassDescriptor<'_>) -> wgpu::RenderPass<'_> {
        if let Some(timestamps) = self.timestamps.as_deref_mut()
            && desc.timestamp_writes.is_none()
            && let Some(writes) = timestamps.pass(GpuPassKind::Author(name))
        {
            return self.encoder.begin_render_pass(&wgpu::RenderPassDescriptor { timestamp_writes: Some(writes), ..desc.clone() });
        }
        self.encoder.begin_render_pass(desc)
    }

    /// Begins a compute pass and requests a timestamp pair under `name` using the same rules as
    /// [`Self::begin_render_pass`].
    pub fn begin_compute_pass(&mut self, name: &'static str, desc: &wgpu::ComputePassDescriptor<'_>) -> wgpu::ComputePass<'_> {
        if let Some(timestamps) = self.timestamps.as_deref_mut()
            && desc.timestamp_writes.is_none()
            && let Some(writes) = timestamps.compute_pass(GpuPassKind::Author(name))
        {
            return self.encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { timestamp_writes: Some(writes), ..desc.clone() });
        }
        self.encoder.begin_compute_pass(desc)
    }
}

enum CanvasInput<'a> {
    Local,
    Backdrop(Backdrop<'a>),
    Transition(Transition<'a>),
}

trait CanvasDraw: for<'a> Fn(&mut CanvasFrame<'a>, CanvasInput<'a>) + wgpu::WasmNotSendSync {}
impl<T> CanvasDraw for T where T: for<'a> Fn(&mut CanvasFrame<'a>, CanvasInput<'a>) + wgpu::WasmNotSendSync {}

type CanvasPrepare = dyn Fn(&wgpu::Device) -> Box<dyn CanvasDraw> + Send + Sync;

pub(super) fn canvas_mode(program: &PaintProgram) -> Option<CanvasMode> {
    let canvas = program.downcast_ref::<CanvasProgram>()?;
    canvas.payload::<Box<CanvasPrepare>>().map(|_| canvas.mode)
}

struct PreparedCanvas {
    /// Keeps the key's allocation distinct and tells us when the app has released the program.
    owner: Weak<dyn Any + Send + Sync>,
    draw: Option<Box<dyn CanvasDraw>>,
    reported_failure: bool,
}

pub(super) struct BackdropCopy<'a> {
    source: &'a wgpu::Texture,
    format: wgpu::TextureFormat,
    origin: wgpu::Origin3d,
    size: [u32; 2],
    offset: [i32; 2],
}

impl<'a> BackdropCopy<'a> {
    pub(super) fn new(source: &'a wgpu::Texture, region: BackdropRegion) -> Self {
        Self {
            source,
            format: source.format(),
            origin: wgpu::Origin3d { x: region.origin[0], y: region.origin[1], z: 0 },
            size: region.size,
            offset: region.offset,
        }
    }

    fn encode(&self, encoder: &mut wgpu::CommandEncoder, destination: &wgpu::Texture) {
        let [width, height] = self.size;
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo { origin: self.origin, ..self.source.as_image_copy() },
            destination.as_image_copy(),
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
    }
}

/// Backdrop inputs are copied and consumed in one ordered command buffer. The borrowed input
/// cannot be used by later commands, so these exact-size descriptors can share a texture.
#[derive(Default)]
struct BackdropInputs {
    textures: VecDeque<wgpu::TextureView>,
    bytes: u64,
}

impl BackdropInputs {
    const CACHE_BYTES: u64 = 32 * 1024 * 1024;
    const CACHE_ENTRIES: usize = 8;

    fn view(&mut self, device: &wgpu::Device, copy: &BackdropCopy<'_>) -> (wgpu::TextureView, bool) {
        let [width, height] = copy.size;
        if let Some(index) = self.textures.iter().position(|view| {
            let texture = view.texture();
            texture.width() == width && texture.height() == height && texture.format() == copy.format
        }) {
            let view = self.textures.remove(index).expect("the cached backdrop index");
            self.textures.push_back(view.clone());
            return (view, true);
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("octara backdrop sample"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: copy.format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        (texture.create_view(&Default::default()), false)
    }

    fn bytes(view: &wgpu::TextureView) -> Option<u64> {
        let texture = view.texture();
        let format = texture.format();
        let (bw, bh) = format.block_dimensions();
        let bpp = format.block_copy_size(Some(wgpu::TextureAspect::All))?;
        Some(u64::from(texture.width().div_ceil(bw)) * u64::from(texture.height().div_ceil(bh)) * u64::from(bpp))
    }

    fn retain_view(&mut self, view: wgpu::TextureView) {
        let Some(bytes) = Self::bytes(&view).filter(|&bytes| bytes <= Self::CACHE_BYTES) else { return };
        while self.textures.len() == Self::CACHE_ENTRIES || self.bytes + bytes > Self::CACHE_BYTES {
            let old = self.textures.pop_front().expect("oldest cached backdrop");
            self.bytes -= Self::bytes(&old).expect("cached backdrop byte size");
        }
        self.bytes += bytes;
        self.textures.push_back(view);
    }
}

#[derive(Default)]
pub(super) struct Programs {
    materials: FxHashMap<(Material, wgpu::TextureFormat), Option<wgpu::RenderPipeline>>,
    canvases: FxHashMap<usize, PreparedCanvas>,
    /// Reuse an exact-size allocation at each frame slot. Never reuse its previous pixels.
    targets: Vec<wgpu::Texture>,
    backdrop_inputs: BackdropInputs,
}

pub(super) enum CanvasTarget<'a> {
    Transient(usize),
    Working(Option<&'a wgpu::Texture>),
}

enum CanvasSource<'a> {
    Local,
    Backdrop(BackdropCopy<'a>, &'a mut BackdropInputs),
    Transition(Transition<'a>),
}

impl Programs {
    pub(super) async fn prepare_material(
        &mut self,
        device: &wgpu::Device,
        layout: &wgpu::PipelineLayout,
        format: wgpu::TextureFormat,
        material: Material,
        node: NodeId,
    ) -> bool {
        use std::collections::hash_map::Entry;
        let pipeline = match self.materials.entry((material, format)) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let pipeline = checked_async(device, || {
                    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("octara material"),
                        source: wgpu::ShaderSource::Wgsl(
                            format!("{}\n{SHARED_SHADER}{}", material.wgsl(), include_str!("material.wgsl")).into(),
                        ),
                    });
                    render_pipeline(device, layout, format, &shader, "octara material", Phase::Material, true)
                })
                .await
                .map_err(|error| eprintln!("Octara material at {node:?}: {error}"))
                .ok();
                entry.insert(pipeline)
            }
        };
        pipeline.is_some()
    }

    pub(super) fn material_pipeline(&self, material: Material, format: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        self.materials[&(material, format)].as_ref().expect("validated material pipeline")
    }

    pub(super) fn prune_canvases(&mut self) {
        self.canvases.retain(|_, canvas| canvas.owner.strong_count() != 0);
    }

    pub(super) fn finish_canvases(&mut self, active: usize) {
        self.targets.truncate(active);
    }

    pub(super) async fn encode_canvas(
        &mut self,
        device: &wgpu::Device,
        paint: &CanvasPaint,
        scale: f32,
        target: CanvasTarget<'_>,
        mut measurement: Option<&mut FrameMeasurement>,
    ) -> Result<(wgpu::TextureView, wgpu::CommandBuffer), PaintFallback> {
        let prepared = Self::prepare_canvas(&mut self.canvases, device, paint).await?;
        let start = measurement.is_some().then(Instant::now);
        let reuse = match target {
            CanvasTarget::Transient(slot) => self.targets.get(slot),
            CanvasTarget::Working(texture) => texture,
        };
        let output = prepared.encode(device, paint, scale, reuse, CanvasSource::Local, measurement.as_deref_mut()).await;
        if let Some(start) = start
            && let Some(measurement) = measurement
        {
            measurement.canvas_encode += start.elapsed();
        }
        let (view, command) = output?;
        if let CanvasTarget::Transient(slot) = target {
            if let Some(target) = self.targets.get_mut(slot) {
                *target = view.texture().clone();
            } else {
                self.targets.push(view.texture().clone());
            }
        }
        Ok((view, command))
    }

    pub(super) async fn encode_backdrop(
        &mut self,
        device: &wgpu::Device,
        paint: &CanvasPaint,
        scale: f32,
        target: Option<&wgpu::Texture>,
        copy: BackdropCopy<'_>,
        measurement: Option<&mut FrameMeasurement>,
    ) -> Result<(wgpu::TextureView, wgpu::CommandBuffer), PaintFallback> {
        Self::prepare_canvas(&mut self.canvases, device, paint)
            .await?
            .encode(device, paint, scale, target, CanvasSource::Backdrop(copy, &mut self.backdrop_inputs), measurement)
            .await
    }

    pub(super) async fn encode_transition(
        &mut self,
        device: &wgpu::Device,
        paint: &CanvasPaint,
        scale: f32,
        images: Transition<'_>,
        measurement: Option<&mut FrameMeasurement>,
    ) -> Result<(wgpu::TextureView, wgpu::CommandBuffer), PaintFallback> {
        Self::prepare_canvas(&mut self.canvases, device, paint)
            .await?
            .encode(device, paint, scale, None, CanvasSource::Transition(images), measurement)
            .await
    }

    async fn prepare_canvas<'a>(
        canvases: &'a mut FxHashMap<usize, PreparedCanvas>,
        device: &wgpu::Device,
        paint: &CanvasPaint,
    ) -> Result<&'a mut PreparedCanvas, PaintFallback> {
        let canvas = paint.program.downcast_ref::<CanvasProgram>().ok_or(PaintFallback::UnsupportedProgram)?;
        let owner = paint.program.downgrade();
        let name = match canvas.mode {
            CanvasMode::Volatile | CanvasMode::Retained => "canvas",
            CanvasMode::Backdrop => "backdrop",
            CanvasMode::Transition => "transition",
        };
        use std::collections::hash_map::Entry;
        Ok(match canvases.entry(owner.as_ptr().cast::<()>().addr()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let prepare = canvas.payload::<Box<CanvasPrepare>>().ok_or(PaintFallback::UnsupportedProgram)?;
                let draw = checked_async(device, || prepare(device))
                    .await
                    .map_err(|error| eprintln!("Octara {name} preparation at {:?}: {error}", paint.source))
                    .ok();
                entry.insert(PreparedCanvas { owner, draw, reported_failure: false })
            }
        })
    }
}

impl PreparedCanvas {
    async fn encode(
        &mut self,
        device: &wgpu::Device,
        paint: &CanvasPaint,
        scale: f32,
        target: Option<&wgpu::Texture>,
        mut source: CanvasSource<'_>,
        mut measurement: Option<&mut FrameMeasurement>,
    ) -> Result<(wgpu::TextureView, wgpu::CommandBuffer), PaintFallback> {
        let draw = self.draw.as_ref().ok_or(PaintFallback::PreparationFailed)?;
        let [width, height] = paint.size;
        let name = match source {
            CanvasSource::Local => "canvas",
            CanvasSource::Backdrop(..) => "backdrop",
            CanvasSource::Transition(_) => "transition",
        };
        let mut timestamps = measurement.as_mut().and_then(|measurement| measurement.timestamps.as_mut());
        let checkpoint = timestamps.as_deref().map(TimestampFrame::checkpoint);
        let output =
            checked_async(device, || record_canvas(draw.as_ref(), device, paint, scale, target, &mut source, timestamps.as_deref_mut()))
                .await;
        match output {
            Ok((output, sample)) => {
                if let Some(measurement) = measurement.as_deref_mut() {
                    measurement.profile.canvas_pixels += u64::from(width) * u64::from(height);
                }
                if let (CanvasSource::Backdrop(copy, inputs), Some((input, reused))) = (source, sample) {
                    if !reused {
                        inputs.retain_view(input);
                    }
                    if let Some(measurement) = measurement {
                        let [width, height] = copy.size;
                        measurement.profile.backdrop_copy_pixels += u64::from(width) * u64::from(height);
                        measurement.profile.backdrop_allocations += u32::from(!reused);
                        measurement.profile.backdrop_reuses += u32::from(reused);
                        let peak = &mut measurement.profile.backdrop_cache_peak_bytes;
                        *peak = (*peak).max(inputs.bytes);
                    }
                }
                Ok(output)
            }
            Err(error) => {
                if let Some((timestamps, checkpoint)) = timestamps.zip(checkpoint) {
                    timestamps.discard_since(checkpoint);
                }
                if !self.reported_failure {
                    eprintln!("Octara {name} at {:?}: {error}", paint.source);
                    self.reported_failure = true;
                }
                Err(PaintFallback::EncodingFailed)
            }
        }
    }
}

fn record_canvas(
    draw: &dyn CanvasDraw,
    device: &wgpu::Device,
    paint: &CanvasPaint,
    scale: f32,
    target: Option<&wgpu::Texture>,
    source: &mut CanvasSource<'_>,
    mut timestamps: Option<&mut TimestampFrame>,
) -> ((wgpu::TextureView, wgpu::CommandBuffer), Option<(wgpu::TextureView, bool)>) {
    let [width, height] = paint.size;
    let (label, kind, clear) = match source {
        CanvasSource::Local => ("octara canvas", GpuPassKind::Canvas, GpuPassKind::CanvasClear),
        CanvasSource::Backdrop(..) => ("octara backdrop", GpuPassKind::Backdrop, GpuPassKind::BackdropClear),
        CanvasSource::Transition(_) => ("octara transition", GpuPassKind::Transition, GpuPassKind::TransitionClear),
    };
    let texture =
        target.filter(|t| t.width() == width && t.height() == height).cloned().unwrap_or_else(|| canvas_target(device, width, height));
    let view = texture.create_view(&Default::default());
    // A failed author pass can invalidate its encoder, so record each canvas independently.
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
    let sample = if let CanvasSource::Backdrop(copy, inputs) = &mut *source {
        let (input, reused) = inputs.view(device, copy);
        TimestampFrame::commands(timestamps.as_deref_mut(), &mut encoder, GpuPassKind::BackdropCopy, |encoder, _| {
            copy.encode(encoder, input.texture());
        });
        Some((input, reused))
    } else {
        None
    };
    TimestampFrame::commands(timestamps, &mut encoder, kind, |encoder, mut timestamps| {
        let timestamp_writes = timestamps.as_deref_mut().and_then(|timestamps| timestamps.clear_pass(clear));
        drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("octara canvas clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
            })],
            timestamp_writes,
            ..Default::default()
        }));
        let input = match &*source {
            CanvasSource::Local => CanvasInput::Local,
            CanvasSource::Backdrop(copy, _) => CanvasInput::Backdrop(Backdrop {
                view: &sample.as_ref().expect("copied backdrop input").0,
                size: copy.size,
                offset: copy.offset,
            }),
            CanvasSource::Transition(images) => CanvasInput::Transition(*images),
        };
        draw(
            &mut CanvasFrame {
                device,
                encoder,
                target: &view,
                size: paint.size,
                scale,
                rect: paint.rect,
                radius: paint.radius,
                sampled_at: paint.sampled_at,
                input: &paint.input,
                timestamps,
            },
            input,
        );
    });
    ((view, encoder.finish()), sample)
}

fn canvas_target(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("octara canvas target"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: WORKING_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

pub(super) async fn checked_async<T>(device: &wgpu::Device, record: impl FnOnce() -> T) -> Result<T, wgpu::Error> {
    let (output, error) = with_error_scopes_async(device, record).await;
    error.map_or(Ok(output), Err)
}

#[cfg(not(target_arch = "wasm32"))]
#[doc(hidden)]
pub fn with_error_scopes<T>(device: &wgpu::Device, record: impl FnOnce() -> T) -> (T, Option<wgpu::Error>) {
    crate::block_on(with_error_scopes_async(device, record))
}

pub(super) async fn with_error_scopes_async<T>(device: &wgpu::Device, record: impl FnOnce() -> T) -> (T, Option<wgpu::Error>) {
    let scopes = ErrorScopes::new(device);
    let output = record();
    (output, scopes.pop().await)
}

pub struct ErrorScopes {
    validation: wgpu::ErrorScopeGuard,
    internal: wgpu::ErrorScopeGuard,
    memory: wgpu::ErrorScopeGuard,
}

impl ErrorScopes {
    pub fn new(device: &wgpu::Device) -> Self {
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        Self { validation, internal, memory }
    }

    /// Pops all scopes synchronously. Only their receipts are awaited.
    pub fn pop(self) -> impl Future<Output = Option<wgpu::Error>> {
        let validation = self.validation.pop();
        let internal = self.internal.pop();
        let memory = self.memory.pop();
        async move {
            let validation = validation.await;
            let internal = internal.await;
            let memory = memory.await;
            validation.or(internal).or(memory)
        }
    }
}

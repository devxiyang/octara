use super::profile::{FrameMeasurement, GpuPassKind, TimestampFrame};
use super::{BackdropRegion, CanvasPaint, FORMAT, GEOMETRY_SHADER, render_pipeline};
use octara_primitives::capture::PaintFallback;
use octara_primitives::identity::NodeId;
use octara_primitives::paint::{PaintProgram, Rect};
use rustc_hash::{FxBuildHasher, FxHashMap};
use std::any::Any;
use std::collections::VecDeque;
use std::hash::{BuildHasher, Hash, Hasher};
use std::ptr;
use std::sync::Weak;
use std::time::{Duration, Instant};

/// A fragment program for an element's rounded box. Define
/// `fn material(uv: vec2f, params: vec4f) -> vec4f` in WGSL. The top-left of the box is
/// `(0, 0)`, and the bottom-right is `(1, 1)`. Return straight-alpha sRGB-encoded RGBA.
///
/// Octara supplies antialiasing, clipping and compositing. Instances of the same shader
/// can be batched with different parameter values. An invalid shader uses the box's
/// ordinary paint.
pub fn material(wgsl: &'static str) -> PaintProgram {
    PaintProgram::new(Material { wgsl, hash: FxBuildHasher.hash_one(wgsl) })
}

#[derive(Clone, Copy, Debug, Eq)]
pub(super) struct Material {
    wgsl: &'static str,
    hash: u64,
}

impl PartialEq for Material {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash && (ptr::eq(self.wgsl, other.wgsl) || self.wgsl == other.wgsl)
    }
}

impl Hash for Material {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.hash);
    }
}

/// A canvas draw records render, compute or copy commands into a local GPU target. The
/// renderer prepares the callback once for a shared program on its device. Each visible
/// draw receives the frame's size, scale and parameters.
///
/// The renderer clears the target to transparent before invoking the callback. Write
/// premultiplied sRGB-encoded RGBA to it. Preparation and encoding validation errors use the
/// box's ordinary paint. The renderer submits the recorded commands with the rest of the frame,
/// so the resources they use must stay unmapped and undestroyed through submission.
pub fn canvas<C>(prepare: impl Fn(&wgpu::Device) -> C + Send + Sync + 'static) -> PaintProgram
where
    C: for<'a> Fn(&mut CanvasFrame<'a>) + Send + Sync + 'static,
{
    local_canvas(prepare, CanvasMode::Volatile)
}

/// A canvas whose pixels can be reused after a compatible output is submitted.
///
/// Draw deterministic pixels for the frozen `params`, local box size and resolved radius,
/// pixel extent, scale, and device. The target is `Rgba8Unorm`, the box origin is `(0, 0)`,
/// and `sampled_at` is `None`. Supply a parameter version for changing author resources and
/// keep the draw free of side effects that callers depend on. Use [`canvas`] when drawing
/// depends on window coordinates, frame time, or undeclared state.
///
/// The renderer may skip the callback and reuse a mount's last compatible submitted pixels.
/// Position, clipping and opacity are applied when the output is composited. Cache misses,
/// eviction and device replacement can invoke the callback again. Drawing and validation
/// otherwise follow [`canvas`].
pub fn retained_canvas<C>(prepare: impl Fn(&wgpu::Device) -> C + Send + Sync + 'static) -> PaintProgram
where
    C: for<'a> Fn(&mut CanvasFrame<'a>) + Send + Sync + 'static,
{
    local_canvas(prepare, CanvasMode::Retained)
}

fn local_canvas<C>(prepare: impl Fn(&wgpu::Device) -> C + Send + Sync + 'static, mode: CanvasMode) -> PaintProgram
where
    C: for<'a> Fn(&mut CanvasFrame<'a>) + Send + Sync + 'static,
{
    PaintProgram::new(Canvas {
        prepare: Box::new(move |device| {
            let draw = prepare(device);
            Box::new(move |frame, _| draw(frame))
        }),
        mode,
    })
}

/// A canvas that can sample already-drawn pixels in its current compositing group. The
/// renderer copies the canvas pixel grid, cropped to the clip bounds and target, at this
/// item's position in the display list. An isolated opacity group starts with transparent pixels.
/// Preparation, output, clipping and validation use the same conventions as [`canvas`].
/// When the current attachment cannot be copied, the renderer draws the box's ordinary paint.
/// An exiting mount replays its last submitted output or uses the ordinary paint if none exists.
pub fn backdrop<C>(prepare: impl Fn(&wgpu::Device) -> C + Send + Sync + 'static) -> PaintProgram
where
    C: for<'a> Fn(&mut CanvasFrame<'a>, Backdrop<'a>) + Send + Sync + 'static,
{
    PaintProgram::new(Canvas {
        prepare: Box::new(move |device| {
            let draw = prepare(device);
            Box::new(move |frame, backdrop| draw(frame, backdrop.expect("backdrop input")))
        }),
        mode: CanvasMode::Backdrop,
    })
}

/// The pixels available to a [`backdrop`] callback in this draw.
pub struct Backdrop<'a> {
    /// Premultiplied sRGB-encoded pixels in the attachment's format (such as `Bgra8Unorm` or
    /// `Rgba8Unorm`). The renderer owns the texture. Sample or copy from it within the callback.
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
    /// Single-sampled `Rgba8Unorm`, with render attachment, storage, sampling and copy
    /// usages. The renderer owns this view and its texture. Use them within the callback.
    pub target: &'a wgpu::TextureView,
    /// Width and height in pixels, rounded up from the laid-out size at `scale`.
    pub size: [u32; 2],
    pub scale: f32,
    /// The resolved box in logical window coordinates, or local coordinates for a retained canvas.
    pub rect: Rect,
    /// The box's resolved corner radius in logical pixels.
    pub radius: f32,
    /// The App clock sampled by a published frame, when available. Retained canvases receive `None`.
    pub sampled_at: Option<Duration>,
    pub params: [f32; 4],
    timestamps: Option<&'a mut TimestampFrame>,
}

impl CanvasFrame<'_> {
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

type CanvasDraw = dyn for<'a> Fn(&mut CanvasFrame<'a>, Option<Backdrop<'a>>) + Send + Sync;
type CanvasPrepare = dyn Fn(&wgpu::Device) -> Box<CanvasDraw> + Send + Sync;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CanvasMode {
    Volatile,
    Retained,
    Backdrop,
}

pub(super) struct Canvas {
    prepare: Box<CanvasPrepare>,
    pub(super) mode: CanvasMode,
}

struct PreparedCanvas {
    /// Keeps the key's allocation distinct and tells us when the app has released the program.
    owner: Weak<dyn Any + Send + Sync>,
    draw: Option<Box<CanvasDraw>>,
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
    pub(super) fn new(source: &'a wgpu::Texture, format: wgpu::TextureFormat, region: BackdropRegion) -> Self {
        Self {
            source,
            format,
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
    materials: FxHashMap<Material, Option<wgpu::RenderPipeline>>,
    canvases: FxHashMap<usize, PreparedCanvas>,
    /// Reuse an exact-size allocation at each frame slot. Never reuse its previous pixels.
    targets: Vec<wgpu::Texture>,
    backdrop_inputs: BackdropInputs,
}

pub(super) enum CanvasTarget<'a> {
    Transient(usize),
    Working(Option<&'a wgpu::Texture>),
}

impl Programs {
    pub(super) fn prepare_material(
        &mut self,
        device: &wgpu::Device,
        layout: &wgpu::PipelineLayout,
        format: wgpu::TextureFormat,
        material: Material,
        node: NodeId,
    ) -> bool {
        self.materials
            .entry(material)
            .or_insert_with(|| {
                checked(device, || {
                    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("octara material"),
                        source: wgpu::ShaderSource::Wgsl(
                            format!("{}\n{GEOMETRY_SHADER}{}", material.wgsl, include_str!("material.wgsl")).into(),
                        ),
                    });
                    render_pipeline(
                        device,
                        layout,
                        format,
                        &shader,
                        "fs_material",
                        "octara material",
                        wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
                    )
                })
                .map_err(|error| eprintln!("Octara material at {node:?}: {error}"))
                .ok()
            })
            .is_some()
    }

    pub(super) fn material_pipeline(&self, material: Material) -> &wgpu::RenderPipeline {
        self.materials[&material].as_ref().expect("validated material pipeline")
    }

    pub(super) fn prune_canvases(&mut self) {
        self.canvases.retain(|_, canvas| canvas.owner.strong_count() != 0);
    }

    pub(super) fn finish_canvases(&mut self, active: usize) {
        self.targets.truncate(active);
    }

    pub(super) fn encode_canvas(
        &mut self,
        device: &wgpu::Device,
        paint: &CanvasPaint,
        scale: f32,
        target: CanvasTarget<'_>,
        mut measurement: Option<&mut FrameMeasurement>,
    ) -> Result<(wgpu::TextureView, wgpu::CommandBuffer), PaintFallback> {
        let prepared = Self::prepare_canvas(&mut self.canvases, device, paint);
        let start = measurement.is_some().then(Instant::now);
        let reuse = match target {
            CanvasTarget::Transient(slot) => self.targets.get(slot),
            CanvasTarget::Working(texture) => texture,
        };
        let output = prepared.encode(device, paint, scale, reuse, None, measurement.as_deref_mut());
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

    pub(super) fn encode_backdrop(
        &mut self,
        device: &wgpu::Device,
        paint: &CanvasPaint,
        scale: f32,
        target: Option<&wgpu::Texture>,
        copy: BackdropCopy<'_>,
        measurement: Option<&mut FrameMeasurement>,
    ) -> Result<(wgpu::TextureView, wgpu::CommandBuffer), PaintFallback> {
        Self::prepare_canvas(&mut self.canvases, device, paint).encode(
            device,
            paint,
            scale,
            target,
            Some((copy, &mut self.backdrop_inputs)),
            measurement,
        )
    }

    fn prepare_canvas<'a>(
        canvases: &'a mut FxHashMap<usize, PreparedCanvas>,
        device: &wgpu::Device,
        paint: &CanvasPaint,
    ) -> &'a mut PreparedCanvas {
        let canvas = paint.program.downcast_ref::<Canvas>().expect("a canvas request");
        let owner = paint.program.downgrade();
        let name = if canvas.mode == CanvasMode::Backdrop { "backdrop" } else { "canvas" };
        canvases.entry(owner.as_ptr().cast::<()>().addr()).or_insert_with(|| {
            let draw = checked(device, || (canvas.prepare)(device))
                .map_err(|error| eprintln!("Octara {name} preparation at {:?}: {error}", paint.fallback_source))
                .ok();
            PreparedCanvas { owner, draw, reported_failure: false }
        })
    }
}

impl PreparedCanvas {
    fn encode(
        &mut self,
        device: &wgpu::Device,
        paint: &CanvasPaint,
        scale: f32,
        target: Option<&wgpu::Texture>,
        mut copy: Option<(BackdropCopy<'_>, &mut BackdropInputs)>,
        mut measurement: Option<&mut FrameMeasurement>,
    ) -> Result<(wgpu::TextureView, wgpu::CommandBuffer), PaintFallback> {
        let draw = self.draw.as_ref().ok_or(PaintFallback::PreparationFailed)?;
        let [width, height] = paint.size;
        let (name, label, kind, clear) = if copy.is_some() {
            ("backdrop", "octara backdrop", GpuPassKind::Backdrop, GpuPassKind::BackdropClear)
        } else {
            ("canvas", "octara canvas", GpuPassKind::Canvas, GpuPassKind::CanvasClear)
        };
        let mut timestamps = measurement.as_mut().and_then(|measurement| measurement.timestamps.as_mut());
        let checkpoint = timestamps.as_deref().map(TimestampFrame::checkpoint);
        let output = checked(device, || {
            let texture = target
                .filter(|t| t.width() == width && t.height() == height)
                .cloned()
                .unwrap_or_else(|| canvas_target(device, width, height));
            let view = texture.create_view(&Default::default());
            // A failed author pass can invalidate its encoder, so record each canvas independently.
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
            let sample = copy.as_mut().map(|(copy, inputs)| {
                let (input, reused) = inputs.view(device, copy);
                TimestampFrame::commands(timestamps.as_deref_mut(), &mut encoder, GpuPassKind::BackdropCopy, |encoder, _| {
                    copy.encode(encoder, input.texture());
                });
                (input, reused)
            });
            TimestampFrame::commands(timestamps.as_deref_mut(), &mut encoder, kind, |encoder, mut timestamps| {
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
                let backdrop = copy.as_ref().zip(sample.as_ref()).map(|((copy, _), (view, _))| Backdrop {
                    view,
                    size: copy.size,
                    offset: copy.offset,
                });
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
                        params: paint.params,
                        timestamps,
                    },
                    backdrop,
                );
            });
            ((view, encoder.finish()), sample)
        });
        match output {
            Ok((output, sample)) => {
                if let Some(measurement) = measurement.as_deref_mut() {
                    let pixels = |[width, height]: [u32; 2]| u64::from(width) * u64::from(height);
                    measurement.profile.canvas_pixels += pixels(paint.size);
                    measurement.profile.backdrop_copy_pixels += copy.as_ref().map_or(0, |(copy, _)| pixels(copy.size));
                }
                if let Some(((_, inputs), (input, reused))) = copy.zip(sample) {
                    if !reused {
                        inputs.retain_view(input);
                    }
                    if let Some(measurement) = measurement {
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
                    eprintln!("Octara {name} at {:?}: {error}", paint.fallback_source);
                    self.reported_failure = true;
                }
                Err(PaintFallback::EncodingFailed)
            }
        }
    }
}

pub(super) fn canvas_size(rect: [f32; 4], max: u32) -> Option<[u32; 2]> {
    let [x, y, width, height] = rect;
    let size = [width.ceil(), height.ceil()];
    (x.is_finite() && y.is_finite() && size.into_iter().all(|axis| axis >= 1.0 && axis <= max as f32)).then(|| size.map(|axis| axis as u32))
}

fn canvas_target(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("octara canvas target"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn checked<T>(device: &wgpu::Device, record: impl FnOnce() -> T) -> Result<T, wgpu::Error> {
    let (output, error) = with_error_scopes(device, record);
    error.map_or(Ok(output), Err)
}

pub(super) fn with_error_scopes<T>(device: &wgpu::Device, record: impl FnOnce() -> T) -> (T, Option<wgpu::Error>) {
    let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let output = record();
    let validation = pollster::block_on(validation.pop());
    let internal = pollster::block_on(internal.pop());
    let memory = pollster::block_on(memory.pop());
    (output, validation.or(internal).or(memory))
}

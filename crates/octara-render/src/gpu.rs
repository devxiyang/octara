//! The GPU renderer: wgpu, instanced batches for each target, and author-defined paint programs.
//! Glyphs and paths use the reference rasterizers ([`crate::glyphs::GlyphCache`], [`crate::paths`]).
//! Draws sample array-backed atlases and preserve painter order at overlapping blended pixels.

use octara_model::capture::PaintSample;
use octara_model::paint::DisplayList;

mod atlas;
mod backing;
mod canvases;
mod custom;
mod device_loss;
mod encode;
#[cfg(not(target_arch = "wasm32"))]
mod external;
mod outputs;
mod pipeline;
mod prepare;
mod profile;
mod readback;
mod recording;
mod resolve;
mod retained;
mod surface;
mod upload;

pub use crate::plan::{FrameProfile, GpuPassKind, GpuPassTiming, GpuTimings, OutputColor};

pub use atlas::RecordError;
use atlas::SceneTextures;
use backing::{Backing, BackingPipelines};
#[doc(hidden)]
pub use canvases::CanvasUpdates;
#[cfg(not(target_arch = "wasm32"))]
#[doc(hidden)]
pub use custom::with_error_scopes;
pub use custom::{Backdrop, CanvasFrame, Transition, TransitionImage, backdrop, canvas, material, retained_canvas, transition};
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
#[doc(hidden)]
pub use custom::{ErrorScopes, local};
#[doc(hidden)]
pub use device_loss::DeviceLoss;
#[cfg(not(target_arch = "wasm32"))]
pub use external::external_texture;
pub use outputs::PendingPresentation;
use outputs::{CanvasCache, PresentationHistory};
use pipeline::PipelineCache;
use profile::TimestampPool;
pub use readback::{MappedReadback, Readback, RecordedCapture};
pub use recording::RecordedFrame;
pub use resolve::FrameTarget;
use resolve::OutputResolve;
use retained::CachedFrame;
pub use surface::{SurfaceFrame, SurfaceRenderer, SurfaceRendererFactory};
use upload::FrameBuffers;

// Layers and canvases store linear-light, premultiplied RGBA.
const WORKING_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// A display list sampled at a particular scale and time.
#[derive(Clone, Copy)]
pub struct FrameInput<'a> {
    pub list: &'a DisplayList,
    pub scale: f32,
    /// The host's settled drawing revision and sample time, when available.
    pub sample: Option<PaintSample>,
}

/// Renders display lists on a device shared by independently configured targets.
pub struct WgpuRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipelines: PipelineCache,
    surface_renderer: Option<Box<dyn SurfaceRenderer>>,
    output_resolve: OutputResolve,
    buffers: FrameBuffers,
    prepared: Option<CachedFrame>,
    backing: Option<Backing>,
    backing_pipelines: BackingPipelines,
    target_generation: u64,
    textures: Option<SceneTextures>,
    /// Layer targets, reused while their allocation covers the same size bucket.
    layers: Vec<Option<wgpu::Texture>>,
    /// A 1×1 texture bound where no layer is.
    no_layer: wgpu::Texture,
    depth: rustc_hash::FxHashMap<[u32; 2], wgpu::TextureView>,
    programs: custom::Programs,
    canvas_cache: CanvasCache,
    presentation_history: PresentationHistory,
    timestamps: TimestampPool,
}

impl WgpuRenderer {
    /// A renderer on the default adapter, or `None` when there is no usable GPU.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new() -> Option<WgpuRenderer> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = crate::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
        let (device, queue) = crate::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
        Some(Self::with_device(device, queue))
    }

    /// A renderer on an existing device. Attachment formats are selected at recording time.
    pub fn with_device(device: wgpu::Device, queue: wgpu::Queue) -> WgpuRenderer {
        let buffers = FrameBuffers::new(&device);
        let no_layer = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("octara no layer"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: WORKING_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let pipelines = PipelineCache::new(&device);
        WgpuRenderer {
            device,
            queue,
            pipelines,
            surface_renderer: None,
            output_resolve: OutputResolve::default(),
            buffers,
            prepared: None,
            backing: None,
            backing_pipelines: BackingPipelines::default(),
            target_generation: 0,
            textures: None,
            layers: Vec::new(),
            no_layer,
            depth: Default::default(),
            programs: custom::Programs::default(),
            canvas_cache: CanvasCache::default(),
            presentation_history: PresentationHistory::default(),
            timestamps: TimestampPool::default(),
        }
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Invalidates retained geometry and scene pixels when the host reconfigures its presentation target.
    pub fn set_target_generation(&mut self, generation: u64) {
        if self.target_generation != generation {
            self.target_generation = generation;
            self.prepared = None;
            self.backing = None;
        }
    }

    /// Installs a backend for recording display lists into this device's attachments.
    pub fn with_surface_renderer(mut self, renderer: Box<dyn SurfaceRenderer>) -> Self {
        self.surface_renderer = Some(renderer);
        self
    }

    /// Keeps a submitted frame's backdrop outputs and transition regions once the host accepts it.
    ///
    /// # Panics
    /// Panics if the token contains outputs from a different renderer.
    pub fn accept_presentation(&mut self, pending: PendingPresentation) {
        self.presentation_history.accept(pending);
    }

    pub fn renderer(&self) -> &'static str {
        "gpu"
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;

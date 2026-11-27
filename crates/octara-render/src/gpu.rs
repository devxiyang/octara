//! The GPU renderer: wgpu, instanced batches for each target, and author-defined paint programs.
//! Glyphs and paths use the reference rasterizers ([`crate::glyphs::GlyphCache`], [`crate::paths`]).
//! Draws bind the pages or dedicated textures that hold their samples, in painter order.

use octara_model::capture::PaintSample;
use octara_model::paint::DisplayList;

mod atlas;
mod batch;
mod canvases;
mod custom;
mod device_loss;
mod encode;
mod glyph_pipeline;
mod outputs;
mod pipeline;
pub mod plan;
mod prepare;
mod profile;
mod readback;
mod recording;
mod resolve;
mod surface;
mod upload;

pub use atlas::RecordError;
use atlas::SceneTextures;
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
use glyph_pipeline::GlyphPipeline;
pub use outputs::PendingPresentation;
use outputs::{CanvasCache, PresentationHistory};
use pipeline::PipelineCache;
use profile::TimestampPool;
pub use profile::{FrameProfile, GpuPassKind, GpuPassTiming, GpuTimings};
pub use readback::{MappedReadback, Readback, RecordedCapture};
pub use recording::RecordedFrame;
use resolve::OutputResolve;
pub use resolve::{FrameTarget, OutputColor};
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
pub struct GpuRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipelines: PipelineCache,
    glyph_pipeline: GlyphPipeline,
    surface_renderer: Option<Box<dyn SurfaceRenderer>>,
    output_resolve: OutputResolve,
    buffers: FrameBuffers,
    textures: Option<SceneTextures>,
    /// Layer targets, reused while their allocation covers the same size bucket.
    layers: Vec<Option<wgpu::Texture>>,
    /// A 1×1 texture bound where no layer is.
    no_layer: wgpu::Texture,
    programs: custom::Programs,
    canvas_cache: CanvasCache,
    presentation_history: PresentationHistory,
    timestamps: TimestampPool,
}

impl GpuRenderer {
    /// A renderer on the default adapter, or `None` when there is no usable GPU.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new() -> Option<GpuRenderer> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
        Some(Self::with_device(device, queue))
    }

    /// A renderer on an existing device. Attachment formats are selected at recording time.
    pub fn with_device(device: wgpu::Device, queue: wgpu::Queue) -> GpuRenderer {
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
        let glyph_pipeline = GlyphPipeline::new(&device);
        GpuRenderer {
            device,
            queue,
            pipelines,
            glyph_pipeline,
            surface_renderer: None,
            output_resolve: OutputResolve::default(),
            buffers,
            textures: None,
            layers: Vec::new(),
            no_layer,
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

use super::{CanvasRequest, FORMAT, GEOMETRY_SHADER, render_pipeline};
use octara_primitives::identity::NodeId;
use octara_primitives::paint::PaintProgram;
use rustc_hash::{FxBuildHasher, FxHashMap};
use std::any::Any;
use std::hash::{BuildHasher, Hash, Hasher};
use std::ptr;
use std::sync::Weak;

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
    PaintProgram::new(Canvas { prepare: Box::new(move |device| Box::new(prepare(device))) })
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
    pub params: [f32; 4],
}

type CanvasDraw = dyn for<'a> Fn(&mut CanvasFrame<'a>) + Send + Sync;
type CanvasPrepare = dyn Fn(&wgpu::Device) -> Box<CanvasDraw> + Send + Sync;

pub(super) struct Canvas {
    prepare: Box<CanvasPrepare>,
}

struct PreparedCanvas {
    /// Keeps the key's allocation distinct and tells us when the app has released the program.
    owner: Weak<dyn Any + Send + Sync>,
    draw: Option<Box<CanvasDraw>>,
    reported_failure: bool,
}

#[derive(Default)]
pub(super) struct Programs {
    materials: FxHashMap<Material, Option<wgpu::RenderPipeline>>,
    canvases: FxHashMap<usize, PreparedCanvas>,
    /// Reuse an exact-size allocation at each frame slot. Never reuse its previous pixels.
    targets: Vec<wgpu::Texture>,
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
        request: &CanvasRequest,
        scale: f32,
        slot: usize,
    ) -> Option<(wgpu::TextureView, wgpu::CommandBuffer)> {
        let canvas = request.program.downcast_ref::<Canvas>().expect("a canvas request");
        let owner = request.program.downgrade();
        let prepared = self.canvases.entry(owner.as_ptr().cast::<()>().addr()).or_insert_with(|| {
            let draw = checked(device, || (canvas.prepare)(device))
                .map_err(|error| eprintln!("Octara canvas preparation at {:?}: {error}", request.source))
                .ok();
            PreparedCanvas { owner, draw, reported_failure: false }
        });
        let draw = prepared.draw.as_ref()?;
        let [width, height] = request.size;
        let result = checked(device, || {
            let texture = self
                .targets
                .get(slot)
                .filter(|t| t.width() == width && t.height() == height)
                .cloned()
                .unwrap_or_else(|| canvas_target(device, width, height));
            let view = texture.create_view(&Default::default());
            // A failed author pass can invalidate its encoder, so record each canvas independently.
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("octara canvas") });
            drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("octara canvas clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            }));
            draw(&mut CanvasFrame { device, encoder: &mut encoder, target: &view, size: request.size, scale, params: request.params });
            (texture, view, encoder.finish())
        });
        let (texture, view, command) = result
            .map_err(|error| {
                if !prepared.reported_failure {
                    eprintln!("Octara canvas at {:?}: {error}", request.source);
                    prepared.reported_failure = true;
                }
            })
            .ok()?;
        if let Some(target) = self.targets.get_mut(slot) {
            *target = texture;
        } else {
            self.targets.push(texture);
        }
        Some((view, command))
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
    let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let output = record();
    let validation = pollster::block_on(validation.pop());
    let internal = pollster::block_on(internal.pop());
    let memory = pollster::block_on(memory.pop());
    validation.or(internal).or(memory).map_or(Ok(output), Err)
}

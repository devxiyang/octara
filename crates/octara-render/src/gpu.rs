//! The GPU renderer: wgpu, instanced batches for each target, and author-defined paint programs.
//! Glyphs and paths are rasterized as by the reference renderer
//! ([`GlyphCache`], [`paths`]) and packed into an alpha atlas (masks; a path larger than a tile
//! takes several) and an RGBA atlas (color glyphs). When an atlas fills up, every atlas starts
//! over and the frame is built again with only what it draws.

use crate::glyphs::{GlyphCache, SUBPIXEL_STEPS};
use crate::paths::{self, MaskKey};
use crate::shape::push_clip;
use bytemuck::{Pod, Zeroable};
use octara_primitives::capture::{Painter, Picture};
use octara_primitives::paint::{CustomPaint, DisplayList, GlyphRun, ImagePaint, Item, PaintProgram, PathPaint, Quad, Rect, Shadow};
use octara_primitives::theme::Color;
use rustc_hash::FxHashMap;
use std::alloc::Layout;
use std::ops::Range;

mod custom;
pub use custom::{CanvasFrame, canvas, material};

const ATLAS: u32 = 2048;
/// The mask atlas's size where the device allows it (paths can be window-sized).
const MASK_ATLAS: u32 = 4096;
/// The largest piece of a path's mask in the atlas.
const TILE: u32 = 1024;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// No clip: a rect far larger than any window.
const NO_CLIP: [f32; 4] = [-1.0e6, -1.0e6, 2.0e6, 2.0e6];
const KIND_QUAD: f32 = 0.0;
const KIND_SHADOW: f32 = 3.0;
const KIND_IMAGE: f32 = 4.0;
const KIND_LAYER: f32 = 5.0;
const KIND_CANVAS: f32 = 7.0;
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
    /// radius, kind (0 quad, 1 mask glyph, 2 color glyph, 3 shadow, 4 image, 5 layer, 6 path,
    /// 7 canvas),
    /// clip radius, has fill (a shadow's σ is `border[0]`; an image's clipping box is `border`, its
    /// radius `params[0]`, its opacity `fill[3]`, its atlas origin and size `uv`; a path's
    /// gradient box is `border`)
    params: [f32; 4],
    /// Atlas coordinates, or the four author parameters for a material.
    uv: [f32; 4],
    /// A gradient's end color (straight); `fill` is its start.
    grad_color: [f32; 4],
    /// A gradient's angle (degrees), start and end fractions, and 1 if there is one.
    grad: [f32; 4],
}

impl Inst {
    fn visible_bounds(&self, window: &Rect) -> Rect {
        // Match the vertex shader's expansion for antialiasing and shadows.
        let grow = match self.params[1] {
            KIND_QUAD | KIND_IMAGE | KIND_CANVAS => 1.0,
            KIND_SHADOW => 3.0 * self.border[0] + 1.0,
            _ => 0.0,
        };
        let [x, y, w, h] = self.rect;
        let bounds = Rect::new(x - grow, y - grow, w + 2.0 * grow, h + 2.0 * grow);
        let [x, y, w, h] = self.clip;
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
    Cutout,
    Layer(usize),
    Material(custom::Material),
    Canvas(usize),
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
    program: PaintProgram,
    fallback: Inst,
    source: octara_primitives::identity::NodeId,
    size: [u32; 2],
    params: [f32; 4],
}

struct PreparedFrame {
    passes: Vec<Pass>,
    instance_offsets: Vec<u32>,
    background: wgpu::Color,
    layer_views: Vec<wgpu::TextureView>,
    bind: wgpu::BindGroup,
    layer_binds: Vec<wgpu::BindGroup>,
    canvas_binds: Vec<wgpu::BindGroup>,
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

/// A frame's recorded commands and the exclusive renderer borrow protecting its staged data.
///
/// Submit or drop this frame before using the renderer again. Dropping it discards the encoder.
/// Its uploads remain queued for the next submission, including atlas entries cached during
/// preparation.
#[must_use = "submit the recorded frame to execute its commands"]
pub struct RecordedFrame<'renderer> {
    before: Vec<wgpu::CommandBuffer>,
    encoder: wgpu::CommandEncoder,
    queue: &'renderer wgpu::Queue,
}

impl RecordedFrame<'_> {
    /// Borrows the frame's encoder to append commands after the Octara draw, such as a readback
    /// copy. Leave the encoder in the frame for submission.
    pub fn encoder(&mut self) -> &mut wgpu::CommandEncoder {
        &mut self.encoder
    }

    /// Finishes the encoder and submits the frame with its staged uploads.
    pub fn submit(self) -> wgpu::SubmissionIndex {
        self.queue.submit(self.before.into_iter().chain([self.encoder.finish()]))
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
        0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4,
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
    fn passes(&mut self, dl: &DisplayList, scale: f32, size: (u32, u32)) -> (Vec<Pass>, Vec<CanvasRequest>) {
        self.overflow = false;
        let passes = self.build_passes(dl, scale, size);
        if !self.overflow {
            return passes;
        }
        self.reset_atlases();
        let passes = self.build_passes(dl, scale, size);
        self.overflow = false;
        passes
    }

    fn build_passes(&mut self, dl: &DisplayList, scale: f32, size: (u32, u32)) -> (Vec<Pass>, Vec<CanvasRequest>) {
        let mut passes = vec![Pass::default()];
        let mut canvases = Vec::new();
        let mut open: Vec<usize> = vec![0];
        let mut clips: Vec<(f32, f32, f32, f32, f32)> = Vec::new();
        let window = Rect::new(0.0, 0.0, size.0 as f32, size.1 as f32);
        for item in &dl.items() {
            let (clip, clip_r) = clips.last().map_or((NO_CLIP, 0.0), |c| ([c.0, c.1, c.2, c.3], c.4));
            let current = *open.last().expect("a pass");
            let out = &mut passes[current].insts;
            match item {
                Item::Shadow(s) => out.push(shadow(s, scale, clip, clip_r)),
                Item::Quad(q) => out.push(quad(q, scale, clip, clip_r)),
                Item::Custom(paint) => {
                    let fallback = quad(&paint.fallback, scale, clip, clip_r);
                    if fallback.visible_bounds(&window).is_empty() {
                        continue;
                    }
                    let mut instance = custom_quad(paint, &fallback);
                    if let Some(&material) = paint.program.downcast_ref::<custom::Material>() {
                        if self.programs.prepare_material(&self.device, &self.pipeline_layout, self.format, material, paint.fallback.source)
                        {
                            passes[current].push_special(instance, Draw::Material(material));
                            continue;
                        }
                    } else if paint.program.downcast_ref::<custom::Canvas>().is_some()
                        && let Some(size) = custom::canvas_size(instance.rect, self.device.limits().max_texture_dimension_2d)
                    {
                        instance.params[1] = KIND_CANVAS;
                        let index = canvases.len();
                        canvases.push(CanvasRequest {
                            pass: current,
                            instance: out.len(),
                            program: paint.program.clone(),
                            fallback,
                            source: paint.fallback.source,
                            size,
                            params: paint.params,
                        });
                        passes[current].push_special(instance, Draw::Canvas(index));
                        continue;
                    }
                    out.push(fallback);
                }
                Item::Image(i) => {
                    if let Some(at) = self.image_slot(i) {
                        out.push(image(i, at, scale, clip, clip_r));
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
                            params: [0.0, if s.color { 2.0 } else { 1.0 }, clip_r, 0.0],
                            uv: [s.x as f32, s.y as f32, 0.0, 0.0],
                            grad_color: [0.0; 4],
                            grad: [0.0; 4],
                        });
                    }
                }
                Item::Path(p) => {
                    let window = paths::window(size, clips.last().map(|c| (c.0, c.1, c.2, c.3)));
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
                            params: [0.0, 6.0, clip_r, 0.0],
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
                    passes[current].push_special(quad(&hole, scale, clip, clip_r), Draw::Cutout);
                }
                // Otherwise its placeholder or snapshot, drawn before, shows.
                Item::Native { .. } => {}
                Item::PushClip { rect, radius, .. } => {
                    let outer = clips.last().map(|c| (c.0, c.1, c.2, c.3));
                    clips.push(push_clip(outer, *rect, *radius, scale));
                }
                Item::PopClip => {
                    clips.pop();
                }
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
                        Inst { fill: [0.0, 0.0, 0.0, opacity], clip: NO_CLIP, params: [0.0, KIND_LAYER, 0.0, 0.0], ..Inst::zeroed() },
                        Draw::Layer(layer),
                    );
                }
            }
        }
        (passes, canvases)
    }

    /// Records the display list into the given encoder, using `view` as a render attachment.
    ///
    /// The caller can record earlier commands into the encoder before passing it here. Preparation
    /// stages queue writes into the renderer's reusable buffers and atlases. These writes execute
    /// before all commands in the next queue submission, including any earlier encoder commands.
    /// Coordinate access to the shared queue so the returned frame is submitted or dropped before
    /// other work is staged or submitted. Finish any open debug groups first. Author canvases use
    /// separate command buffers between earlier commands and the draw, in the same submission.
    /// `size` is the view's extent in device pixels.
    pub fn record(
        &mut self,
        mut encoder: wgpu::CommandEncoder,
        dl: &DisplayList,
        scale: f32,
        view: &wgpu::TextureView,
        size: (u32, u32),
    ) -> RecordedFrame<'_> {
        let (frame, mut before) = self.prepare_frame(dl, scale, size);
        if !before.is_empty() {
            before.insert(0, encoder.finish());
            encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("octara drawing") });
        }
        self.encode_frame(&mut encoder, &frame, view);
        RecordedFrame { before, encoder, queue: &self.queue }
    }

    fn prepare_canvases(
        &mut self,
        passes: &mut [Pass],
        requests: Vec<CanvasRequest>,
        scale: f32,
    ) -> (Vec<wgpu::TextureView>, Vec<wgpu::CommandBuffer>) {
        self.programs.prune_canvases();
        if requests.is_empty() {
            self.programs.finish_canvases(0);
            return (Vec::new(), Vec::new());
        }
        let mut slots = Vec::with_capacity(requests.len());
        let mut views = Vec::with_capacity(requests.len());
        let mut commands = Vec::with_capacity(requests.len() + 1);
        // Preparation follows display-list order, including canvases inside child layers.
        for request in requests {
            let index = views.len();
            let product = self.programs.encode_canvas(&self.device, &request, scale, index);
            let slot = product.map(|(view, command)| {
                views.push(view);
                commands.push(command);
                index
            });
            if slot.is_none() {
                passes[request.pass].insts[request.instance] = request.fallback;
            }
            slots.push(slot);
        }
        self.programs.finish_canvases(views.len());
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
        (views, commands)
    }

    fn prepare_frame(&mut self, dl: &DisplayList, scale: f32, size: (u32, u32)) -> (PreparedFrame, Vec<wgpu::CommandBuffer>) {
        let (mut passes, canvases) = self.passes(dl, scale, size);
        let (canvas_views, before) = self.prepare_canvases(&mut passes, canvases, scale);
        let window = [size.0, size.1];
        let bounds = bound_passes(&mut passes, window);
        // One buffer holds every pass's instances.
        let mut all: Vec<Inst> = Vec::new();
        let mut instance_offsets = Vec::with_capacity(passes.len());
        for p in &passes {
            instance_offsets.push(all.len() as u32);
            all.extend_from_slice(&p.insts);
        }
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
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
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
        let layer_views: Vec<wgpu::TextureView> = self.layers.iter().map(|t| t.create_view(&Default::default())).collect();
        let mask_view = self.mask.create_view(&Default::default());
        let color_view = self.color.create_view(&Default::default());
        let image_view = self.images.create_view(&Default::default());
        let no_layer = self.no_layer.create_view(&Default::default());
        let bind_with = |layer: &wgpu::TextureView| {
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("octara"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &self.globals,
                            offset: 0,
                            size: wgpu::BufferSize::new(uniform_size as u64),
                        }),
                    },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&mask_view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&color_view) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&image_view) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(layer) },
                ],
            })
        };
        let bind = bind_with(&no_layer);
        let layer_binds: Vec<wgpu::BindGroup> = layer_views.iter().map(bind_with).collect();
        let canvas_binds = canvas_views.iter().map(bind_with).collect();
        let [r, g, b, a] = dl.background.components;
        let background = wgpu::Color { r: (r * a) as f64, g: (g * a) as f64, b: (b * a) as f64, a: a as f64 };
        (PreparedFrame { passes, instance_offsets, background, layer_views, bind, layer_binds, canvas_binds }, before)
    }

    fn encode_frame(&self, encoder: &mut wgpu::CommandEncoder, frame: &PreparedFrame, view: &wgpu::TextureView) {
        let (buffer, _) = self.instances.as_ref().expect("instance buffer");
        // Layers draw before the passes that composite them.
        for (i, p) in frame.passes.iter().enumerate().rev() {
            let (target, clear) = if i == 0 { (view, frame.background) } else { (&frame.layer_views[i - 1], wgpu::Color::TRANSPARENT) };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("octara"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            if p.insts.is_empty() {
                continue;
            }
            let projection = [u32::try_from(i * self.globals_stride).expect("pass uniform offset")];
            pass.set_vertex_buffer(0, buffer.slice(..));
            let base = frame.instance_offsets[i];
            let mut draw = |range: Range<usize>, special: Option<Draw>| {
                let (pipeline, bind) = match special {
                    None => (&self.pipeline, &frame.bind),
                    Some(Draw::Cutout) => (&self.cutout, &frame.bind),
                    Some(Draw::Layer(layer)) => (&self.pipeline, &frame.layer_binds[layer - 1]),
                    Some(Draw::Canvas(view)) => (&self.pipeline, &frame.canvas_binds[view]),
                    Some(Draw::Material(material)) => (self.programs.material_pipeline(material), &frame.bind),
                };
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, bind, &projection);
                pass.draw(0..6, base + range.start as u32..base + range.end as u32);
            };
            let mut from = 0;
            for batch in &p.special {
                if batch.instances.start > from {
                    draw(from..batch.instances.start, None);
                }
                draw(batch.instances.clone(), Some(batch.draw));
                from = batch.instances.end;
            }
            let end = p.insts.len();
            if end > from {
                draw(from..end, None);
            }
        }
    }

    /// Renders offscreen and reads the pixels back (tests, screenshots, a window's captures), in
    /// straight-alpha sRGB RGBA8, regardless of the renderer's texture format. Each axis has a
    /// one-pixel minimum.
    ///
    /// # Panics
    /// If the dimensions or scale are invalid, the capture format is unsupported, or GPU readback
    /// fails. [`Painter::paint`] returns an error in these cases.
    pub fn render(&mut self, dl: &DisplayList, scale: f32) -> Picture {
        self.try_render(dl, scale).expect("GPU rendering failed")
    }

    fn try_render(&mut self, dl: &DisplayList, scale: f32) -> Result<Picture, String> {
        let bgra = match self.format {
            wgpu::TextureFormat::Rgba8Unorm => false,
            wgpu::TextureFormat::Bgra8Unorm => true,
            format => return Err(format!("capture requires RGBA8 or BGRA8 (renderer format is {format:?})")),
        };
        let (width, height) = crate::pixels::dimensions(dl.size, scale)?;
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
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("octara readback"),
            size: layout.bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let view = target.create_view(&Default::default());
        let encoder = self.device.create_command_encoder(&Default::default());
        let mut frame = self.record(encoder, dl, scale, &view, (width, height));
        frame.encoder().copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(layout.row), rows_per_image: Some(height) },
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        let (mapped, mapping) = std::sync::mpsc::channel();
        frame.encoder().map_buffer_on_submit(&readback, wgpu::MapMode::Read, .., move |result| {
            let _ = mapped.send(result);
        });
        let submission = frame.submit();
        if let Some(error) = pollster::block_on(validation.pop()) {
            return Err(format!("capture GPU validation failed: {error}"));
        }
        self.device
            .poll(wgpu::PollType::Wait { submission_index: Some(submission), timeout: None })
            .map_err(|e| format!("capture GPU poll failed: {e}"))?;
        mapping
            .try_recv()
            .map_err(|e| format!("capture readback callback did not report completion: {e}"))?
            .map_err(|e| format!("capture readback mapping failed: {e}"))?;
        let data = readback.get_mapped_range(..).map_err(|e| format!("capture readback buffer access failed: {e}"))?;
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
        readback.unmap();
        Ok(Picture { width, height, rgba })
    }
}

fn custom_quad(paint: &CustomPaint, fallback: &Inst) -> Inst {
    Inst {
        rect: fallback.rect,
        fill: [0.0, 0.0, 0.0, paint.opacity],
        clip: fallback.clip,
        params: [fallback.params[0], KIND_QUAD, fallback.params[2], 0.0],
        uv: paint.params,
        ..Inst::zeroed()
    }
}

fn image(i: &ImagePaint, (ax, ay, w, h): (u32, u32, u32, u32), scale: f32, clip: [f32; 4], clip_r: f32) -> Inst {
    let (r, b) = (i.rect, i.bounds);
    Inst {
        rect: [r.x * scale, r.y * scale, r.w * scale, r.h * scale],
        fill: [0.0, 0.0, 0.0, i.opacity],
        border_color: [0.0; 4],
        border: [b.x * scale, b.y * scale, b.w * scale, b.h * scale],
        clip,
        params: [i.radius * scale, KIND_IMAGE, clip_r, 0.0],
        uv: [ax as f32, ay as f32, w as f32, h as f32],
        grad_color: [0.0; 4],
        grad: [0.0; 4],
    }
}

fn shadow(s: &Shadow, scale: f32, clip: [f32; 4], clip_r: f32) -> Inst {
    let r = s.rect;
    Inst {
        rect: [r.x * scale, r.y * scale, r.w * scale, r.h * scale],
        fill: straight(s.color),
        border_color: [0.0; 4],
        border: [s.blur * 0.5 * scale, 0.0, 0.0, 0.0],
        clip,
        params: [s.radius * scale, KIND_SHADOW, clip_r, 1.0],
        uv: [0.0; 4],
        grad_color: [0.0; 4],
        grad: [0.0; 4],
    }
}

fn quad(q: &Quad, scale: f32, clip: [f32; 4], clip_r: f32) -> Inst {
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
        params: [q.radius * scale, KIND_QUAD, clip_r, if filled { 1.0 } else { 0.0 }],
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
        self.try_render(list, scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use octara_primitives::identity::NodeId;
    use octara_primitives::paint::Part;
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

    #[test]
    fn author_programs_preserve_renderer_send() {
        fn assert_send<T: Send>() {}
        assert_send::<GpuRenderer>();
    }

    #[test]
    fn materials_batch_in_painter_order_with_per_instance_params() {
        let Some(mut gpu) = GpuRenderer::new() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        const SHADER: &str = "fn material(uv: vec2f, params: vec4f) -> vec4f { return params; }";
        let custom =
            |rect: Rect, program, params| Item::Custom(CustomPaint { fallback: solid(rect, rgb(0xff00ff)), program, params, opacity: 1.0 });
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
        let (passes, canvases) = gpu.passes(&dl, 1.0, (32, 24));
        assert!(canvases.is_empty());
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
                        drop(frame.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
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
                        }));
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
    fn nested_layers_bound_their_painted_pixels_instead_of_the_window() {
        let window = [1600, 1200];
        let composite = Inst { params: [0.0, KIND_LAYER, 0.0, 0.0], clip: NO_CLIP, ..Inst::zeroed() };
        let quad = Inst { rect: [370.5, 460.25, 28.0, 18.0], clip: NO_CLIP, ..Inst::zeroed() };
        let shadow = Inst {
            rect: [400.25, 480.5, 21.0, 12.0],
            clip: [404.0, 480.0, 10.0, 10.0],
            border: [2.0, 0.0, 0.0, 0.0],
            params: [0.0, KIND_SHADOW, 0.0, 0.0],
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
    fn atlas_refuses_oversized_images_without_wrapping_or_moving_the_shelf() {
        let mut shelf = Shelf::new(ATLAS);
        assert_eq!(shelf.alloc(1, 1), Some((0, 0)));
        assert_eq!(shelf.alloc(u32::MAX, 1), None);
        assert_eq!(shelf.alloc(1, u32::MAX), None);
        assert_eq!(shelf.alloc(1, 1), Some((2, 0)));
    }
}

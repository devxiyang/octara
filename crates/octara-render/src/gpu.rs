//! The GPU renderer: wgpu, one WGSL shader (`shader.wgsl`), one instance per display-list item,
//! one draw call per frame. Glyphs and paths are rasterized as by the reference renderer
//! ([`GlyphCache`], [`paths`]) and packed into an alpha atlas (masks; a path larger than a tile
//! takes several) and an RGBA atlas (color glyphs). When an atlas fills up, every atlas starts
//! over and the frame is built again with only what it draws.

use crate::Bitmap;
use crate::glyphs::{GlyphCache, SUBPIXEL_STEPS};
use crate::paths::{self, MaskKey};
use crate::shape::push_clip;
use bytemuck::{Pod, Zeroable};
use octara::kernel::Color;
use octara::paint::{DisplayList, GlyphRun, ImagePaint, Item, PathPaint, Quad, Shadow};
use rustc_hash::FxHashMap;

const ATLAS: u32 = 2048;
/// The mask atlas's size where the device allows it (paths can be window-sized).
const MASK_ATLAS: u32 = 4096;
/// The largest piece of a path's mask in the atlas.
const TILE: u32 = 1024;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// No clip: a rect far larger than any window.
const NO_CLIP: [f32; 4] = [-1.0e6, -1.0e6, 2.0e6, 2.0e6];

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Inst {
    rect: [f32; 4],
    fill: [f32; 4],
    border_color: [f32; 4],
    border: [f32; 4],
    clip: [f32; 4],
    /// radius, kind (0 quad, 1 mask glyph, 2 color glyph, 3 shadow, 4 image, 5 layer, 6 path),
    /// clip radius, has fill (a shadow's σ is `border[0]`; an image's clipping box is `border`, its
    /// radius `params[0]`, its opacity `fill[3]`, its atlas origin and size `uv`; a path's
    /// gradient box is `border`)
    params: [f32; 4],
    uv: [f32; 4],
    /// A gradient's end color (straight); `fill` is its start.
    grad_color: [f32; 4],
    /// A gradient's angle (degrees), start and end fractions, and 1 if there is one.
    grad: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    size: [f32; 2],
    _pad: [f32; 2],
}

/// The instances drawn into one target: the window, or a layer composited at `opacity`.
#[derive(Default)]
struct Pass {
    insts: Vec<Inst>,
    /// Instances that composite a layer: (index in `insts`, the layer's pass).
    composites: Vec<(usize, usize)>,
    opacity: f32,
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
        if self.x + w > self.size {
            self.x = 0;
            self.y += self.row;
            self.row = 0;
        }
        if self.y + h > self.size || w > self.size {
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

/// Renders display lists with the GPU.
pub struct GpuRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    globals: wgpu::Buffer,
    mask: wgpu::Texture,
    color: wgpu::Texture,
    images: wgpu::Texture,
    /// Layer targets, reused across frames (window-sized, the target's format).
    layers: Vec<wgpu::Texture>,
    /// A 1×1 texture bound where no layer is.
    no_layer: wgpu::Texture,
    /// Images in the image atlas: id → (version, x, y).
    image_slots: FxHashMap<u64, (u64, u32, u32)>,
    shelves: [Shelf; 3],
    slots: FxHashMap<AtlasKey, Option<Slot>>,
    path_slots: FxHashMap<MaskKey, Vec<Tile>>,
    /// An atlas ran out of room while building this frame.
    overflow: bool,
    glyphs: GlyphCache,
    instances: Option<(wgpu::Buffer, u64)>,
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
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
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
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
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
        let attributes = wgpu::vertex_attr_array![
            0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4,
            7 => Float32x4, 8 => Float32x4
        ];
        let premultiplied = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("octara"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
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
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState { color: premultiplied, alpha: premultiplied }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("octara globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
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
            layout,
            globals,
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
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d { x, y, z: 0 }, aspect: wgpu::TextureAspect::All },
            data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(stride * bpp), rows_per_image: Some(h) },
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
                let start = (dy * mask.width + dx) as usize;
                let end = start + ((h - 1) * mask.width + w) as usize;
                self.upload(&self.mask, (ax, ay), (w, h), 1, &mask.data[start..end], mask.width);
                tiles.push(Tile { ax, ay, dx, dy, w, h });
            }
        }
        self.path_slots.insert(at.key, tiles.clone());
        Some(tiles)
    }

    /// The image's place in the image atlas, uploading its current pixels when they are new; an
    /// image larger than the atlas is not drawn.
    fn image_slot(&mut self, i: &ImagePaint) -> Option<(u32, u32, u32, u32)> {
        let f = i.image.frame();
        if f.width == 0 || f.height == 0 {
            return None;
        }
        if let Some((v, x, y)) = self.image_slots.get(&i.image.id())
            && *v == f.version
        {
            return Some((*x, *y, f.width, f.height));
        }
        let Some(at) = self.shelves[2].alloc(f.width, f.height) else {
            self.overflow = true;
            return None;
        };
        self.upload(&self.images, at, (f.width, f.height), 4, &f.rgba, f.width);
        self.image_slots.insert(i.image.id(), (f.version, at.0, at.1));
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
    fn passes(&mut self, dl: &DisplayList, scale: f32, size: (u32, u32)) -> Vec<Pass> {
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

    fn build_passes(&mut self, dl: &DisplayList, scale: f32, size: (u32, u32)) -> Vec<Pass> {
        let mut passes = vec![Pass::default()];
        let mut open: Vec<usize> = vec![0];
        let mut clips: Vec<(f32, f32, f32, f32, f32)> = Vec::new();
        for item in &dl.items {
            let (clip, clip_r) = clips.last().map_or((NO_CLIP, 0.0), |c| ([c.0, c.1, c.2, c.3], c.4));
            let current = *open.last().expect("a pass");
            let out = &mut passes[current].insts;
            match item {
                Item::Shadow(s) => out.push(shadow(s, scale, clip, clip_r)),
                Item::Quad(q) => out.push(quad(q, scale, clip, clip_r)),
                Item::Image(i) => {
                    if let Some(at) = self.image_slot(i) {
                        out.push(image(i, at, scale, clip, clip_r));
                    }
                }
                Item::Glyphs(run) => {
                    let size = run.size * scale;
                    for g in &run.glyphs {
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
                // Platform views are the shell's (an underlay's cutout comes with the shell).
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
                    let at = passes[parent].insts.len();
                    let opacity = passes[layer].opacity;
                    // The layer covers the target; what clips it was applied inside.
                    passes[parent].insts.push(Inst {
                        rect: [0.0, 0.0, size.0 as f32, size.1 as f32],
                        fill: [0.0, 0.0, 0.0, opacity],
                        border_color: [0.0; 4],
                        border: [0.0; 4],
                        clip: NO_CLIP,
                        params: [0.0, 5.0, 0.0, 0.0],
                        uv: [0.0; 4],
                        grad_color: [0.0; 4],
                        grad: [0.0; 4],
                    });
                    passes[parent].composites.push((at, layer));
                }
            }
        }
        passes
    }

    /// Records the draw of `dl` into `view` (a window's surface texture or an offscreen target).
    /// Layers draw first, each into a texture of its own, then composite into their parents.
    pub fn draw(&mut self, dl: &DisplayList, scale: f32, view: &wgpu::TextureView, size: (u32, u32)) {
        let passes = self.passes(dl, scale, size);
        // One buffer holds every pass's instances.
        let mut all: Vec<Inst> = Vec::new();
        let mut offsets = Vec::with_capacity(passes.len());
        for p in &passes {
            offsets.push(all.len() as u32);
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
        let (buffer, _) = self.instances.as_ref().expect("instance buffer");
        if !bytes.is_empty() {
            self.queue.write_buffer(buffer, 0, bytes);
        }
        let globals = Globals { size: [size.0 as f32, size.1 as f32], _pad: [0.0; 2] };
        self.queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));
        // Layer targets, window-sized.
        let layer_count = passes.len() - 1;
        self.layers.retain(|t| t.width() == size.0 && t.height() == size.1);
        while self.layers.len() < layer_count {
            self.layers.push(self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("octara layer"),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            }));
        }
        let layer_views: Vec<wgpu::TextureView> = self.layers[..layer_count].iter().map(|t| t.create_view(&Default::default())).collect();
        let mask_view = self.mask.create_view(&Default::default());
        let color_view = self.color.create_view(&Default::default());
        let image_view = self.images.create_view(&Default::default());
        let no_layer = self.no_layer.create_view(&Default::default());
        let bind_with = |layer: &wgpu::TextureView| {
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("octara"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.globals.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&mask_view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&color_view) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&image_view) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(layer) },
                ],
            })
        };
        let bind = bind_with(&no_layer);
        let layer_binds: Vec<wgpu::BindGroup> = layer_views.iter().map(bind_with).collect();
        let [r, g, b, a] = dl.background.components;
        let background = wgpu::Color { r: (r * a) as f64, g: (g * a) as f64, b: (b * a) as f64, a: a as f64 };
        let mut encoder = self.device.create_command_encoder(&Default::default());
        for (i, p) in passes.iter().enumerate().rev() {
            let (target, clear) = if i == 0 { (view, background) } else { (&layer_views[i - 1], wgpu::Color::TRANSPARENT) };
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
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.set_vertex_buffer(0, buffer.slice(..));
            let base = offsets[i];
            let mut from = 0u32;
            for &(at, layer) in &p.composites {
                let at = at as u32;
                if at > from {
                    pass.draw(0..6, base + from..base + at);
                }
                pass.set_bind_group(0, &layer_binds[layer - 1], &[]);
                pass.draw(0..6, base + at..base + at + 1);
                pass.set_bind_group(0, &bind, &[]);
                from = at + 1;
            }
            let end = p.insts.len() as u32;
            if end > from {
                pass.draw(0..6, base + from..base + end);
            }
        }
        self.queue.submit([encoder.finish()]);
    }

    /// Renders offscreen and reads the pixels back (tests, screenshots, a window's captures), in
    /// the renderer's format (RGBA or BGRA, 8 bits).
    pub fn render(&mut self, dl: &DisplayList, scale: f32) -> Bitmap {
        let bgra = match self.format {
            wgpu::TextureFormat::Rgba8Unorm => false,
            wgpu::TextureFormat::Bgra8Unorm => true,
            f => panic!("offscreen rendering reads RGBA8 or BGRA8, not {f:?}"),
        };
        let width = (dl.size.0 * scale).ceil().max(1.0) as u32;
        let height = (dl.size.1 * scale).ceil().max(1.0) as u32;
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
        self.draw(dl, scale, &target.create_view(&Default::default()), (width, height));
        let row = (width * 4).div_ceil(256) * 256;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("octara readback"),
            size: (row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(height) },
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);
        readback.map_async(wgpu::MapMode::Read, .., |r| r.expect("map readback"));
        self.device.poll(wgpu::PollType::wait_indefinitely()).expect("gpu poll");
        let data = readback.get_mapped_range(..).expect("mapped readback");
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            let line = &data[(y * row) as usize..(y * row + width * 4) as usize];
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
        Bitmap { width, height, rgba }
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
        params: [i.radius * scale, 4.0, clip_r, 0.0],
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
        params: [s.radius * scale, 3.0, clip_r, 1.0],
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
        params: [q.radius * scale, 0.0, clip_r, if filled { 1.0 } else { 0.0 }],
        uv: [0.0; 4],
        grad_color,
        grad,
    }
}

impl octara::capture::Painter for GpuRenderer {
    fn renderer(&self) -> &'static str {
        "gpu"
    }
    fn paint(&mut self, list: &DisplayList, scale: f32) -> Result<octara::capture::Picture, String> {
        let b = self.render(list, scale);
        Ok(octara::capture::Picture { width: b.width, height: b.height, rgba: b.rgba })
    }
}

//! Vertex-pulled glyphs, shared by mask and color atlas pages.

use super::atlas::Region;
use super::pipeline::{clips_entry, globals_entry, texture_entry};
use bytemuck::{Pod, Zeroable};
use rustc_hash::FxHashMap;

// Keep the high bit in sync with glyphs.wgsl.
const COLOR_GLYPH: u32 = 1 << 31;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct GlyphInst {
    pos: [f32; 2],
    size: u32,
    atlas_origin: u32,
    /// Straight sRGB, with alpha in the high byte.
    color_rgba8: u32,
    clip_id_flags: u32,
}

const _: () = assert!(size_of::<GlyphInst>() == 24);

impl GlyphInst {
    pub(super) fn new(pos: [f32; 2], region: Region, color: u32, clip: u32, color_glyph: bool) -> Self {
        let pack = |[x, y]: [u32; 2]| {
            debug_assert!(x <= u16::MAX.into() && y <= u16::MAX.into());
            x | (y << 16)
        };
        assert!(clip < COLOR_GLYPH, "glyph clip node index");
        Self {
            pos,
            size: pack(region.size),
            atlas_origin: pack(region.origin),
            color_rgba8: color,
            clip_id_flags: clip | if color_glyph { COLOR_GLYPH } else { 0 },
        }
    }
}

pub(super) struct GlyphPipeline {
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    pub(super) layout: wgpu::BindGroupLayout,
    pub(super) formats: FxHashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
}

impl GlyphPipeline {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("octara glyphs"),
            source: wgpu::ShaderSource::Wgsl(concat!(include_str!("shared.wgsl"), include_str!("glyphs.wgsl")).into()),
        });
        let glyphs_entry = wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(size_of::<GlyphInst>() as u64),
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("octara glyphs"),
            entries: &[globals_entry(), texture_entry(1), glyphs_entry, clips_entry()],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("octara glyphs"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        Self { shader, pipeline_layout, layout, formats: FxHashMap::default() }
    }

    pub(super) fn prepare(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        self.formats.entry(format).or_insert_with(|| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("octara glyphs"),
                layout: Some(&self.pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &self.shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &self.shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        });
    }
}

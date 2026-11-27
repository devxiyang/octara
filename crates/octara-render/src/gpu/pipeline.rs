//! The four built-in draw pipelines and the author-material extension.

use super::glyph_pipeline::GlyphInst;
use super::instances::{DrawInst, GlyphClip, GlyphDraw, GradientInst, ImageInst, MaterialInst, QuadInst, ShadowInst};
use crate::shape::ClipNode;
use bytemuck::{Pod, Zeroable};
use rustc_hash::FxHashMap;

pub(super) const SHARED_SHADER: &str = include_str!("shared.wgsl");
pub(super) const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct Globals {
    pub(super) size: [f32; 2],
    pub(super) origin: [f32; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PipelineKind {
    Quad,
    Shadow,
    Image,
    Glyph,
    Material,
}

impl PipelineKind {
    fn label(self) -> &'static str {
        match self {
            Self::Quad => "octara quad",
            Self::Shadow => "octara shadow",
            Self::Image => "octara image",
            Self::Glyph => "octara glyph",
            Self::Material => "octara material",
        }
    }

    fn shader(self) -> &'static str {
        match self {
            Self::Quad => concat!(include_str!("shared.wgsl"), include_str!("gradient.wgsl"), include_str!("quad.wgsl")),
            Self::Shadow => concat!(include_str!("shared.wgsl"), include_str!("shadow.wgsl")),
            Self::Image => concat!(include_str!("shared.wgsl"), include_str!("gradient.wgsl"), include_str!("image.wgsl")),
            Self::Glyph => concat!(include_str!("shared.wgsl"), include_str!("glyphs.wgsl")),
            Self::Material => unreachable!("materials supply author code"),
        }
    }

    fn payload_size(self) -> usize {
        match self {
            Self::Quad => size_of::<QuadInst>(),
            Self::Shadow => size_of::<ShadowInst>(),
            Self::Image => size_of::<ImageInst>(),
            Self::Glyph => size_of::<GlyphInst>(),
            Self::Material => size_of::<MaterialInst>(),
        }
    }
}

fn buffer_entry(binding: u32, size: usize, visibility: wgpu::ShaderStages, uniform: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: if uniform { wgpu::BufferBindingType::Uniform } else { wgpu::BufferBindingType::Storage { read_only: true } },
            has_dynamic_offset: uniform,
            min_binding_size: wgpu::BufferSize::new(size as u64),
        },
        count: None,
    }
}

fn texture_entry(binding: u32, view_dimension: wgpu::TextureViewDimension, filterable: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable }, view_dimension, multisampled: false },
        count: None,
    }
}

pub(super) struct DrawLayout {
    pub(super) bind: wgpu::BindGroupLayout,
    pub(super) pipeline: wgpu::PipelineLayout,
}

impl DrawLayout {
    fn new(device: &wgpu::Device, kind: PipelineKind) -> Self {
        use wgpu::{ShaderStages as Stages, TextureViewDimension as Dim};
        let mut entries = vec![
            buffer_entry(0, size_of::<Globals>(), Stages::VERTEX_FRAGMENT, true),
            buffer_entry(
                2,
                kind.payload_size(),
                if kind == PipelineKind::Glyph { Stages::VERTEX_FRAGMENT } else { Stages::FRAGMENT },
                false,
            ),
            buffer_entry(5, size_of::<ClipNode>(), Stages::VERTEX_FRAGMENT, false),
            buffer_entry(
                6,
                if kind == PipelineKind::Glyph { size_of::<GlyphDraw>() } else { size_of::<DrawInst>() },
                Stages::VERTEX_FRAGMENT,
                false,
            ),
            buffer_entry(8, size_of::<[f32; 4]>(), Stages::VERTEX_FRAGMENT, false),
        ];
        if matches!(kind, PipelineKind::Quad | PipelineKind::Image) {
            entries.push(buffer_entry(7, size_of::<GradientInst>(), Stages::FRAGMENT, false));
        }
        if kind == PipelineKind::Glyph {
            entries.push(buffer_entry(7, size_of::<GlyphClip>(), Stages::VERTEX_FRAGMENT, false));
        }
        if matches!(kind, PipelineKind::Glyph | PipelineKind::Image) {
            entries.push(texture_entry(1, Dim::D2Array, kind == PipelineKind::Image));
        }
        if kind == PipelineKind::Image {
            entries.push(texture_entry(4, Dim::D2, true));
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: Stages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            });
        }
        let bind = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(kind.label()), entries: &entries });
        let pipeline = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(kind.label()),
            bind_group_layouts: &[Some(&bind)],
            immediate_size: 0,
        });
        Self { bind, pipeline }
    }
}

#[derive(Clone, Copy)]
pub(super) enum Phase {
    Blend,
    Core,
    Cutout,
    Material,
}

pub(super) fn render_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    shader: &wgpu::ShaderModule,
    label: &str,
    phase: Phase,
    fragment_clipping: bool,
) -> wgpu::RenderPipeline {
    let core = matches!(phase, Phase::Core);
    let vertex = if core { "vs_core" } else { "vs" };
    let fragment = match phase {
        Phase::Core => "fs_core",
        Phase::Material => "fs_material",
        _ => "fs",
    };
    let cutout = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Zero,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    };
    let blend = match phase {
        Phase::Core => None,
        Phase::Cutout => Some(wgpu::BlendState { color: cutout, alpha: cutout }),
        _ => Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
    };
    let constants: &[(&str, f64)] = if fragment_clipping || core { &[] } else { &[("FRAGMENT_CLIPPING", 0.0)] };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState { module: shader, entry_point: Some(vertex), compilation_options: Default::default(), buffers: &[] },
        primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(core),
            depth_compare: Some(if core { wgpu::CompareFunction::Greater } else { wgpu::CompareFunction::GreaterEqual }),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment),
            compilation_options: wgpu::PipelineCompilationOptions { constants, ..Default::default() },
            targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

pub(super) fn color_texture(
    device: &wgpu::Device,
    label: &'static str,
    format: wgpu::TextureFormat,
    [width, height]: [u32; 2],
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

pub(super) fn depth_texture(device: &wgpu::Device, [width, height]: [u32; 2]) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("octara painter depth"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

pub(super) struct DrawPipelines {
    pub(super) quad: wgpu::RenderPipeline,
    pub(super) quad_core: wgpu::RenderPipeline,
    pub(super) cutout: wgpu::RenderPipeline,
    pub(super) shadow: wgpu::RenderPipeline,
    pub(super) image: wgpu::RenderPipeline,
    pub(super) image_core: wgpu::RenderPipeline,
    pub(super) glyph: wgpu::RenderPipeline,
}

struct MicroPipeline {
    shader: wgpu::ShaderModule,
    layout: DrawLayout,
    kind: PipelineKind,
}

impl MicroPipeline {
    fn new(device: &wgpu::Device, kind: PipelineKind) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(kind.label()),
            source: wgpu::ShaderSource::Wgsl(kind.shader().into()),
        });
        Self { shader, layout: DrawLayout::new(device, kind), kind }
    }

    fn render(&self, device: &wgpu::Device, format: wgpu::TextureFormat, phase: Phase, clipping: bool) -> wgpu::RenderPipeline {
        render_pipeline(device, &self.layout.pipeline, format, &self.shader, self.kind.label(), phase, clipping)
    }
}

pub(super) struct PipelineCache {
    quad: MicroPipeline,
    shadow: MicroPipeline,
    image: MicroPipeline,
    glyph: MicroPipeline,
    pub(super) material: DrawLayout,
    pub(super) formats: FxHashMap<(wgpu::TextureFormat, bool), DrawPipelines>,
}

impl PipelineCache {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        Self {
            quad: MicroPipeline::new(device, PipelineKind::Quad),
            shadow: MicroPipeline::new(device, PipelineKind::Shadow),
            image: MicroPipeline::new(device, PipelineKind::Image),
            glyph: MicroPipeline::new(device, PipelineKind::Glyph),
            material: DrawLayout::new(device, PipelineKind::Material),
            formats: FxHashMap::default(),
        }
    }

    pub(super) fn layout(&self, kind: PipelineKind) -> &wgpu::BindGroupLayout {
        &match kind {
            PipelineKind::Quad => &self.quad.layout,
            PipelineKind::Shadow => &self.shadow.layout,
            PipelineKind::Image => &self.image.layout,
            PipelineKind::Glyph => &self.glyph.layout,
            PipelineKind::Material => &self.material,
        }
        .bind
    }

    pub(super) fn prepare(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat, clipping: bool) {
        if self.formats.contains_key(&(format, clipping)) {
            return;
        }
        let (quad_core, image_core) = self.formats.get(&(format, !clipping)).map_or_else(
            || (self.quad.render(device, format, Phase::Core, false), self.image.render(device, format, Phase::Core, false)),
            |draw| (draw.quad_core.clone(), draw.image_core.clone()),
        );
        self.formats.insert(
            (format, clipping),
            DrawPipelines {
                quad: self.quad.render(device, format, Phase::Blend, clipping),
                quad_core,
                cutout: self.quad.render(device, format, Phase::Cutout, clipping),
                shadow: self.shadow.render(device, format, Phase::Blend, clipping),
                image: self.image.render(device, format, Phase::Blend, clipping),
                image_core,
                glyph: self.glyph.render(device, format, Phase::Blend, clipping),
            },
        );
    }
}

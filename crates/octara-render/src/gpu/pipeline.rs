//! Instance pipelines and bind group layouts shared by all targets on a device.

use super::batch::Inst;
use crate::shape::ClipNode;
use bytemuck::{Pod, Zeroable};
use rustc_hash::FxHashMap;

pub(super) const GEOMETRY_SHADER: &str = concat!(include_str!("shared.wgsl"), include_str!("geometry.wgsl"));

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct Globals {
    pub(super) size: [f32; 2],
    pub(super) origin: [f32; 2],
}

pub(super) fn globals_entry() -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: true,
            min_binding_size: wgpu::BufferSize::new(size_of::<Globals>() as u64),
        },
        count: None,
    }
}

pub(super) fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

pub(super) fn clips_entry() -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: 5,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: wgpu::BufferSize::new(size_of::<ClipNode>() as u64),
        },
        count: None,
    }
}

pub(super) fn render_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    shader: &wgpu::ShaderModule,
    fragment: &str,
    label: &str,
    blend: wgpu::BlendState,
) -> wgpu::RenderPipeline {
    let attributes = wgpu::vertex_attr_array![
        0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Uint32x3, 6 => Float32x4,
        7 => Float32x4, 8 => Float32x4, 9 => Float32x4
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

pub(super) struct DrawPipelines {
    pub(super) pipeline: wgpu::RenderPipeline,
    /// The same shader blending destination-out: clears toward transparent by coverage.
    pub(super) cutout: wgpu::RenderPipeline,
}

impl DrawPipelines {
    fn new(device: &wgpu::Device, layout: &wgpu::PipelineLayout, shader: &wgpu::ShaderModule, format: wgpu::TextureFormat) -> Self {
        let cutout_blend = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        Self {
            pipeline: render_pipeline(device, layout, format, shader, "fs", "octara", wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            cutout: render_pipeline(
                device,
                layout,
                format,
                shader,
                "fs",
                "octara cutout",
                wgpu::BlendState { color: cutout_blend, alpha: cutout_blend },
            ),
        }
    }
}

pub(super) struct PipelineCache {
    shader: wgpu::ShaderModule,
    pub(super) formats: FxHashMap<wgpu::TextureFormat, DrawPipelines>,
    pub(super) layout: wgpu::BindGroupLayout,
    pub(super) pipeline_layout: wgpu::PipelineLayout,
}

impl PipelineCache {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("octara"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(include_str!("shared.wgsl"), include_str!("geometry.wgsl"), include_str!("../shader.wgsl")).into(),
            ),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("octara"),
            entries: &[globals_entry(), texture_entry(1), texture_entry(3), texture_entry(4), clips_entry()],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("octara"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        Self { shader, formats: FxHashMap::default(), layout, pipeline_layout }
    }

    pub(super) fn prepare(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        self.formats.entry(format).or_insert_with(|| DrawPipelines::new(device, &self.pipeline_layout, &self.shader, format));
    }
}

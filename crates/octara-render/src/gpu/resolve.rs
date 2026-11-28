//! Output attachment selection and conversion from linear-light composition.

use super::encode::color_pass;
use super::pipeline::color_texture;
use super::{OutputColor, WORKING_FORMAT};
use crate::plan::TargetInfo;
use rustc_hash::FxHashMap;

/// A recording's final attachment and its consumer's color representation.
#[derive(Clone, Copy)]
pub struct FrameTarget<'a> {
    pub texture: &'a wgpu::Texture,
    pub color: OutputColor,
}

#[derive(Clone, Copy)]
pub(super) struct AttachmentInfo {
    pub(super) format: wgpu::TextureFormat,
    pub(super) requirements: TargetInfo,
}

impl AttachmentInfo {
    pub(super) fn for_target(target: FrameTarget<'_>, scale: f32, max_texture_dimension: u32) -> Self {
        use wgpu::TextureFormat::{Bgra8Unorm, Rgba8Unorm};

        let (texture, color) = (target.texture, target.color);
        let format = texture.format();
        let encode_output = match color {
            OutputColor::Linear => {
                assert_eq!(format, WORKING_FORMAT, "linear output requires Rgba16Float");
                false
            }
            _ => {
                assert!(matches!(format.remove_srgb_suffix(), Bgra8Unorm | Rgba8Unorm), "sRGB output requires RGBA8 or BGRA8");
                color != OutputColor::SrgbOpaque || !format.is_srgb()
            }
        };
        Self {
            format: if encode_output { WORKING_FORMAT } else { format },
            requirements: TargetInfo {
                extent: [texture.width(), texture.height()],
                scale,
                max_texture_dimension,
                copyable: texture.usage().contains(wgpu::TextureUsages::COPY_SRC),
                encode_output,
            },
        }
    }
}

/// An attachment that can be read during scene drawing, plus the pass that transfers it to the output.
#[derive(Default)]
pub(super) struct OutputResolve {
    source: Option<Source>,
    pipelines: FxHashMap<(wgpu::TextureFormat, OutputColor), Blit>,
}

struct Source {
    view: wgpu::TextureView,
    bind: wgpu::BindGroup,
    output: (wgpu::TextureFormat, OutputColor),
}

impl OutputResolve {
    pub(super) fn prepare(
        &mut self,
        device: &wgpu::Device,
        attachment: Option<AttachmentInfo>,
        target: FrameTarget<'_>,
    ) -> Option<wgpu::TextureView> {
        let Some(attachment) = attachment else {
            self.release_source();
            return None;
        };
        let format = attachment.format;
        let [width, height] = attachment.requirements.extent;
        let output = (target.texture.format(), target.color);
        if let Some(source) = &self.source {
            let texture = source.view.texture();
            if source.output == output && texture.format() == format && texture.width() == width && texture.height() == height {
                return Some(source.view.clone());
            }
        }
        let pipeline = self.pipelines.entry(output).or_insert_with(|| Blit::new(device, output.0, output.1));
        let texture = color_texture(device, "octara readable output", format, [width, height]);
        let view = texture.create_view(&Default::default());
        let bind = pipeline.bind(device, &view);
        self.source = Some(Source { view: view.clone(), bind, output });
        Some(view)
    }

    pub(super) fn release_source(&mut self) {
        self.source = None;
    }

    pub(super) fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>,
    ) {
        let source = self.source.as_ref().expect("the frame planned an output resolve");
        self.pipelines[&source.output].encode(encoder, &source.bind, target, timestamps);
    }
}

pub(super) struct Blit(wgpu::RenderPipeline);

impl Blit {
    pub(super) fn new(device: &wgpu::Device, format: wgpu::TextureFormat, color: OutputColor) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("octara output"),
            source: wgpu::ShaderSource::Wgsl(include_str!("resolve.wgsl").into()),
        });
        Self(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("octara output"),
            layout: None,
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &[("output_color", f64::from(color as u8)), ("srgb_attachment", f64::from(u8::from(format.is_srgb())))],
                    ..Default::default()
                },
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        }))
    }

    pub(super) fn bind(&self, device: &wgpu::Device, source: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("octara output"),
            layout: &self.0.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(source) }],
        })
    }

    pub(super) fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::BindGroup,
        target: &wgpu::TextureView,
        timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>,
    ) {
        let mut pass = color_pass(encoder, target, wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), timestamps);
        pass.set_pipeline(&self.0);
        pass.set_bind_group(0, source, &[]);
        pass.draw(0..3, 0..1);
    }
}

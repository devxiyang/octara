//! Submitted scene pixels and the transfer to each newly acquired frame target.

use super::damage::{DamageRect, SceneSnapshot};
use super::pipeline::{DEPTH_FORMAT, color_texture};
use super::resolve::{AttachmentInfo, Blit};
use super::retained::RetainedTarget;
use super::{FrameTarget, OutputColor};
use rustc_hash::FxHashMap;
use wgpu::util::DeviceExt;

#[derive(Clone, Copy)]
pub(super) enum Raster {
    Full,
    Damage(DamageRect),
    Reuse,
}

#[derive(Clone)]
pub(super) struct Backing {
    pub(super) target: RetainedTarget,
    pub(super) scene: SceneSnapshot,
    pub(super) spatial: bool,
    pub(super) view: wgpu::TextureView,
    color: OutputColor,
    pub(super) clear: Option<Clear>,
    blit: Option<wgpu::BindGroup>,
}

#[derive(Default)]
pub(super) struct BackingPipelines {
    blits: FxHashMap<(wgpu::TextureFormat, OutputColor), Blit>,
    clears: FxHashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
}

impl BackingPipelines {
    pub(super) fn prepare(
        &mut self,
        device: &wgpu::Device,
        attachment: AttachmentInfo,
        target: FrameTarget<'_>,
        scene: SceneSnapshot,
        previous: Option<&Backing>,
        spatial: bool,
    ) -> Backing {
        let view = previous.map(|backing| backing.view.clone()).unwrap_or_else(|| {
            color_texture(device, "octara persistent backing", attachment.format, attachment.requirements.extent)
                .create_view(&Default::default())
        });
        let blit = if attachment.format == target.texture.format()
            && !attachment.requirements.encode_output
            && target.texture.usage().contains(wgpu::TextureUsages::COPY_DST)
        {
            None
        } else if let Some(bind) = previous.and_then(|backing| backing.blit.clone()) {
            Some(bind)
        } else {
            let key = (target.texture.format(), target.color);
            let pipeline = self.blits.entry(key).or_insert_with(|| Blit::new(device, key.0, key.1));
            Some(pipeline.bind(device, &view))
        };
        Backing {
            target: target.into(),
            scene,
            spatial,
            view,
            blit,
            color: target.color,
            clear: previous.and_then(|backing| backing.clear.clone()),
        }
    }

    pub(super) fn transfer(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        backing: &Backing,
        output: &wgpu::TextureView,
        recording: &mut super::recording::FrameRecording<'_>,
    ) {
        let (source, target) = (backing.view.texture(), output.texture());
        if let Some(bind) = &backing.blit {
            let timestamps = recording.pass(super::GpuPassKind::Output);
            self.blits[&(target.format(), backing.color)].encode(encoder, bind, output, timestamps);
        } else {
            encoder.copy_texture_to_texture(source.as_image_copy(), target.as_image_copy(), target.size());
        }
        if let Some(measurement) = recording.measurement.as_deref_mut() {
            measurement.profile.output_pixels = u64::from(target.width()) * u64::from(target.height());
        }
    }

    pub(super) fn prepare_clear(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat, background: wgpu::Color) -> Clear {
        let pipeline = self.clears.entry(format).or_insert_with(|| clear_pipeline(device, format));
        let mut color = [background.r as f32, background.g as f32, background.b as f32, background.a as f32];
        if format == super::WORKING_FORMAT {
            // Match the attachment clear's stored precision before blending over this color.
            color = color.map(|channel| half::f16::from_f32(channel).to_f32());
        }
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("octara damage background"),
            contents: bytemuck::bytes_of(&color),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("octara damage background"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
        });
        Clear { pipeline: pipeline.clone(), bind }
    }
}

#[derive(Clone)]
pub(super) struct Clear {
    pipeline: wgpu::RenderPipeline,
    bind: wgpu::BindGroup,
}

impl Clear {
    pub(super) fn encode(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn clear_pipeline(device: &wgpu::Device, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("octara damage clear"),
        source: wgpu::ShaderSource::Wgsl(include_str!("clear.wgsl").into()),
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("octara damage clear"),
        layout: None,
        vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
        }),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        primitive: Default::default(),
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

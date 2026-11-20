use octara_primitives::paint::PaintProgram;
use octara_render::{
    gpu::{Backdrop, CanvasFrame, backdrop},
    wgpu,
};
use wgpu::util::DeviceExt;

pub(super) fn program() -> PaintProgram {
    let validation_probe = std::env::var_os("OCTARA_GPU_LAB_PROBE_FALLBACK").is_some_and(|value| value == "1");
    backdrop(move |device| {
        let probe = validation_probe.then(|| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("shader lab validation probe"),
                size: 4,
                usage: wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shader lab glass lens"),
            source: wgpu::ShaderSource::Wgsl(include_str!("glass.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shader lab glass lens"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("fullscreen_triangle"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("refract"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let layout = pipeline.get_bind_group_layout(0);

        move |frame: &mut CanvasFrame<'_>, scene: Backdrop<'_>| {
            let [strength, ..] = frame.params;
            if strength == 1.0
                && let Some(buffer) = &probe
            {
                // The live smoke test asks for an encoding failure at full refraction.
                frame.encoder.clear_buffer(buffer, 0, None);
                return;
            }
            let [width, height] = frame.size;
            let [x, y] = scene.offset;
            let bytes = [width as f32, height as f32, frame.scale, strength, x as f32, y as f32, frame.radius, 0.0].map(f32::to_ne_bytes);
            let uniforms = frame.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("shader lab lens parameters"),
                contents: bytes.as_flattened(),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let inputs = frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("shader lab lens inputs"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(scene.view) },
                ],
            });
            let mut pass = frame.begin_render_pass(
                "shader_lab_glass_refraction",
                &wgpu::RenderPassDescriptor {
                    label: Some("shader lab lens resolve"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: frame.target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    })],
                    ..Default::default()
                },
            );
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &inputs, &[]);
            pass.draw(0..3, 0..1);
        }
    })
}

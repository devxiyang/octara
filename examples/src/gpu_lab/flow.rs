use octara_primitives::paint::PaintProgram;
use octara_render::{
    gpu::{CanvasFrame, canvas, retained_canvas},
    wgpu,
};
use std::sync::Mutex;
use wgpu::util::DeviceExt;

const SHADER: &str = concat!(include_str!("palette.wgsl"), include_str!("flow.wgsl"));
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

pub(super) fn program(retained: bool) -> PaintProgram {
    let prepare = |device: &wgpu::Device| {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shader lab flow field"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let compute = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("shader lab vector field"),
            layout: None,
            module: &shader,
            entry_point: Some("vector_field"),
            compilation_options: Default::default(),
            cache: None,
        });
        let compute_layout = compute.get_bind_group_layout(0);
        let render_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shader lab render inputs"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let render_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shader lab render passes"),
            bind_group_layouts: &[Some(&render_layout)],
            immediate_size: 0,
        });
        let resolve = render_pipeline(device, &shader, &render_pipeline_layout, "color_resolve", None);
        let grid = render_pipeline(
            device,
            &shader,
            &render_pipeline_layout,
            "reference_grid",
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        );
        let scratch = Mutex::new(None::<([u32; 2], wgpu::TextureView)>);

        move |frame: &mut CanvasFrame<'_>| {
            let [width, height] = frame.size;
            let field = {
                let mut scratch = scratch.lock().expect("shader lab scratch texture");
                if scratch.as_ref().is_some_and(|(size, _)| *size != frame.size) {
                    *scratch = None;
                }
                scratch.get_or_insert_with(|| (frame.size, field_texture(frame.device, width, height))).1.clone()
            };
            // The field texture can be reused in queue order. Keep each recording's parameters separate.
            let bytes = frame.params.into_iter().chain([frame.scale, 0.0, 0.0, 0.0]).flat_map(f32::to_ne_bytes).collect::<Vec<_>>();
            let uniforms = frame.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("shader lab parameters"),
                contents: &bytes,
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let compute_inputs = inputs(frame.device, &compute_layout, &uniforms, 0, &field);
            let render_inputs = inputs(frame.device, &render_layout, &uniforms, 2, &field);
            {
                let mut pass = frame.begin_compute_pass(
                    "shader_lab_flow_compute",
                    &wgpu::ComputePassDescriptor { label: Some("shader lab vector field"), timestamp_writes: None },
                );
                pass.set_pipeline(&compute);
                pass.set_bind_group(0, &compute_inputs, &[]);
                pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
            }
            draw(frame, &resolve, &render_inputs, wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), "shader_lab_color_resolve");
            if frame.params[2] > 0.5 {
                draw(frame, &grid, &render_inputs, wgpu::LoadOp::Load, "shader_lab_grid_composite");
            }
        }
    };
    if retained { retained_canvas(prepare) } else { canvas(prepare) }
}

fn field_texture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("shader lab field samples"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

fn inputs(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    params: &wgpu::Buffer,
    binding: u32,
    field: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("shader lab pass inputs"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 1, resource: params.as_entire_binding() },
            wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::TextureView(field) },
        ],
    })
}

fn render_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    fragment: &'static str,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(fragment),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("fullscreen_triangle"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format: FORMAT, blend, write_mask: wgpu::ColorWrites::ALL })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn draw(
    frame: &mut CanvasFrame<'_>,
    pipeline: &wgpu::RenderPipeline,
    bind: &wgpu::BindGroup,
    load: wgpu::LoadOp<wgpu::Color>,
    label: &'static str,
) {
    let mut pass = frame.begin_render_pass(
        label,
        &wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: frame.target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        },
    );
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind, &[]);
    pass.draw(0..3, 0..1);
}

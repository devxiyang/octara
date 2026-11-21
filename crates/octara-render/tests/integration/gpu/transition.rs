use octara_primitives::capture::{PaintOutcome, Painter};
use octara_primitives::identity::NodeId;
use octara_primitives::paint::{CustomPaint, DisplayList, Item, PaintProgram, Part, Quad, Rect};
use octara_primitives::theme::{Color, rgb};
use octara_render::gpu::{CanvasFrame, canvas};
use octara_render::{GpuRenderer, wgpu};
use wgpu::util::DeviceExt;

const SCALE: f32 = 2.0;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

#[derive(Clone)]
struct TransitionInput {
    view: wgpu::TextureView,
    bounds: Rect,
}

impl TransitionInput {
    fn new(device: &wgpu::Device, bounds: Rect) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("transition source"),
            size: wgpu::Extent3d { width: (bounds.w * SCALE) as u32, height: (bounds.h * SCALE) as u32, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        Self { view: texture.create_view(&Default::default()), bounds }
    }

    fn render(&self, gpu: &mut GpuRenderer, painted_height: f32, color: Color) {
        let Rect { w, h, .. } = self.bounds;
        let panel = filled_quad(Rect::new(0.0, 0.0, w, painted_height), color);
        let list = DisplayList { size: (w, h), background: Color::TRANSPARENT, parts: vec![Part::Item(Item::Quad(panel))] };
        let encoder = gpu.device().create_command_encoder(&Default::default());
        let texture = self.view.texture();
        gpu.record(encoder, &list, SCALE, &self.view, (texture.width(), texture.height())).submit();
    }

    fn origin(&self) -> [f32; 2] {
        [self.bounds.x * SCALE, self.bounds.y * SCALE]
    }
}

fn filled_quad(rect: Rect, color: Color) -> Quad {
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

fn crossfade(old: TransitionInput, new: TransitionInput) -> PaintProgram {
    canvas(move |device| {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("two-input transition"),
            source: wgpu::ShaderSource::Wgsl(include_str!("transition.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("two-input transition"),
            layout: None,
            module: &shader,
            entry_point: Some("crossfade"),
            compilation_options: Default::default(),
            cache: None,
        });
        let layout = pipeline.get_bind_group_layout(0);
        let (old, new) = (old.clone(), new.clone());
        move |frame: &mut CanvasFrame<'_>| {
            let [old_x, old_y] = old.origin();
            let [new_x, new_y] = new.origin();
            let output_x = frame.rect.x * frame.scale;
            let output_y = frame.rect.y * frame.scale;
            let bytes = [output_x, output_y, old_x, old_y, new_x, new_y, frame.params[0], 0.0].map(f32::to_ne_bytes);
            let uniforms = frame.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("transition origins and progress"),
                contents: bytes.as_flattened(),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let inputs = frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("transition inputs and output"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&old.view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&new.view) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(frame.target) },
                ],
            });
            let [width, height] = frame.size;
            let mut pass = frame.begin_compute_pass("crossfade", &Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &inputs, &[]);
            pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
        }
    })
}

#[test]
fn gpu_crossfades_two_local_outputs_in_window_coordinates() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let info = adapter.get_info();
    eprintln!("transition adapter: {} ({:?})", info.name, info.backend);
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).expect("a GPU device");
    let mut gpu = GpuRenderer::with_device(device, queue, FORMAT);

    // Keep the previous submission's texture while producing the incoming panel on the same queue.
    // Both inputs stay on the GPU. Only the finished composition is read back.
    let old = TransitionInput::new(gpu.device(), Rect::new(5.0, 4.0, 14.0, 13.0));
    old.render(&mut gpu, 12.0, Color::new([1.0, 0.0, 0.0, 0.5]));
    let new = TransitionInput::new(gpu.device(), Rect::new(11.0, 6.0, 13.0, 11.0));
    new.render(&mut gpu, 9.0, Color::new([0.0, 0.0, 1.0, 0.25]));
    let source = NodeId::default();
    let bounds = Rect::new(4.0, 3.0, 22.0, 16.0);
    let paint = CustomPaint {
        fallback: filled_quad(bounds, Color::BLACK),
        program: crossfade(old, new.clone()),
        mount: Default::default(),
        exiting: false,
        params: [0.0; 4],
        opacity: 1.0,
    };

    const GREEN: [u8; 4] = [0, 255, 0, 255];
    let checks = [
        ("both inputs", (28, 22), [[64, 191, 0, 255], [32, 207, 16, 255], [0, 223, 32, 255], [32, 175, 48, 255]]),
        ("old input only", (16, 22), [[64, 191, 0, 255], [32, 223, 0, 255], GREEN, [32, 223, 0, 255]]),
        ("new input only", (41, 22), [GREEN, [0, 239, 16, 255], [0, 223, 32, 255], [0, 207, 48, 255]]),
        ("opacity of overlapping paints", (32, 22), [[128, 191, 64, 255], [112, 207, 96, 255], [96, 223, 128, 255], [80, 175, 96, 255]]),
        ("transparent inside both sources", (28, 32), [GREEN; 4]),
        ("outer rounded clip", (10, 8), [GREEN; 4]),
        ("inner clip", (46, 18), [GREEN; 4]),
        ("outside the effect", (55, 22), [GREEN; 4]),
        ("later foreground", (26, 19), [[255; 4]; 4]),
    ];
    for (index, (progress, updated)) in [(0.0, false), (0.5, false), (1.0, false), (0.5, true)].into_iter().enumerate() {
        if updated {
            // Repeat the midpoint after recording new incoming pixels with the old input still held.
            new.render(&mut gpu, 9.0, Color::new([0.0, 0.0, 1.0, 0.75]));
        }
        let mut paint = paint.clone();
        paint.params[0] = progress;
        let parts = [
            Item::PushClip { source, rect: bounds, radius: 8.0 },
            Item::PushClip { source, rect: Rect::new(4.0, 4.0, 18.0, 14.0), radius: 0.0 },
            Item::PushLayer { source, opacity: 0.5 },
            Item::Quad(filled_quad(Rect::new(15.0, 10.0, 2.0, 3.0), Color::WHITE)),
            Item::Custom(paint),
            Item::PopLayer,
            Item::PopClip,
            Item::PopClip,
            Item::Quad(filled_quad(Rect::new(12.0, 9.0, 2.0, 2.0), Color::WHITE)),
        ];
        let list = DisplayList { size: (30.0, 22.0), background: rgb(0x00ff00), parts: parts.into_iter().map(Part::Item).collect() };
        let frame = gpu.paint_with_receipts(&list, SCALE, None).expect("a composited transition frame");
        assert_eq!((frame.picture.width, frame.picture.height), (60, 44));
        let [effect] = frame.effects.as_slice() else { panic!("expected one author pass") };
        assert_eq!(effect.outcome, PaintOutcome::Rendered);
        for (name, (x, y), expected) in checks {
            let actual = frame.picture.pixel(x, y);
            let expected = expected[index];
            eprintln!("progress {progress}, updated {updated}: {name} ({x}, {y}) = {actual:?}");
            assert!(
                actual.into_iter().zip(expected).all(|(actual, expected)| actual.abs_diff(expected) <= 1),
                "progress {progress}, updated {updated}: {name} ({x}, {y}) = {actual:?}, expected {expected:?}"
            );
        }
    }
}

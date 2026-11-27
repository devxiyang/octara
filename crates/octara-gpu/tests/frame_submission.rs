#![cfg(not(target_arch = "wasm32"))]

use octara_model::{
    capture::{PaintOutcome, PaintSample},
    geometry::{Rect, Size},
    identity::NodeId,
    paint::{CustomPaint, DisplayList, Image, ImagePaint, Item, Part, Quad},
    theme::Color,
};
use octara_render::{GpuRenderer, wgpu};

#[test]
fn gpu_submits_only_the_kept_frame_with_its_surrounding_commands() {
    use octara_render::gpu::{CanvasFrame, FrameInput, FrameTarget, OutputColor, canvas};
    use std::time::Duration;

    let Some(mut gpu) = GpuRenderer::new() else {
        assert!(std::env::var_os("OCTARA_REQUIRE_GPU").is_none(), "a GPU adapter is required");
        return;
    };
    let red = [255, 0, 0, 255];
    let green = [0, 255, 0, 255];
    let blue = [0, 0, 255, 255];
    let source_image = Image::rgba(8, 8, red.repeat(64));
    let publisher = std::thread::current().id();
    let sampled_at = Duration::from_millis(25);
    let authored = canvas(move |_| {
        move |frame: &mut CanvasFrame<'_>| {
            assert_ne!(std::thread::current().id(), publisher);
            assert_eq!(frame.sampled_at, Some(sampled_at));
            drop(frame.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: frame.target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLUE), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            }));
        }
    });
    let size = (16, 8);
    let left = Rect::new(0.0, 0.0, 8.0, 8.0);
    let right = Rect::new(8.0, 0.0, 8.0, 8.0);
    let author = NodeId::default();
    let scene = DisplayList {
        size: Size::new(size.0 as f32, size.1 as f32),
        background: Color::TRANSPARENT,
        parts: vec![
            Part::Item(Item::Image(ImagePaint {
                source: author,
                image: source_image.sample(),
                rect: left,
                bounds: left,
                radius: [0.0; 4],
                opacity: 1.0,
            })),
            Part::Item(Item::Custom(CustomPaint {
                fallback: Quad {
                    source: author,
                    rect: right,
                    radius: [0.0; 4],
                    fill: Some(Color::BLACK),
                    gradient: None,
                    border: [0.0; 4],
                    border_color: Color::TRANSPARENT,
                },
                program: authored,
                mount: Default::default(),
                exiting: false,
                input: Default::default(),
                opacity: 1.0,
            })),
        ],
    };
    let sample = PaintSample { sampled_at, revision: 1 };
    source_image.set(8, 8, green.repeat(64));

    let (pixels, paint) = std::thread::scope(|scope| {
        scope
            .spawn(move || {
                let device = gpu.device().clone();
                let target = device.create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
                let input = FrameInput { list: &scene, scale: 1.0, sample: Some(sample) };
                let output = FrameTarget { texture: &target, color: OutputColor::SrgbOpaque };
                // The discarded frame must leave the image uncommitted.
                let encoder = device.create_command_encoder(&Default::default());
                let (discarded, _) = gpu.record(encoder, output, input, None).unwrap();
                let upload_bytes = discarded.upload_bytes();
                drop(discarded);

                let readback = device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: 16,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                let copy_pixel = |encoder: &mut wgpu::CommandEncoder, x, offset| {
                    encoder.copy_texture_to_buffer(
                        wgpu::TexelCopyTextureInfo { origin: wgpu::Origin3d { x, y: 4, z: 0 }, ..target.as_image_copy() },
                        wgpu::TexelCopyBufferInfo {
                            buffer: &readback,
                            layout: wgpu::TexelCopyBufferLayout { offset, bytes_per_row: None, rows_per_image: None },
                        },
                        wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                    );
                };
                let mut encoder = device.create_command_encoder(&Default::default());
                copy_pixel(&mut encoder, 4, 0);
                copy_pixel(&mut encoder, 12, 4);
                let (mut draw, _) = gpu.record(encoder, output, input, None).unwrap();
                assert_eq!(draw.upload_bytes(), upload_bytes, "the kept frame must restage the discarded image");
                copy_pixel(draw.encoder(), 4, 8);
                copy_pixel(draw.encoder(), 12, 12);
                let paint = draw.paint_receipts().to_vec();
                let submission = draw.submit();
                readback.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
                device.poll(wgpu::PollType::Wait { submission_index: Some(submission), timeout: None }).unwrap();
                let pixels: [[u8; 4]; 4] = readback.get_mapped_range(..).unwrap().as_chunks::<4>().0.try_into().unwrap();
                let encoder = device.create_command_encoder(&Default::default());
                let (warm, _) = gpu.record(encoder, output, input, None).unwrap();
                assert!(warm.upload_bytes() + 8 * 8 * 4 <= upload_bytes, "only a submission can make the image resident");
                (pixels, paint)
            })
            .join()
            .expect("the renderer worker finished")
    });
    assert_eq!(pixels, [[0; 4], [0; 4], red, blue]);
    let [effect] = paint.as_slice() else { panic!("the worker should return one author receipt") };
    assert_eq!((effect.source, effect.outcome), (author, PaintOutcome::Rendered));
}

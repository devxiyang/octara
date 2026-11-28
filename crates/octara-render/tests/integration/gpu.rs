//! The GPU renderer against the reference renderer: same display list, same pixels within two
//! levels per channel (premultiplied). Skipped when there is no GPU.

use super::fixtures::{glyphs, list, quad};
use octara_model::capture::PaintSample;
use octara_model::geometry::Rect;
use octara_model::geometry::point;
use octara_model::identity::NodeId;
use octara_model::native::NativeSlot;
use octara_model::paint::Picture;
use octara_model::paint::Sample;
use octara_model::paint::{CustomPaint, Image, PaintInput, Quad, Shadow};
use octara_model::paint::{DisplayList, ImagePaint, Item, Part};
use octara_model::theme::{Color, Gradient, rgb};
use octara_render::{CpuRenderer, WgpuRenderer, wgpu};

mod micro_pipelines;
mod transition;

fn gpu_or_skip() -> Option<WgpuRenderer> {
    let gpu = WgpuRenderer::new();
    if gpu.is_none() {
        assert!(std::env::var_os("OCTARA_REQUIRE_GPU").is_none(), "a GPU adapter is required");
        eprintln!("no GPU adapter: skipped");
    }
    gpu
}

fn compare(name: &str, cpu: &Picture, gpu: &Picture) {
    assert_eq!((cpu.width(), cpu.height()), (gpu.width(), gpu.height()), "{name}: size");
    let (mut worst, mut off, mut at) = (0u8, 0usize, (0, 0));
    for y in 0..cpu.height() {
        for x in 0..cpu.width() {
            // Compare color weighted by coverage so barely visible edge colors carry little weight.
            let pre = |p: [u8; 4]| [0, 1, 2].map(|i| ((p[i] as u32 * p[3] as u32 + 127) / 255) as u8);
            let (a, b) = (cpu.pixel(x, y).unwrap(), gpu.pixel(x, y).unwrap());
            let d = pre(a).iter().zip(pre(b)).map(|(a, b)| a.abs_diff(b)).max().unwrap().max(a[3].abs_diff(b[3]));
            if d > 0 {
                off += 1;
            }
            if d > worst {
                worst = d;
                at = (x, y);
            }
        }
    }
    eprintln!("{name}: max channel difference {worst} at {at:?}, {off} pixels differ");
    assert!(
        worst <= 2,
        "{name}: max channel difference {worst} at {at:?} (cpu {:?}, gpu {:?})",
        cpu.pixel(at.0, at.1).unwrap(),
        gpu.pixel(at.0, at.1).unwrap()
    );
}

fn picture() -> Image {
    let (w, h) = (24u32, 16u32);
    let mut px = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let checker = ((x / 4) + (y / 4)) % 2 == 0;
            let a = if x < 6 && y < 6 { 0 } else { 255 };
            px.extend([(x * 10) as u8, (y * 15) as u8, if checker { 230 } else { 40 }, a]);
        }
    }
    Image::rgba(w, h, px)
}

fn sample(image: &Image, rect: Rect, bounds: Rect, radius: f32, opacity: f32) -> Item {
    Item::Image(ImagePaint { source: NodeId::default(), image: image.sample(), rect, bounds, radius: [radius; 4], opacity })
}

fn scene(background: Color) -> DisplayList {
    let source = NodeId::default();
    let accent = rgb(0x3b82f6);
    let danger = rgb(0xef4444);
    let image = picture();
    let clip = Rect::new(10.25, 10.5, 135.0, 90.0);
    let content = Rect::new(26.75, 22.5, 94.0, 48.0);
    let gradient = Quad {
        gradient: Some(Gradient { angle: 135.0, from: (accent.with_alpha(0.8), 0.2), to: (danger, 0.85) }),
        border: [1.0, 2.0, 3.0, 4.0],
        border_color: rgb(0x7c3aed),
        radius: [15.0, 8.0, 22.0, 0.0],
        ..quad(content, 11.0, accent)
    };
    list(
        (176.0, 128.0),
        background,
        [
            Item::PushClip { source, rect: clip, radius: [19.0, 30.0, 0.0, 10.0] },
            Item::Shadow(Shadow {
                source,
                rect: Rect::new(25.5, 30.25, 102.0, 60.0),
                radius: [14.0, 0.0, 18.0, 21.0],
                color: accent.with_alpha(0.6),
                blur: 8.0,
            }),
            Item::Shadow(Shadow {
                source,
                rect: Rect::new(11.5, 12.25, 36.0, 44.0),
                radius: [6.0; 4],
                color: danger.with_alpha(0.7),
                blur: 0.0,
            }),
            Item::Quad(gradient),
            Item::PushLayer { source, opacity: 0.8 },
            Item::Quad(quad(Rect::new(12.0, 42.25, 30.5, 43.0), 4.0, danger)),
            Item::PushClip { source, rect: Rect::new(16.0, 45.0, 125.0, 72.0), radius: [13.0; 4] },
            Item::PushLayer { source, opacity: 0.55 },
            sample(&image, Rect::new(42.25, 32.5, 105.0, 62.0), Rect::new(54.0, 38.25, 80.0, 56.0), 14.0, 0.65),
            glyphs("你好世界", point(31.25, 82.0), 25.0, Color::WHITE),
            Item::PopLayer,
            Item::PopClip,
            Item::PopLayer,
            Item::PopClip,
            sample(&image, Rect::new(145.0, 18.0, 28.0, 17.0), Rect::new(145.0, 18.0, 28.0, 17.0), 4.0, 1.0),
            Item::Quad(quad(Rect::new(132.5, 86.25, 26.0, 30.0), 12.0, danger.with_alpha(0.4))),
        ],
    )
}

const IMAGE_COLORS: [[u8; 4]; 5] = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255, 192, 0, 255], [192, 0, 255, 255]];

fn solid_image(width: u32, height: u32, rgba: [u8; 4]) -> Sample {
    Image::rgba(width, height, rgba.repeat((width * height) as usize)).sample()
}

fn image_row(images: &[Sample]) -> DisplayList {
    let parts = images
        .iter()
        .enumerate()
        .map(|(column, image)| {
            let rect = Rect { x: column as f32 * 8.0, y: 0.0, w: 8.0, h: 8.0 };
            Part::Item(Item::Image(ImagePaint {
                source: NodeId::default(),
                image: image.clone(),
                rect,
                bounds: rect,
                radius: [0.0; 4],
                opacity: 1.0,
            }))
        })
        .collect();
    DisplayList { size: octara_model::geometry::Size::new(images.len() as f32 * 8.0, 8.0), background: Color::WHITE, parts }
}

fn image_row_pixels(gpu: &mut WgpuRenderer, row: &DisplayList) -> Vec<[u8; 4]> {
    let picture = gpu.render(row, 1.0);
    (4..picture.width()).step_by(8).map(|x| picture.pixel(x, 4).unwrap()).collect()
}

#[test]
fn gpu_draws_an_oversized_image_alongside_packed_images() {
    let Some(mut gpu) = gpu_or_skip() else { return };
    let [red, green, blue, amber, purple] = IMAGE_COLORS;
    let boundary = image_row(&[solid_image(2048, 1, red)]);
    assert_eq!(image_row_pixels(&mut gpu, &boundary), [red]);

    let wide = solid_image(1535, 1023, red);
    let narrow = solid_image(511, 1023, green);
    let square_a = solid_image(1023, 1023, blue);
    let square_b = solid_image(1023, 1023, amber);
    let packed = image_row(&[wide.clone(), narrow.clone(), square_a.clone(), square_b.clone()]);
    assert_eq!(image_row_pixels(&mut gpu, &packed), [red, green, blue, amber]);

    // Reorder the four resident images and append an image above the atlas edge.
    let oversized = solid_image(2049, 1, purple);
    let reordered = image_row(&[wide, square_a, square_b, narrow, oversized]);
    assert_eq!(image_row_pixels(&mut gpu, &reordered), [red, blue, amber, green, purple]);

    let mut filtered = reordered;
    let rect = Rect::new(28.25, 1.25, 9.5, 6.0);
    let stripe = (0..2049).flat_map(|x| if x < 1024 { [255, 64, 0, 128] } else { [0, 32, 255, 200] }).collect::<Vec<_>>();
    filtered.parts.push(Part::Item(Item::Image(ImagePaint {
        source: NodeId::default(),
        image: Image::rgba(2049, 2, stripe.repeat(2)).sample(),
        rect,
        bounds: rect,
        radius: [2.0; 4],
        opacity: 0.55,
    })));
    compare("oversized image filtering and overlap", &CpuRenderer::new().render(&filtered, 1.0), &gpu.render(&filtered, 1.0));
}

#[test]
fn gpu_draws_the_full_working_set_when_images_fill_more_than_one_page() {
    let Some(mut gpu) = gpu_or_skip() else { return };
    // Extend the frame beyond one atlas page, then revisit each working set.
    let images = IMAGE_COLORS.map(|color| solid_image(1023, 1023, color));
    let fitting = image_row(&images[..4]);
    assert_eq!(image_row_pixels(&mut gpu, &fitting), IMAGE_COLORS[..4]);

    let overflow = image_row(&images);
    assert_eq!(image_row_pixels(&mut gpu, &overflow), IMAGE_COLORS);
    assert_eq!(image_row_pixels(&mut gpu, &overflow), IMAGE_COLORS);
    assert_eq!(image_row_pixels(&mut gpu, &image_row(&images[4..])), IMAGE_COLORS[4..]);
    assert_eq!(image_row_pixels(&mut gpu, &fitting), IMAGE_COLORS[..4]);
}

#[test]
fn gpu_rejects_images_above_the_device_texture_limit_and_recovers() {
    let Some(mut gpu) = gpu_or_skip() else { return };
    let limit = gpu.device().limits().max_texture_dimension_2d;
    let [red, green, ..] = IMAGE_COLORS;
    let retained = solid_image(1, 1, red);
    let mut bad = image_row(&[retained.clone(), solid_image(limit + 1, 1, green)]);
    let error = gpu.paint(&bad, 1.0).unwrap_err();
    assert!(error.contains("exceeds the GPU dimension limit"), "{error}");
    assert_eq!(image_row_pixels(&mut gpu, &image_row(&[retained])), [red]);

    // Only visible textures participate in the frame's resource checks.
    let Part::Item(Item::Image(offscreen)) = bad.parts.last_mut().unwrap() else { panic!("the oversized image") };
    offscreen.rect.x = 32.0;
    offscreen.bounds.x = 32.0;
    assert_eq!(image_row_pixels(&mut gpu, &bad), [red, [255; 4]]);
}

#[test]
fn gpu_redraws_each_sampled_image_version() {
    let Some(mut gpu) = gpu_or_skip() else { return };
    let [red, green, blue, ..] = IMAGE_COLORS;
    let first = solid_image(2048, 1023, red);
    let image = first.source().clone();
    let old_list = image_row(std::slice::from_ref(&first));
    image.set(2047, 1023, green.repeat(2047 * 1023));
    let second = image.sample();
    let new_list = image_row(std::slice::from_ref(&second));
    // Drawing the first sample twice must reuse its slot even when both versions are visible.
    let mixed_list = image_row(&[first.clone(), second, first]);
    image.set(1, 1, blue);

    assert_eq!(image_row_pixels(&mut gpu, &new_list), [green]);
    assert_eq!(image_row_pixels(&mut gpu, &old_list), [red]);
    assert_eq!(image_row_pixels(&mut gpu, &mixed_list), [red, green, red]);
}

#[test]
fn gpu_matches_the_reference_renderer() {
    let Some(mut gpu) = gpu_or_skip() else { return };
    let adapter = gpu.device().adapter_info();
    eprintln!("GPU pixel parity on {} ({:?})", adapter.name, adapter.backend);
    let mut cpu = CpuRenderer::new();
    for (name, background) in [("transparent", Color::TRANSPARENT), ("white", Color::WHITE), ("dark", rgb(0x161b22))] {
        let dl = scene(background);
        for scale in [1.0, 2.0] {
            compare(&format!("{name} scene at {scale}x"), &cpu.render(&dl, scale), &gpu.render(&dl, scale));
        }
    }
}

fn capture_with_upload_bytes(gpu: &mut WgpuRenderer, list: &DisplayList, scale: f32) -> (Picture, u64) {
    let mut capture = gpu.record_capture(list, scale, None).unwrap().submit().unwrap();
    gpu.device().poll(wgpu::PollType::Wait { submission_index: Some(capture.submission().clone()), timeout: None }).unwrap();
    let picture = capture.take_mapped().unwrap().unwrap().finish().unwrap().picture;
    (picture, capture.upload_bytes())
}

#[test]
fn gpu_spatial_draws_match_nested_parent_and_content_clips() {
    use octara_model::geometry::{Size, Vec2};
    use octara_model::paint::{FillRule, Path, PathPaint, PathStyle, ScrollPlacement, ScrollPlacements, Transform};
    use std::sync::Arc;

    let Some(mut gpu) = gpu_or_skip() else { return };
    let mut cpu = CpuRenderer::new();
    let source = NodeId::default();
    let area = Rect::new(9.25, 17.0, 74.0, 62.0);
    let image = picture();
    let inner: Arc<[Part]> = Arc::from([
        Part::Item(Item::PushClip { source, rect: Rect::new(3.25, 21.5, 69.5, 62.0), radius: [9.0, 7.0, 19.0, 0.0] }),
        Part::Item(Item::Quad(quad(area, 6.5, rgb(0x3b82f6).with_alpha(0.7)))),
        Part::Item(sample(&image, area, Rect::new(18.0, 21.5, 53.25, 55.0), 14.0, 0.5)),
        Part::Item(glyphs("你好", point(21.25, 48.5), 22.0, Color::WHITE)),
        Part::Item(Item::Path(PathPaint {
            source,
            path: Path::circle(point(0.0, 0.0), 7.5),
            style: PathStyle::Fill(FillRule::NonZero),
            transform: Transform::translate(40.1, 44.75),
            bounds: Rect::new(32.6, 37.25, 15.0, 15.0),
            color: rgb(0xc04b22),
            gradient: None,
        })),
        Part::Item(Item::PopClip),
    ]);
    let outer: Arc<[Part]> = Arc::from([
        Part::Item(Item::PushAxisClip { source, rect: Rect::new(0.0, 16.25, 0.0, 44.0), horizontal: false }),
        Part::ScrollContent { source, spatial_id: 2, scrollbar_offset: Vec2::default(), parts: inner },
        Part::Item(Item::PopClip),
    ]);
    let parts: Arc<[Part]> = Arc::from([
        Part::Item(Item::PushClip { source, rect: Rect::new(13.25, 8.25, 64.0, 47.5), radius: [0.0; 4] }),
        Part::ScrollContent { source, spatial_id: 1, scrollbar_offset: Vec2::default(), parts: outer },
        Part::Item(Item::PopClip),
    ]);
    for scale in [2.0, 3.0] {
        for (index, shift) in [0.0, -3.4, -10.15, -20.2, -39.2, -2.12].into_iter().enumerate() {
            let mut scrolls = ScrollPlacements::default();
            scrolls.set(source, Some(ScrollPlacement { offset: Vec2::new(0.0, shift), scrollbars: [None, None] }));
            let dl = DisplayList {
                size: Size::new(94.0, 76.0),
                background: Color::TRANSPARENT,
                parts: vec![Part::ScrollFrame { scrolls, parts: parts.clone() }],
            };
            let (picture, uploaded) = capture_with_upload_bytes(&mut gpu, &dl, scale);
            if index > 0 {
                assert_eq!(uploaded, 2 * 16, "only the two spatial rows changed");
            }
            compare(&format!("nested scroll, {scale}x, {shift}pt"), &cpu.render(&dl, scale), &picture);
        }
    }
}

#[test]
fn gpu_retains_translation_motion_with_fractional_x() {
    use octara_model::geometry::{Size, Vec2};
    use octara_model::paint::{ScrollPlacement, ScrollPlacements};
    use std::sync::Arc;

    let Some(mut gpu) = gpu_or_skip() else { return };
    let mut cpu = CpuRenderer::new();
    let source = NodeId::default();
    let content = Part::Item(Item::Quad(quad(Rect::new(2.25, 4.0, 37.5, 23.0), 7.25, rgb(0x138191))));
    let parts: Arc<[Part]> = Arc::from([
        Part::Item(Item::PushClip { source, rect: Rect::new(10.0, 0.0, 38.0, 34.0), radius: [4.0; 4] }),
        Part::MotionContent { source, spatial_id: 1, parts: Arc::from([content]) },
        Part::Item(Item::PopClip),
    ]);
    for (index, offset) in [Vec2::default(), Vec2::new(0.15, 2.55), Vec2::new(18.7, 12.4)].into_iter().enumerate() {
        let mut scrolls = ScrollPlacements::default();
        scrolls.set(source, Some(ScrollPlacement { offset, scrollbars: [None, None] }));
        let list = DisplayList {
            size: Size::new(60.0, 40.0),
            background: Color::TRANSPARENT,
            parts: vec![Part::ScrollFrame { scrolls, parts: parts.clone() }],
        };
        let (picture, uploaded) = capture_with_upload_bytes(&mut gpu, &list, 3.0);
        if index > 0 {
            assert_eq!(uploaded, 16, "only the moving layer's spatial row changed");
        }
        compare("translation motion at 3x", &cpu.render(&list, 3.0), &picture);
    }
}

#[test]
fn capture_painter_rejects_invalid_sizes_without_losing_the_renderer() {
    let Some(mut gpu) = gpu_or_skip() else { return };
    let mut list = DisplayList { size: octara_model::geometry::Size::new(3.25, 2.5), background: Color::WHITE, parts: Vec::new() };
    for scale in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, -1.0] {
        assert!(gpu.paint(&list, scale).is_err(), "scale {scale} must be refused before rendering");
    }
    for size in [(f32::MAX, 1.0), (1.0, f32::MAX), (f32::NAN, 1.0), (1.0, f32::INFINITY), (-1.0, 1.0)] {
        list.size = size.into();
        assert!(gpu.paint(&list, 1.0).is_err(), "size {size:?} must be refused before rendering");
    }
    list.size = octara_model::geometry::Size::new(3.25, 2.5);
    let picture = gpu.paint(&list, 2.0).expect("a valid capture still works after refusals");
    assert_eq!((picture.width(), picture.height()), (7, 5));
    assert_eq!(picture.rgba(), [255; 7 * 5 * 4]);

    for (background, expected) in [(Color::new([1.0, 0.25, 0.5, 0.5]), [255, 64, 128, 128]), (Color::new([1.0, 0.0, 1.0, 0.0]), [0; 4])] {
        list.background = background;
        let picture: Picture = gpu.render(&list, 2.0);
        let pixel = picture.pixel(0, 0).unwrap();
        assert!(pixel.into_iter().zip(expected).all(|(actual, expected)| actual.abs_diff(expected) <= 1), "{pixel:?} vs {expected:?}");
        assert_eq!(picture, gpu.paint(&list, 2.0).unwrap());
    }
}

#[test]
fn a_published_frame_rebuilds_its_resources_on_a_replacement_device() {
    use octara_render::gpu::{CanvasFrame, canvas, material};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let Ok(adapter) = octara_render::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())) else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let device = |adapter: &wgpu::Adapter| {
        let (device, queue) = octara_render::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("a GPU device");
        WgpuRenderer::with_device(device, queue)
    };
    let mut gpu = device(&adapter);
    let preparations = Arc::new(AtomicU32::new(0));
    let authored = canvas({
        let preparations = preparations.clone();
        move |device| {
            preparations.fetch_add(1, Ordering::Relaxed);
            let size = wgpu::Extent3d { width: 8, height: 8, depth_or_array_layers: 1 };
            let scratch = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("loss regression author texture"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: CanvasFrame::FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = scratch.create_view(&Default::default());
            move |frame: &mut CanvasFrame<'_>| {
                assert_eq!(frame.sampled_at, Some(Duration::from_millis(25)));
                assert_eq!((frame.rect.w, frame.rect.h, frame.radius), (8.0, 8.0, [3.0; 4]));
                let color = *frame.input.data::<wgpu::Color>().expect("the scene owns a color source for this device");
                drop(frame.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(color), store: wgpu::StoreOp::Store },
                    })],
                    ..Default::default()
                }));
                frame.encoder.copy_texture_to_texture(scratch.as_image_copy(), frame.target.texture().as_image_copy(), size);
            }
        }
    });
    let material_rect = Rect::new(10.0, 80.0, 8.0, 8.0);
    let canvas_rect = Rect::new(20.0, 80.0, 8.0, 8.0);
    let image_rect = Rect::new(10.0, 30.0, 25.0, 17.0);
    let site = |rect, radius, program, input| {
        Item::Custom(CustomPaint {
            fallback: quad(rect, radius, Color::BLACK),
            program,
            mount: Default::default(),
            exiting: false,
            input,
            opacity: 1.0,
        })
    };
    let display = list(
        (160.0, 96.0),
        Color::WHITE,
        [
            glyphs("你好", point(8.0, 20.0), 14.0, Color::BLACK),
            sample(&picture(), image_rect, image_rect, 0.0, 1.0),
            site(
                material_rect,
                0.0,
                material("fn material(uv: vec2f, params: vec4f) -> vec4f { return params; }"),
                [1.0, 0.0, 0.0, 1.0].into(),
            ),
            site(canvas_rect, 3.0, authored, PaintInput::with_data([0.0; 4], Arc::new(wgpu::Color::BLUE))),
        ],
    );
    let sample = Some(PaintSample { revision: 1, sampled_at: Duration::from_millis(25) });
    let original = gpu.paint_with_receipts(&display, 1.0, sample).unwrap().picture;
    assert_eq!(original.pixel(14, 84), Some([255, 0, 0, 255]));
    assert_eq!(original.pixel(24, 84), Some([0, 0, 255, 255]));
    assert_eq!(preparations.load(Ordering::Relaxed), 1);

    let lost = gpu.device().clone();
    gpu.device().destroy();
    assert!(gpu.paint_with_receipts(&display, 1.0, sample).is_err());
    drop(gpu);

    let adapter = octara_render::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).expect("a live adapter");
    let mut replacement = device(&adapter);
    assert_ne!(replacement.device(), &lost);
    let restored = replacement.paint_with_receipts(&display, 1.0, sample).unwrap().picture;
    assert_eq!(restored, original);
    assert_eq!(preparations.load(Ordering::Relaxed), 2, "author resources were built once per device");
}

/// Paths of every kind, and one larger than an atlas tile.
fn paths_list() -> DisplayList {
    use octara_model::paint::PathPaint;
    use octara_model::paint::{FillRule, LineCap, LineJoin, Path, PathStyle, Stroke, Transform};
    use octara_model::theme::Gradient;
    let item = |path: Path, style: PathStyle, color: Color, transform: Transform| {
        let b = path.bounds().unwrap_or_default();
        let o = transform.apply(point(b.x, b.y));
        Item::Path(PathPaint {
            source: NodeId::default(),
            path,
            transform,
            style,
            color,
            gradient: None,
            bounds: Rect { x: o.x, y: o.y, w: b.w, h: b.h },
        })
    };
    let ring = Path::circle(point(0.0, 0.0), 30.0).append(&Path::circle(point(0.0, 0.0), 16.0));
    let zigzag = Path::new().add_polygon(&[point(0.0, 30.0), point(20.0, 0.0), point(40.0, 30.0), point(60.0, 0.0)], false);
    let mut gradient = item(
        Path::rounded_rect(Rect { x: 0.0, y: 0.0, w: 90.0, h: 50.0 }, 12.0),
        PathStyle::Fill(FillRule::NonZero),
        Color::BLACK,
        Transform::translate(300.0, 20.0),
    );
    if let Item::Path(p) = &mut gradient {
        p.gradient = Some(Gradient { angle: 135.0, from: (rgb(0xf59e0b), 0.0), to: (rgb(0x7c3aed), 1.0) });
    }
    let items = vec![
        // Larger than a 1024-pixel tile at 2×: drawn in pieces.
        item(Path::circle(point(320.0, 300.0), 280.0), PathStyle::Fill(FillRule::NonZero), rgb(0xdbeafe), Transform::IDENTITY),
        item(ring.clone(), PathStyle::Fill(FillRule::EvenOdd), rgb(0x2563eb), Transform::translate(50.3, 50.7)),
        item(ring, PathStyle::Fill(FillRule::NonZero), rgb(0x10b981).with_alpha(0.6), Transform::translate(110.0, 50.0)),
        item(
            zigzag.clone(),
            PathStyle::Stroke(Stroke::new(6.0).join(LineJoin::Round).cap(LineCap::Round)),
            rgb(0xef4444),
            Transform::translate(160.0, 30.0),
        ),
        item(zigzag.clone(), PathStyle::Stroke(Stroke::new(3.0).dash(&[6.0, 4.0])), rgb(0x111827), Transform::translate(160.0, 80.0)),
        item(zigzag, PathStyle::Stroke(Stroke::new(4.0)), rgb(0x7c3aed), Transform::rotate(30.0).then(&Transform::translate(240.0, 40.0))),
        gradient,
        Item::PushClip { source: NodeId::default(), rect: Rect { x: 20.0, y: 120.0, w: 120.0, h: 60.0 }, radius: [16.0; 4] },
        item(Path::circle(point(80.0, 150.0), 50.0), PathStyle::Fill(FillRule::NonZero), rgb(0xec4899), Transform::IDENTITY),
        Item::PopClip,
        Item::PushLayer { source: NodeId::default(), opacity: 0.5 },
        item(
            Path::rect(Rect { x: 200.0, y: 120.0, w: 60.0, h: 60.0 }),
            PathStyle::Fill(FillRule::NonZero),
            rgb(0x0ea5e9),
            Transform::IDENTITY,
        ),
        item(Path::circle(point(260.0, 180.0), 30.0), PathStyle::Fill(FillRule::NonZero), rgb(0xf97316), Transform::IDENTITY),
        Item::PopLayer,
    ];
    DisplayList {
        size: octara_model::geometry::Size::new(640.0, 400.0),
        background: Color::WHITE,
        parts: items.into_iter().map(Part::Item).collect(),
    }
}

#[test]
fn gpu_paths_match_the_reference_renderer() {
    let Some(mut gpu) = gpu_or_skip() else { return };
    let mut cpu = CpuRenderer::new();
    let dl = paths_list();
    for scale in [1.0, 2.0] {
        compare(&format!("paths@{scale}x"), &cpu.render(&dl, scale), &gpu.render(&dl, scale));
    }
}

#[test]
fn gpu_draws_glyphs_and_paths_across_mask_pages_without_reuploading() {
    use octara_model::paint::PathPaint;
    use octara_model::paint::{FillRule, Path, PathStyle, Transform};
    use octara_render::gpu::{FrameInput, FrameTarget, OutputColor};

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let Ok(adapter) = octara_render::block_on(instance.request_adapter(&Default::default())) else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let edge = 128;
    let limits = wgpu::Limits { max_texture_dimension_2d: edge, ..Default::default() };
    let (device, queue) =
        octara_render::block_on(adapter.request_device(&wgpu::DeviceDescriptor { required_limits: limits, ..Default::default() })).unwrap();
    let mut gpu = WgpuRenderer::with_device(device, queue);
    let mut list = list(
        (edge as f32, edge as f32),
        Color::WHITE,
        [glyphs("你好", point(12.25, 40.0), 22.0, Color::BLACK), glyphs("世界", point(8.5, 75.0), 26.0, Color::BLACK)],
    );
    assert!(list.glyph_runs().next().is_some());
    let bounds = Rect::new(0.0, 0.0, edge as f32, edge as f32);
    list.parts.insert(
        0,
        Part::Item(Item::Path(PathPaint {
            source: NodeId::default(),
            path: Path::rect(bounds),
            transform: Transform::IDENTITY,
            style: PathStyle::Fill(FillRule::NonZero),
            color: rgb(0xdbeafe),
            gradient: None,
            bounds,
        })),
    );
    let reference = CpuRenderer::new().render(&list, 1.0);
    let picture = gpu.render(&list, 1.0);
    compare("glyphs following a full mask page", &reference, &picture);

    let target = gpu.device().create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: edge, height: edge, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let encoder = gpu.device().create_command_encoder(&Default::default());
    let target = FrameTarget { texture: &target, color: OutputColor::SrgbOpaque };
    let input = FrameInput { list: &list, scale: 1.0, sample: None };
    let (frame, profile) = gpu.record(encoder, target, input, Some(Box::new(|_| {}))).unwrap();
    let profile = profile.unwrap();
    assert!(profile.atlas_bytes >= 2 * u64::from(edge).pow(2));
    assert_eq!(profile.atlas_upload_bytes, 0);
    frame.submit();
}

#[test]
fn compact_glyph_draws_keep_colors_clips_layers_and_painter_order() {
    let Some(mut gpu) = gpu_or_skip() else { return };
    let source = NodeId::default();
    let Item::Glyphs(mut run) = glyphs("你好世界", point(4.25, 28.5), 20.0, Color::BLACK) else {
        panic!("glyph fixture must return a run");
    };
    run.colors = Some([rgb(0xb52f36).with_alpha(0.65), rgb(0x16c857), Color::WHITE, Color::TRANSPARENT].into());
    let simple = list((98.0, 40.0), Color::TRANSPARENT, [Item::Glyphs(run.clone())]);
    let first = gpu.render(&simple, 1.25);
    compare("per-glyph colors", &CpuRenderer::new().render(&simple, 1.25), &first);
    let capture = gpu.record_capture(&simple, 1.25, None).unwrap().submit().unwrap();
    assert_eq!(capture.upload_bytes(), 16 + 3 * (8 + 24), "a projection and depth metadata and payload for three visible glyphs");

    let scene = list(
        (98.0, 40.0),
        Color::TRANSPARENT,
        [
            Item::PushClip { source, rect: Rect::new(8.25, 4.125, 78.5, 29.75), radius: [0.0; 4] },
            Item::PushLayer { source, opacity: 0.7 },
            Item::Glyphs(run.clone()),
            Item::Quad(quad(Rect::new(27.0, 6.0, 19.0, 25.0), 2.0, rgb(0x662ff8))),
            Item::PushClip { source, rect: Rect::new(37.0, 5.0, 30.5, 22.25), radius: [7.0; 4] },
            Item::PushLayer { source, opacity: 0.5 },
            Item::Glyphs(octara_model::paint::GlyphRun { origin: point(32.0, 27.0), ..run }),
            Item::PopLayer,
            Item::PopClip,
            Item::PopLayer,
            Item::PopClip,
        ],
    );
    compare("painter-ordered glyph layers and clips", &CpuRenderer::new().render(&scene, 1.25), &gpu.render(&scene, 1.25));
}

#[test]
fn underlay_cutouts_clear_to_transparent() {
    let dl = list(
        (200.0, 128.0),
        rgb(0xeeeeee),
        [
            Item::Quad(quad(Rect::new(10.0, 10.0, 180.0, 110.0), 12.0, Color::WHITE)),
            Item::Native {
                source: NodeId::default(),
                rect: Rect::new(20.0, 20.0, 160.0, 90.0),
                radius: [10.0; 4],
                slot: NativeSlot::Underlay,
                cut: true,
            },
            Item::Quad(quad(Rect::new(40.0, 40.0, 32.0, 32.0), 16.0, rgb(0x2255aa))),
        ],
    );
    let cpu = CpuRenderer::new().render(&dl, 1.0);
    assert_eq!(cpu.pixel(150, 80).unwrap()[3], 0, "the view shows through");
    assert_eq!(cpu.pixel(56, 56).unwrap()[3], 255, "the button painted after it stays");
    assert_eq!(cpu.pixel(5, 5).unwrap()[3], 255, "the page around it stays");
    let Some(mut gpu) = gpu_or_skip() else { return };
    for scale in [1.0, 2.0] {
        compare(&format!("underlay@{scale}x"), &CpuRenderer::new().render(&dl, scale), &gpu.render(&dl, scale));
    }
}

use super::atlas::SceneTextures;
use super::encode::color_pass;
use super::pipeline::color_texture;
use super::*;
use crate::plan::batch::{Batch, Draw};
use crate::plan::{self, FramePlan, TargetInfo};
use octara_model::capture::{PaintFallback, PaintOutcome, PaintReceipt};
use octara_model::geometry::Rect;
use octara_model::identity::NodeId;
use octara_model::paint::{CustomPaint, Item, PaintInput, PaintMount, PaintProgram, Part, Picture, Quad, TransitionPaint};
use octara_model::theme::{Color, rgb};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

fn solid(rect: Rect, color: Color) -> Quad {
    Quad {
        source: NodeId::default(),
        rect,
        radius: [0.0; 4],
        fill: Some(color),
        gradient: None,
        border: [0.0; 4],
        border_color: Color::TRANSPARENT,
    }
}

fn picture(items: impl IntoIterator<Item = Item>) -> DisplayList {
    DisplayList {
        size: octara_model::geometry::Size::new(32.0, 24.0),
        background: Color::TRANSPARENT,
        parts: items.into_iter().map(Part::Item).collect(),
    }
}

fn close(actual: [u8; 4], expected: [u8; 4]) {
    assert!(actual.into_iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 1), "pixel {actual:?}, expected {expected:?}");
}

fn device_target(device: &wgpu::Device, usage: wgpu::TextureUsages) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: 32, height: 24, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: WORKING_FORMAT,
        usage: usage | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
}

fn linear_target(texture: &wgpu::Texture) -> FrameTarget<'_> {
    FrameTarget { texture, color: OutputColor::Linear }
}

fn half_pixel(pixel: [f32; 4]) -> [u8; 8] {
    pixel.map(|channel| half::f16::from_f32(channel).to_bits().to_le_bytes()).as_flattened().try_into().unwrap()
}

fn copy_sample(frame: &mut CanvasFrame<'_>, sample: &Backdrop<'_>, from: [u32; 2], to: [u32; 2], size: [u32; 2]) {
    let origin = |[x, y]: [u32; 2]| wgpu::Origin3d { x, y, z: 0 };
    frame.encoder.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo { origin: origin(from), ..sample.view.texture().as_image_copy() },
        wgpu::TexelCopyTextureInfo { origin: origin(to), ..frame.target.texture().as_image_copy() },
        wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
    );
}

fn program_box(rect: Rect, program: PaintProgram, params: [f32; 4]) -> CustomPaint {
    CustomPaint {
        fallback: solid(rect, rgb(0xff00ff)),
        program,
        input: params.into(),
        opacity: 1.0,
        mount: PaintMount::default(),
        exiting: false,
    }
}

#[test]
fn one_device_records_into_independent_output_formats() {
    use wgpu::TextureFormat::{Bgra8Unorm, Bgra8UnormSrgb, Rgba8Unorm, Rgba8UnormSrgb};

    let Some(mut gpu) = WgpuRenderer::new() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let device = gpu.device().clone();
    let (sample_format, samples) = mpsc::channel();
    let glass = backdrop(move |_| {
        let sample_format = sample_format.clone();
        move |_, sample| sample_format.send(sample.view.texture().format()).unwrap()
    });
    let color = material("fn material(uv: vec2f, params: vec4f) -> vec4f { return params; }");
    let paint = |x| Item::Custom(program_box(Rect::new(x, 0.0, 8.0, 8.0), color.clone(), [0.25, 0.5, 0.75, 1.0]));
    let list = picture([
        paint(0.0),
        Item::PushLayer { source: NodeId::default(), opacity: 0.5 },
        paint(8.0),
        Item::PopLayer,
        Item::Custom(program_box(Rect::new(0.0, 0.0, 8.0, 8.0), glass, [0.0; 4])),
        Item::Quad(solid(Rect::new(16.0, 0.0, 8.0, 8.0), Color::BLACK)),
        Item::Quad(solid(Rect::new(16.0, 0.0, 8.0, 8.0), Color::WHITE.with_alpha(0.5))),
    ]);
    for (format, encoding, group) in [
        (Bgra8UnormSrgb, OutputColor::SrgbOpaque, [99, 137, 165, 128]),
        (Rgba8UnormSrgb, OutputColor::SrgbOpaque, [99, 137, 165, 128]),
        (Rgba8Unorm, OutputColor::SrgbOpaque, [99, 137, 165, 128]),
        (Bgra8Unorm, OutputColor::SrgbStraight, [137, 188, 225, 128]),
        (Rgba8UnormSrgb, OutputColor::SrgbStraight, [137, 188, 225, 128]),
        (Bgra8UnormSrgb, OutputColor::SrgbPremultiplied, [68, 94, 112, 128]),
        (Bgra8UnormSrgb, OutputColor::SrgbOpaque, [99, 137, 165, 128]),
    ] {
        let direct = encoding == OutputColor::SrgbOpaque && format.is_srgb();
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: 32, height: 24, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 16,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let encoder = device.create_command_encoder(&Default::default());
        let input = FrameInput { list: &list, scale: 1.0, sample: None };
        let output = FrameTarget { texture: &target, color: encoding };
        let (mut frame, profile) = gpu.record(encoder, output, input, Some(Box::new(|_| {}))).unwrap();
        assert_eq!(profile.unwrap().output_pixels, if direct { 0 } else { 32 * 24 });
        assert!(frame.paint_receipts().iter().all(|receipt| receipt.outcome == PaintOutcome::Rendered));
        for (x, offset) in [(4, 0), (12, 4), (20, 8), (28, 12)] {
            frame.encoder().copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo { origin: wgpu::Origin3d { x, y: 4, z: 0 }, ..target.as_image_copy() },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout { offset, bytes_per_row: None, rows_per_image: None },
                },
                wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            );
        }
        let submission = frame.submit();
        buffer.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
        device.poll(wgpu::PollType::Wait { submission_index: Some(submission), timeout: None }).unwrap();
        let pixels = buffer.get_mapped_range(..).unwrap();
        let expected = [[137, 188, 225, 255], group, [188, 188, 188, 255], [0; 4]];
        for (&[r, g, b, a], expected) in pixels.as_chunks::<4>().0.iter().zip(expected) {
            let rgba = if format.remove_srgb_suffix() == Bgra8Unorm { [b, g, r, a] } else { [r, g, b, a] };
            close(rgba, expected);
        }
        assert_eq!(samples.try_recv().unwrap(), if direct { format } else { WORKING_FORMAT });
    }
    assert_eq!(gpu.pipelines.formats.len(), 3);
    let capture = gpu.render(&list, 1.0);
    close(capture.pixel(4, 4).unwrap(), [137, 188, 225, 255]);
    close(capture.pixel(12, 4).unwrap(), [137, 188, 225, 128]);
    close(capture.pixel(20, 4).unwrap(), [188, 188, 188, 255]);
    assert_eq!(capture.pixel(28, 4).unwrap(), [0; 4]);
    assert_eq!(samples.try_recv().unwrap(), WORKING_FORMAT);
}

fn profile_frame<'a>(
    gpu: &'a mut WgpuRenderer,
    target: &wgpu::Texture,
    list: &DisplayList,
    scale: f32,
    revision: u64,
) -> (RecordedFrame<'a>, FrameProfile) {
    let encoder = gpu.device().create_command_encoder(&Default::default());
    let sample = Some(PaintSample { revision, sampled_at: Duration::from_millis(revision) });
    let (frame, profile) = gpu.record(encoder, linear_target(target), FrameInput { list, scale, sample }, Some(Box::new(|_| {}))).unwrap();
    (frame, profile.expect("a profiled frame"))
}

#[test]
fn retained_scroll_submits_sparse_offsets_with_shared_atlas_pixels() {
    use octara_model::geometry::Vec2;
    use octara_model::image::Image;
    use octara_model::paint::{ImagePaint, ScrollPlacement, ScrollPlacements};

    let Some(mut gpu) = WgpuRenderer::new() else {
        assert!(std::env::var_os("OCTARA_REQUIRE_GPU").is_none(), "a GPU adapter is required");
        return;
    };
    let source = NodeId::default();
    let rect = Rect::new(3.0, 11.0, 12.0, 8.0);
    let image = Image::rgba(1, 1, [255, 0, 0, 255]);
    let geometry = Arc::from([
        Part::Item(Item::PushClip { source, rect: Rect::new(2.0, 7.0, 14.0, 12.0), radius: [0.0; 4] }),
        Part::ScrollContent {
            source,
            spatial_id: 91,
            scrollbar_offset: Vec2::default(),
            parts: Arc::from([Part::Item(Item::Image(ImagePaint {
                source,
                image: image.sample(),
                rect,
                bounds: rect,
                radius: [0.0; 4],
                opacity: 1.0,
            }))]),
        },
        Part::Item(Item::PopClip),
    ]);
    let scene = |x, y| {
        let mut scrolls = ScrollPlacements::default();
        scrolls.set(source, Some(ScrollPlacement { offset: Vec2::new(x, y), scrollbars: [None, None] }));
        DisplayList { parts: vec![Part::ScrollFrame { scrolls, parts: Arc::clone(&geometry) }], ..picture([]) }
    };
    let first_list = scene(0.0, 0.0);
    let scrolled_list = scene(0.25, -5.6);
    let discarded = gpu.record_capture(&first_list, 1.0, None).unwrap();
    drop(discarded);
    assert!(gpu.prepared.is_none());

    let mut first = gpu.record_capture(&first_list, 1.0, None).unwrap().submit().unwrap();
    let mut second = gpu.record_capture(&scrolled_list, 1.0, None).unwrap().submit().unwrap();
    assert!(first.upload_bytes() > 2 * 16);
    assert_eq!(second.upload_bytes(), 16);
    gpu.device().poll(wgpu::PollType::Wait { submission_index: Some(second.submission().clone()), timeout: None }).unwrap();
    let first = first.take_mapped().unwrap().unwrap().finish().unwrap().picture;
    let second = second.take_mapped().unwrap().unwrap().finish().unwrap().picture;
    for x in [4, 10] {
        assert_eq!(first.pixel(x, 8), Some([0; 4]));
        assert_eq!(second.pixel(x, 8), Some([255, 0, 0, 255]));
        assert_eq!(second.pixel(x, 6), Some([0; 4]));
        assert_eq!(first.pixel(x, 16), Some([255, 0, 0, 255]));
        assert_eq!(second.pixel(x, 16), Some([0; 4]));
    }

    gpu.set_target_generation(1);
    assert!(gpu.prepared.is_none());
    let target = device_target(gpu.device(), wgpu::TextureUsages::COPY_SRC);
    let (first, profile) = profile_frame(&mut gpu, &target, &first_list, 1.0, 1);
    assert!(!profile.retained_prepare && profile.instance_upload_bytes > 0);
    first.submit();
    let (abandoned, profile) = profile_frame(&mut gpu, &target, &scrolled_list, 1.0, 2);
    assert!(profile.retained_prepare);
    assert_eq!((profile.instance_upload_bytes, profile.spatial_upload_bytes, profile.atlas_upload_bytes), (0, 16, 0));
    drop(abandoned);
    // A discarded recording cannot publish its new offsets as submitted data.
    let (next, profile) = profile_frame(&mut gpu, &target, &scrolled_list, 1.0, 3);
    assert!(!profile.retained_prepare && profile.instance_upload_bytes > 0);
    next.submit();
}

#[test]
fn retained_scroll_inherits_an_enclosing_translation() {
    use octara_model::geometry::Vec2;
    use octara_model::paint::{ScrollPlacement, ScrollPlacements};

    let Some(mut gpu) = WgpuRenderer::new() else {
        assert!(std::env::var_os("OCTARA_REQUIRE_GPU").is_none(), "a GPU adapter is required");
        return;
    };
    let source = NodeId::default();
    let geometry = Arc::from([Part::TranslatedGroup {
        spatial_id: 7,
        offset: Vec2::new(8.0, 0.0),
        parts: Arc::from([Part::ScrollContent {
            source,
            spatial_id: 9,
            scrollbar_offset: Vec2::default(),
            parts: Arc::from([Part::Item(Item::Quad(solid(Rect::new(1.0, 8.0, 4.0, 4.0), Color::WHITE)))]),
        }]),
    }]);
    let scene = |y| {
        let mut scrolls = ScrollPlacements::default();
        scrolls.set(source, Some(ScrollPlacement { offset: Vec2::new(0.0, y), scrollbars: [None, None] }));
        DisplayList { parts: vec![Part::ScrollFrame { scrolls, parts: Arc::clone(&geometry) }], ..picture([]) }
    };
    let first = scene(0.0);
    let second = scene(-4.0);
    let first = gpu.render(&first, 1.0);
    assert_eq!(first.pixel(10, 9), Some([255; 4]));
    assert_eq!(first.pixel(2, 9), Some([0; 4]));
    let second = gpu.record_capture(&second, 1.0, None).unwrap().submit().unwrap();
    assert_eq!(second.upload_bytes(), 16);
}

fn retained_color(draws: Arc<AtomicUsize>) -> PaintProgram {
    retained_canvas(move |device| {
        let draws = draws.clone();
        let copy_source = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("retained canvas validation probe"),
            size: 4,
            usage: wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        move |frame: &mut CanvasFrame<'_>| {
            draws.fetch_add(1, Ordering::Relaxed);
            assert_eq!((frame.rect.x, frame.rect.y, frame.sampled_at), (0.0, 0.0, None));
            let [r, g, b, a] = frame.input.params.map(f64::from);
            if r < 0.0 {
                frame.encoder.clear_buffer(&copy_source, 0, None);
            } else {
                drop(color_pass(frame.encoder, frame.target, wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a }), None));
            }
        }
    })
}

#[test]
fn independent_canvas_updates_keep_exact_inputs_and_drop_superseded_recordings() {
    let Some(mut gpu) = WgpuRenderer::new() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let draws = Arc::new(AtomicUsize::new(0));
    let program = retained_color(draws.clone());
    let mut first = program_box(Rect::new(0.0, 0.0, 6.0, 6.0), program.clone(), [1.0, 0.0, 0.0, 1.0]);
    let second = program_box(Rect::new(12.0, 3.0, 6.0, 6.0), program.clone(), [0.0, 0.0, 1.0, 1.0]);
    let clipped = program_box(Rect::new(26.0, 3.0, 6.0, 6.0), program, [1.0; 4]);
    let volatile = program_box(Rect::new(3.0, 11.0, 6.0, 6.0), canvas(|_| |_: &mut CanvasFrame<'_>| {}), [1.0; 4]);
    let source = NodeId::default();
    let mut list = picture([Item::PushClip { source, rect: Rect::new(1.0, 1.0, 20.0, 18.0), radius: [0.0; 4] }]);
    list.parts.push(Part::Drawing {
        items: vec![Item::Custom(first.clone())].into(),
        offset: octara_model::geometry::Vec2::new(3.0, 3.0),
        fade: 1.0,
    });
    list.parts.extend([Item::Custom(second), Item::Custom(clipped), Item::Custom(volatile), Item::PopClip].map(Part::Item));

    let mut updates = gpu.canvas_updates(&list, 1.0);
    assert!(gpu.prepare_next_canvas(&mut updates, 10, || true).unwrap());
    assert_eq!(draws.load(Ordering::Relaxed), 1);
    assert!(!updates.is_empty());
    assert!(gpu.prepare_next_canvas(&mut updates, 10, || true).unwrap());
    assert!(updates.is_empty(), "only visible retained canvases became independent work");

    let mut reusable = gpu.canvas_updates(&list, 1.0);
    assert!(!gpu.prepare_next_canvas(&mut reusable, 10, || true).unwrap());
    assert_eq!(draws.load(Ordering::Relaxed), 2);

    first.input.params = [0.0, 1.0, 0.0, 1.0];
    first.fallback.rect = Rect::new(3.0, 3.0, 6.0, 6.0);
    let newer = picture([Item::Custom(first)]);
    let mut cancelled = gpu.canvas_updates(&newer, 1.0);
    assert!(!gpu.prepare_next_canvas(&mut cancelled, 20, || false).unwrap());

    let old = gpu.paint_with_receipts(&list, 1.0, None).unwrap();
    assert!(old.effects[..2].iter().all(|receipt| receipt.outcome == PaintOutcome::Retained { revision: Some(10) }));
    close(old.picture.pixel(5, 5).unwrap(), [255, 0, 0, 255]);
    close(old.picture.pixel(14, 5).unwrap(), [0, 0, 255, 255]);
    let new = gpu.paint_with_receipts(&newer, 1.0, None).unwrap();
    assert_eq!(new.effects[0].outcome, PaintOutcome::Rendered);
    close(new.picture.pixel(5, 5).unwrap(), [0, 255, 0, 255]);
}

#[test]
fn retained_canvas_tracks_data_versions_through_discard_and_capture() {
    let Some(mut gpu) = WgpuRenderer::new() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let program = retained_canvas(|_| {
        |frame: &mut CanvasFrame<'_>| {
            let color = *frame.input.data::<wgpu::Color>().expect("published canvas color");
            drop(color_pass(frame.encoder, frame.target, wgpu::LoadOp::Clear(color), None));
        }
    });
    let paint = program_box(Rect::new(2.0, 2.0, 8.0, 8.0), program, [0.0; 4]);
    let list = |color| {
        let input = PaintInput::with_data([0.0; 4], Arc::new(color));
        picture([Item::Custom(CustomPaint { input, ..paint.clone() })])
    };
    let old = list(wgpu::Color::RED);
    let discarded = list(wgpu::Color::BLUE);
    let current = list(wgpu::Color::GREEN);
    let target = device_target(gpu.device(), wgpu::TextureUsages::empty());
    profile_frame(&mut gpu, &target, &old, 1.0, 10).0.submit();
    drop(profile_frame(&mut gpu, &target, &discarded, 1.0, 20).0);

    let still_old = gpu.paint_with_receipts(&old, 1.0, None).unwrap();
    assert_eq!(still_old.effects[0].outcome, PaintOutcome::Retained { revision: Some(10) });
    close(still_old.picture.pixel(4, 4).unwrap(), [255, 0, 0, 255]);
    let (recorded, _) = profile_frame(&mut gpu, &target, &current, 1.0, 30);
    assert_eq!(recorded.paint_receipts()[0].outcome, PaintOutcome::Rendered, "only the author's data version changed");
    recorded.submit();

    let replay = gpu.paint_with_receipts(&old, 1.0, None).unwrap();
    assert_eq!(replay.effects[0].outcome, PaintOutcome::Rendered);
    assert_eq!(replay.picture, still_old.picture);
    let live = gpu.paint_with_receipts(&current, 1.0, None).unwrap();
    assert_eq!(live.effects[0].outcome, PaintOutcome::Retained { revision: Some(30) });
    close(live.picture.pixel(4, 4).unwrap(), [0, 255, 0, 255]);
}

#[test]
fn retained_canvas_reuses_pixels_through_scroll_clip_and_group_opacity() {
    let Some(mut gpu) = WgpuRenderer::new() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let draws = Arc::new(AtomicUsize::new(0));
    let mut paint = program_box(Rect::new(2.0, 3.0, 6.25, 7.25), retained_color(draws.clone()), [1.0, 0.0, 0.0, 1.0]);
    paint.fallback.radius = [0.5; 4];
    let target = device_target(gpu.device(), wgpu::TextureUsages::empty());
    let first = picture([Item::Custom(paint.clone())]);
    let (recorded, profile) = profile_frame(&mut gpu, &target, &first, 1.0, 10);
    assert_eq!(recorded.paint_receipts()[0].outcome, PaintOutcome::Rendered);
    assert_eq!((profile.canvas_pixels, profile.retained_canvases, profile.render_passes), (56, 0, 1));
    recorded.submit();

    let source = NodeId::default();
    let mut moved = picture([
        Item::PushClip { source, rect: Rect::new(13.0, 8.0, 6.0, 9.0), radius: [0.0; 4] },
        Item::PushLayer { source, opacity: 0.5 },
    ]);
    moved.parts.push(Part::Drawing {
        items: vec![Item::Custom(paint.clone())].into(),
        offset: octara_model::geometry::Vec2::new(10.0, 5.0),
        fade: 1.0,
    });
    moved.parts.extend([Part::Item(Item::PopLayer), Part::Item(Item::PopClip)]);
    let hit = PaintOutcome::Retained { revision: Some(10) };
    let (recorded, profile) = profile_frame(&mut gpu, &target, &moved, 1.0, 20);
    assert_eq!(recorded.paint_receipts()[0].outcome, hit);
    assert_eq!((profile.canvas_pixels, profile.retained_canvases, profile.render_passes), (0, 1, 2));
    recorded.submit();
    let capture = gpu.paint_with_receipts(&moved, 1.0, None).unwrap();
    assert_eq!(capture.effects[0].outcome, hit);
    assert_eq!(draws.load(Ordering::Relaxed), 1);
    close(capture.picture.pixel(15, 11).unwrap(), [255, 0, 0, 128]);
    assert_eq!(capture.picture.pixel(12, 11).unwrap(), [0; 4]);
    assert_eq!(capture.picture.pixel(4, 5).unwrap(), [0; 4]);

    let mut variants = vec![];
    paint.fallback.rect.w = 6.5; // The logical box changed while its pixel extent stayed 7×8.
    variants.push((paint.clone(), 1.0));
    paint.fallback.radius = [1.5; 4];
    variants.push((paint.clone(), 1.0));
    variants.push((paint.clone(), 1.05)); // This scale also preserves the pixel extent.
    paint.input.params = [0.0, 1.0, 0.0, 1.0];
    variants.push((paint.clone(), 1.05));
    paint.mount = PaintMount::default();
    variants.push((paint.clone(), 1.05));
    paint.program = retained_color(draws.clone());
    variants.push((paint, 1.05));
    for (index, (paint, scale)) in variants.iter().enumerate() {
        let list = picture([Item::Custom(paint.clone())]);
        let (recorded, profile) = profile_frame(&mut gpu, &target, &list, *scale, 30 + index as u64);
        assert_eq!(recorded.paint_receipts()[0].outcome, PaintOutcome::Rendered, "changed input {index}");
        assert_eq!((profile.canvas_pixels, profile.retained_canvases), (56, 0));
        recorded.submit();
    }
    assert_eq!(draws.load(Ordering::Relaxed), 1 + variants.len());
}

#[test]
fn retained_canvas_keeps_submitted_pixels_and_independent_outputs_for_duplicate_mounts() {
    let Some(mut gpu) = WgpuRenderer::new() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let draws = Arc::new(AtomicUsize::new(0));
    let paint = program_box(Rect::new(2.0, 2.0, 8.0, 8.0), retained_color(draws.clone()), [1.0, 0.0, 0.0, 1.0]);
    let draw = |x: f32, w: f32, params: [f32; 4]| {
        let mut paint = CustomPaint { input: params.into(), ..paint.clone() };
        paint.fallback.rect.x = x;
        paint.fallback.rect.w = w;
        Item::Custom(paint)
    };
    let red = [1.0, 0.0, 0.0, 1.0];
    let green = [0.0, 1.0, 0.0, 1.0];
    let blue = [0.0, 0.0, 1.0, 1.0];
    let submitted = picture([draw(2.0, 8.0, green)]);
    let device = gpu.device().clone();
    let target = device_target(&device, wgpu::TextureUsages::COPY_SRC);
    for (revision, frame) in [(10, picture([draw(2.0, 8.0, red)])), (20, submitted.clone())] {
        profile_frame(&mut gpu, &target, &frame, 1.0, revision).0.submit();
    }
    let unsubmitted = picture([draw(2.0, 8.0, blue)]);
    let (recorded, _) = profile_frame(&mut gpu, &target, &unsubmitted, 1.0, 30);
    assert_eq!(recorded.paint_receipts()[0].outcome, PaintOutcome::Rendered);
    drop(recorded);
    let resized = picture([draw(2.0, 12.0, blue)]);
    let capture = gpu.paint_with_receipts(&resized, 1.0, None).unwrap();
    assert_eq!(capture.effects[0].outcome, PaintOutcome::Rendered);
    close(capture.picture.pixel(11, 4).unwrap(), [0, 0, 255, 255]);
    let (recorded, _) = profile_frame(&mut gpu, &target, &resized, 1.0, 40);
    assert_eq!(recorded.paint_receipts()[0].outcome, PaintOutcome::Rendered);
    drop(recorded);

    let failed = picture([draw(2.0, 8.0, [-1.0, 0.0, 0.0, 1.0])]);
    let (recorded, profile) = profile_frame(&mut gpu, &target, &failed, 1.0, 50);
    assert_eq!(recorded.paint_receipts()[0].outcome, PaintOutcome::Fallback { reason: PaintFallback::EncodingFailed });
    assert_eq!((profile.canvas_pixels, profile.retained_canvases), (0, 0));
    recorded.submit();
    let capture = gpu.paint_with_receipts(&submitted, 1.0, None).unwrap();
    assert_eq!(capture.effects[0].outcome, PaintOutcome::Retained { revision: Some(20) });
    close(capture.picture.pixel(4, 4).unwrap(), [0, 255, 0, 255]);
    assert_eq!(draws.load(Ordering::Relaxed), 6);

    // Both author draws precede the target pass. A shared working allocation would render
    // the last color at both positions when the renderer samples them.
    let duplicate = picture([draw(2.0, 8.0, blue), draw(16.0, 8.0, red)]);
    let (mut recorded, profile) = profile_frame(&mut gpu, &target, &duplicate, 1.0, 60);
    assert!(recorded.paint_receipts().iter().all(|receipt| receipt.outcome == PaintOutcome::Rendered));
    assert_eq!((profile.canvas_pixels, profile.retained_canvases), (128, 0));
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 16,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    for (x, offset) in [(4, 0), (18, 8)] {
        recorded.encoder().copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { origin: wgpu::Origin3d { x, y: 4, z: 0 }, ..target.as_image_copy() },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout { offset, bytes_per_row: None, rows_per_image: None },
            },
            wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        );
    }
    let submission = recorded.submit();
    readback.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
    device.poll(wgpu::PollType::Wait { submission_index: Some(submission), timeout: None }).unwrap();
    let expected = [[0.0, 0.0, 1.0, 1.0], [1.0, 0.0, 0.0, 1.0]].map(half_pixel);
    assert_eq!(&*readback.get_mapped_range(..).unwrap(), expected.as_flattened());

    drop(submitted);
    drop(unsubmitted);
    drop(resized);
    drop(failed);
    drop(duplicate);
    drop(paint);
    gpu.render(&picture([]), 1.0);
    assert!(gpu.canvas_cache.is_empty());
}

#[test]
fn nested_rounded_clips_keep_both_corners_in_layered_draws() {
    let source = NodeId::default();
    let rect = Rect::new(0.0, 0.0, 16.0, 16.0);
    let dl = |draw| {
        picture([
            Item::PushClip { source, rect, radius: [8.0; 4] },
            Item::PushLayer { source, opacity: 0.5 },
            Item::PushClip { source, rect: Rect::new(1.0, 1.0, 12.0, 12.0), radius: [1.0; 4] },
            draw,
            Item::PopClip,
            Item::PopLayer,
            Item::PopClip,
        ])
    };
    let assert_corners = |image: Picture| {
        // The outer circle removes this pixel even though the inner rounded rectangle covers it.
        assert_eq!(image.pixel(1, 1).unwrap(), [0; 4]);
        // The inner right edge removes this pixel even though the outer circle covers it.
        assert_eq!(image.pixel(13, 8).unwrap(), [0; 4]);
        // At (2.5, 2.5), the outer distance is sqrt(5.5² + 5.5²) − 8.
        close(image.pixel(2, 2).unwrap(), [255, 0, 0, 92]);
        close(image.pixel(8, 8).unwrap(), [255, 0, 0, 128]);
    };
    let red = Item::Quad(solid(rect, rgb(0xff0000)));
    assert_corners(crate::CpuRenderer::new().render(&dl(red.clone()), 1.0));
    let Some(mut gpu) = WgpuRenderer::new() else {
        eprintln!("no GPU adapter: skipped GPU corner check");
        return;
    };
    assert_corners(gpu.render(&dl(red), 1.0));
    let program = material("fn material(uv: vec2f, params: vec4f) -> vec4f { return params; }");
    assert_corners(gpu.render(&dl(Item::Custom(program_box(rect, program, [1.0, 0.0, 0.0, 1.0]))), 1.0));
}

fn crossfade() -> PaintProgram {
    use wgpu::util::DeviceExt;

    transition(|device| {
        let source = "
            struct Inputs {
                origins: vec4f,
                output: vec4f,
                source_scales: vec4f,
            };
            @group(0) @binding(0) var<uniform> inputs: Inputs;
            @group(0) @binding(1) var old_image: texture_2d<f32>;
            @group(0) @binding(2) var new_image: texture_2d<f32>;
            @group(0) @binding(3) var output_image: texture_storage_2d<rgba16float, write>;

            fn sample_at(image: texture_2d<f32>, xy: vec2f, origin: vec2f, scale: f32) -> vec4f {
                let at = vec2i(floor(xy * scale - origin));
                if any(at < vec2i(0)) || any(at >= vec2i(textureDimensions(image))) {
                    return vec4f(0.0);
                }
                return textureLoad(image, at, 0);
            }

            @compute @workgroup_size(8, 8)
            fn fade(@builtin(global_invocation_id) pixel: vec3u) {
                if any(pixel.xy >= textureDimensions(output_image)) { return; }
                let xy = inputs.output.xy + (vec2f(pixel.xy) + 0.5) / inputs.output.z;
                let old_pixel = sample_at(old_image, xy, inputs.origins.xy, inputs.source_scales.x);
                let new_pixel = sample_at(new_image, xy, inputs.origins.zw, inputs.source_scales.y);
                textureStore(output_image, vec2i(pixel.xy), mix(old_pixel, new_pixel, inputs.output.w));
            }
        ";
        let shader =
            device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(source.into()) });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &shader,
            entry_point: Some("fade"),
            compilation_options: Default::default(),
            cache: None,
        });
        let layout = pipeline.get_bind_group_layout(0);
        move |frame: &mut CanvasFrame<'_>, images: Transition<'_>| {
            let [ox, oy] = images.old.origin.map(|n| n as f32);
            let [nx, ny] = images.new.origin.map(|n| n as f32);
            let data = [
                [ox, oy, nx, ny],
                [frame.rect.x, frame.rect.y, frame.scale, images.progress],
                [images.old.scale, images.new.scale, 0.0, 0.0],
            ];
            let uniforms = frame.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::bytes_of(&data),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let [old, new, target] = [images.old.view, images.new.view, frame.target];
            let bind = frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(old) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(new) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(target) },
                ],
            });
            let [w, h] = frame.size;
            let mut pass = frame.begin_compute_pass("crossfade", &Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
        }
    })
}

#[test]
fn local_transition_composites_accepted_history_and_current_pixels_in_place() {
    let Some(mut gpu) = WgpuRenderer::new() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let source = NodeId::default();
    let initial = TransitionPaint {
        source,
        mount: PaintMount::default(),
        run: PaintMount::default(),
        revision: 0,
        program: crossfade(),
        progress: 1.0,
        opacity: 1.0,
    };
    let scene = |paint, rect, color| {
        let mut list = picture([
            Item::PushClip { source, rect: Rect::new(0.0, 0.0, 26.0, 20.0), radius: [8.0; 4] },
            Item::PushClip { source, rect: Rect::new(1.0, 1.0, 21.0, 16.0), radius: [0.0; 4] },
            Item::PushLayer { source, opacity: 0.5 },
            Item::PushTransition(paint),
            Item::Quad(solid(rect, color)),
            Item::PopLayer,
            Item::PopLayer,
            Item::PopClip,
            Item::PopClip,
            Item::Quad(solid(Rect::new(12.0, 10.0, 2.0, 2.0), Color::WHITE)),
        ]);
        list.background = rgb(0x00ff00);
        list
    };
    let target = device_target(gpu.device(), wgpu::TextureUsages::empty());
    let submit = |gpu: &mut WgpuRenderer, list: &DisplayList, revision| {
        let (recorded, profile) = profile_frame(gpu, &target, list, 1.0, revision);
        let effects = recorded.paint_receipts().to_vec();
        (recorded.submit_for_presentation().1, effects, profile)
    };
    let sample = |revision| Some(PaintSample { revision, sampled_at: Duration::from_millis(revision) });
    let old_rect = Rect::new(2.0, 2.0, 13.0, 12.0);
    let new_rect = Rect::new(10.0, 2.0, 14.0, 12.0);
    let (pending, effects, _) = submit(&mut gpu, &scene(initial.clone(), old_rect, rgb(0xff0000)), 10);
    assert!(effects.is_empty());
    gpu.accept_presentation(pending);
    // Submitting another settled output is insufficient to publish its pixels.
    drop(submit(&mut gpu, &scene(initial.clone(), old_rect, rgb(0xff00ff)), 11).0);

    let active = TransitionPaint { run: PaintMount::default(), revision: 1, progress: 0.5, ..initial };
    let incoming = scene(active.clone(), new_rect, rgb(0x0000ff));
    let midpoint = PaintOutcome::Transition { old: sample(10).unwrap() };
    let before = gpu.paint_with_receipts(&incoming, 1.0, sample(20)).unwrap();
    assert_eq!(before.effects, [PaintReceipt { source, outcome: PaintOutcome::Fallback { reason: PaintFallback::HistoryUnavailable } }]);
    close(before.picture.pixel(6, 8).unwrap(), [0, 255, 0, 255]);
    let (pending, effects, timing) = submit(&mut gpu, &incoming, 20);
    assert_eq!(effects, [PaintReceipt { source, outcome: midpoint }]);
    assert_eq!(timing.render_passes, 4);
    assert_eq!(timing.backdrop_copy_pixels, 0);
    gpu.accept_presentation(pending);

    let accepted = gpu.paint_with_receipts(&incoming, 1.0, sample(20)).unwrap();
    assert_eq!(accepted.effects, [PaintReceipt { source, outcome: midpoint }]);
    close(accepted.picture.pixel(6, 8).unwrap(), [137, 225, 0, 255]); // Old-only extent.
    close(accepted.picture.pixel(12, 8).unwrap(), [137, 188, 137, 255]); // Both inputs.
    close(accepted.picture.pixel(18, 8).unwrap(), [0, 225, 137, 255]); // New-only extent.
    close(accepted.picture.pixel(2, 2).unwrap(), [118, 234, 0, 255]); // Rounded coverage applied once.
    close(accepted.picture.pixel(22, 8).unwrap(), [0, 255, 0, 255]); // Inner rectangular clip.
    close(accepted.picture.pixel(12, 10).unwrap(), [255; 4]); // Later foreground.

    let revised = scene(active.clone(), new_rect, rgb(0xffff00));
    let capture = gpu.paint_with_receipts(&revised, 2.0, sample(21)).unwrap();
    assert_eq!(capture.effects, accepted.effects);
    close(capture.picture.pixel(12, 16).unwrap(), [137, 225, 0, 255]);
    close(capture.picture.pixel(24, 16).unwrap(), [188, 225, 0, 255]);

    let interrupted = TransitionPaint { run: PaintMount::default(), revision: 2, progress: 0.0, ..active };
    let final_rect = Rect::new(6.0, 4.0, 10.0, 8.0);
    let final_scene = scene(interrupted.clone(), final_rect, rgb(0x0000ff));
    let (pending, effects, _) = submit(&mut gpu, &final_scene, 30);
    assert_eq!(effects, [PaintReceipt { source, outcome: PaintOutcome::Transition { old: sample(20).unwrap() } }]);
    gpu.accept_presentation(pending);
    let capture = gpu.paint_with_receipts(&final_scene, 1.0, sample(30)).unwrap();
    close(capture.picture.pixel(12, 8).unwrap(), accepted.picture.pixel(12, 8).unwrap());

    let broken = transition(|device| {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 4,
            usage: wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        move |frame: &mut CanvasFrame<'_>, _: Transition<'_>| frame.encoder.clear_buffer(&buffer, 0, None)
    });
    let broken_scene = scene(TransitionPaint { program: broken, ..interrupted }, final_rect, rgb(0x0000ff));
    let fallback = gpu.paint_with_receipts(&broken_scene, 1.0, sample(30)).unwrap();
    assert_eq!(fallback.effects, [PaintReceipt { source, outcome: PaintOutcome::Fallback { reason: PaintFallback::EncodingFailed } }]);
    close(fallback.picture.pixel(3, 8).unwrap(), [0, 255, 0, 255]);
    close(fallback.picture.pixel(12, 8).unwrap(), [0, 188, 188, 255]);
}

#[test]
fn author_programs_and_profiles_preserve_renderer_send_and_sync() {
    fn assert_send_and_sync<T: Send + Sync>() {}
    assert_send_and_sync::<WgpuRenderer>();
    assert_send_and_sync::<RecordedFrame<'static>>();
    assert_send_and_sync::<PendingPresentation>();
}

#[test]
fn color_glyphs_keep_their_atlas_color_with_sampled_opacity() {
    use crate::glyphs::{GlyphImage, GlyphKey};
    use octara_model::geometry::point;
    use octara_model::paint::{FontFace, FontId, Glyph, GlyphRun};

    let Some(mut gpu) = WgpuRenderer::new() else {
        assert!(std::env::var_os("OCTARA_REQUIRE_GPU").is_none(), "a GPU adapter is required");
        return;
    };
    let run = GlyphRun {
        source: NodeId::default(),
        font: FontFace { id: FontId(42), data: (&[] as &[u8]).into(), index: 0 },
        size: 16.0,
        coords: vec![],
        embolden: false,
        skew: 0.0,
        color: Color::WHITE,
        colors: Some([rgb(0xff0000).with_alpha(128.0 / 255.0)].into()),
        origin: point(4.0, 5.0),
        glyphs: [Glyph { id: 1, x: 0.0, y: 0.0, source_end: 0 }].into(),
    };
    let mut textures = SceneTextures::default();
    textures.glyphs.insert_test_glyph(
        GlyphKey::new(&run, 1, run.size, 0),
        GlyphImage { left: 0, top: 0, width: 1, height: 1, color: true, data: [13, 160, 227, 128].into() },
    );
    gpu.textures = Some(textures);
    let pixels = gpu.render(&picture([Item::Glyphs(run)]), 1.0);
    close(pixels.pixel(4, 5).unwrap(), [13, 160, 227, 64]);
    assert_eq!(pixels.pixel(5, 5), Some([0; 4]));
}

#[test]
fn materials_batch_in_painter_order_with_per_instance_params() {
    let Some(mut gpu) = WgpuRenderer::new() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    const SHADER: &str = "fn material(uv: vec2f, params: vec4f) -> vec4f { return params; }";
    let custom = |rect: Rect, program, params| Item::Custom(program_box(rect, program, params));
    let dl = picture([
        Item::PushClip { source: NodeId::default(), rect: Rect::new(2.0, 2.0, 17.0, 13.0), radius: [5.0; 4] },
        custom(Rect::new(2.0, 2.0, 13.0, 12.0), material(SHADER), [1.0, 0.0, 0.0, 1.0]),
        custom(Rect::new(8.0, 2.0, 9.0, 13.0), material(SHADER), [0.0, 1.0, 0.0, 0.5]),
        Item::PopClip,
        Item::Quad(solid(Rect::new(10.0, 6.0, 4.0, 8.0), rgb(0x0000ff))),
        custom(Rect::new(20.0, 2.0, 8.0, 12.0), material(SHADER), [0.0, 0.5, 1.0, 1.0]),
        custom(Rect::new(20.0, 15.0, 8.0, 6.0), material("invalid WGSL"), [1.0; 4]),
        custom(Rect::new(2.0, 16.0, 8.0, 6.0), PaintProgram::new(()), [1.0; 4]),
    ]);
    let output = TargetInfo { extent: [32, 24], scale: 1.0, max_texture_dimension: 4096, copyable: true, encode_output: false };
    let FramePlan { mut passes, mut requests, resources, .. } = plan::plan_frame(&dl, output);
    assert!(requests.canvases.is_empty() && requests.backdrops.is_empty());
    crate::block_on(gpu.prepare_resources(&mut passes, resources, &mut SceneTextures::default(), WORKING_FORMAT, &mut requests.receipts))
        .unwrap();
    let material = *material(SHADER).downcast_ref::<plan::Material>().unwrap();
    assert_eq!(
        passes[0].batches,
        [
            Batch { instances: 0..2, draw: Draw::Material(material) },
            Batch { instances: 2..3, draw: Draw::Quad },
            Batch { instances: 3..4, draw: Draw::Material(material) },
            Batch { instances: 4..6, draw: Draw::Quad },
        ]
    );
    let frame = gpu.render(&dl, 1.0);
    close(frame.pixel(2, 2).unwrap(), [0, 0, 0, 0]); // Rounded ancestor clip.
    close(frame.pixel(6, 8).unwrap(), [255, 0, 0, 255]);
    close(frame.pixel(8, 8).unwrap(), [188, 188, 0, 255]); // Blend in instance order.
    close(frame.pixel(12, 8).unwrap(), [0, 0, 255, 255]); // Ordinary quad covers both materials.
    close(frame.pixel(16, 8).unwrap(), [0, 255, 0, 128]); // Straight-alpha output.
    close(frame.pixel(22, 8).unwrap(), [0, 188, 255, 255]);
    close(frame.pixel(22, 18).unwrap(), [255, 0, 255, 255]); // Failed shader.
    close(frame.pixel(5, 18).unwrap(), [255, 0, 255, 255]); // Unrecognized back end.
}

#[test]
fn canvas_uses_clip_group_opacity_frame_params_and_validation_fallback() {
    let Some(mut gpu) = WgpuRenderer::new() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let preparations = Arc::new(AtomicUsize::new(0));
    let program = canvas({
        let preparations = preparations.clone();
        move |device| {
            preparations.fetch_add(1, Ordering::Relaxed);
            let copy_source = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("test canvas copy source"),
                size: 4,
                usage: wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            move |frame: &mut CanvasFrame<'_>| {
                assert_eq!(frame.size, [(21.0 * frame.scale) as u32; 2]);
                if frame.input.params[3] < 0.0 {
                    frame.encoder.clear_buffer(&copy_source, 0, None);
                    return;
                }
                if frame.input.params[3] > 0.0 {
                    let [r, g, b, a] = frame.input.params.map(f64::from);
                    drop(frame.begin_render_pass(
                        "test_canvas_fill",
                        &wgpu::RenderPassDescriptor {
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: frame.target,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a }),
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            ..Default::default()
                        },
                    ));
                }
            }
        }
    });
    let draw = |params: [f32; 4]| {
        let mut dl = picture([
            Item::PushClip { source: NodeId::default(), rect: Rect::new(3.0, 3.0, 16.0, 16.0), radius: [6.0; 4] },
            Item::PushLayer { source: NodeId::default(), opacity: 0.5 },
            Item::Custom(CustomPaint {
                fallback: solid(Rect::new(1.0, 1.0, 21.0, 21.0), rgb(0x00ff00)),
                program: program.clone(),
                input: params.into(),
                opacity: 1.0,
                mount: PaintMount::default(),
                exiting: false,
            }),
            Item::Quad(solid(Rect::new(10.0, 8.0, 4.0, 4.0), Color::BLACK)),
            Item::PopLayer,
            Item::PopClip,
            Item::Quad(solid(Rect::new(14.0, 7.0, 4.0, 6.0), rgb(0x0000ff))),
        ]);
        dl.background = Color::WHITE;
        dl
    };
    for scale in [1, 2] {
        let frame = gpu.render(&draw([0.5, 0.0, 0.0, 0.5]), scale as f32);
        let pixel = |x: u32, y: u32| frame.pixel(x * scale, y * scale).unwrap();
        close(pixel(3, 3), [255; 4]); // Rounded clip corner, inside the canvas's own box.
        close(pixel(8, 10), [255, 225, 225, 255]); // Alpha is applied in linear light.
        close(pixel(11, 10), [188, 188, 188, 255]); // The group's black quad fully covers the canvas.
        close(pixel(15, 10), [0, 0, 255, 255]); // Quad after the layer.
        let empty = gpu.render(&draw([0.0; 4]), scale as f32); // Reuses a cleared allocation.
        close(empty.pixel(8 * scale, 10 * scale).unwrap(), [255; 4]);
    }
    let failed = gpu.render(&draw([0.0, 0.0, 0.0, -1.0]), 2.0);
    close(failed.pixel(16, 20).unwrap(), [188, 255, 188, 255]);
    close(failed.pixel(30, 20).unwrap(), [0, 0, 255, 255]);
    assert_eq!(preparations.load(Ordering::Relaxed), 1);
}

#[test]
fn backdrops_read_the_painter_prefix_and_keep_surrounding_commands_in_order() {
    let Some(mut gpu) = WgpuRenderer::new() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let blue = canvas(|_| {
        |frame: &mut CanvasFrame<'_>| {
            drop(color_pass(frame.encoder, frame.target, wgpu::LoadOp::Clear(wgpu::Color::BLUE), None));
        }
    });
    let transfer = backdrop(|_| {
        |frame: &mut CanvasFrame<'_>, sample: Backdrop<'_>| {
            assert_eq!((sample.size, sample.offset), ([14, 12], [0, 0]));
            let [x, y, tx, ty] = frame.input.params.map(|value| value as u32);
            copy_sample(frame, &sample, [x, y], [tx, ty], [1, 1]);
        }
    });
    let rect = Rect::new(0.0, 0.0, 14.0, 12.0);
    let dl = picture([
        Item::Custom(program_box(Rect::new(2.0, 2.0, 4.0, 4.0), blue, [0.0; 4])),
        Item::Custom(program_box(rect, transfer.clone(), [3.0, 3.0, 8.0, 7.0])),
        Item::Custom(program_box(rect, transfer, [8.0, 7.0, 10.0, 9.0])),
        Item::Quad(solid(Rect::new(2.0, 2.0, 4.0, 4.0), rgb(0xff0000))),
    ]);
    let device = gpu.device().clone();
    let snapshot = color_texture(&device, "backdrop test readback", wgpu::TextureFormat::Rgba8UnormSrgb, [32, 24]);
    let snapshot_view = snapshot.create_view(&Default::default());
    let blit = resolve::Blit::new(&device, snapshot.format(), OutputColor::SrgbOpaque);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 16,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let copy_pixel = |encoder: &mut wgpu::CommandEncoder, x, y, offset| {
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { origin: wgpu::Origin3d { x, y, z: 0 }, ..snapshot.as_image_copy() },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout { offset, bytes_per_row: None, rows_per_image: None },
            },
            wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        );
    };
    let expected = [[0, 255, 0, 255], [255, 0, 0, 255], [0, 0, 255, 255], [0, 0, 255, 255]];
    for copyable in [true, false, true] {
        use wgpu::TextureUsages as Usage;
        let usage = if copyable { Usage::TEXTURE_BINDING | Usage::COPY_SRC } else { Usage::TEXTURE_BINDING };
        let texture = device_target(&device, usage);
        let view = texture.create_view(&Default::default());
        let bind = blit.bind(&device, &view);
        let mut encoder = device.create_command_encoder(&Default::default());
        drop(color_pass(&mut encoder, &view, wgpu::LoadOp::Clear(wgpu::Color::GREEN), None));
        // Read the final attachment through sampling, even when it has no COPY_SRC usage.
        blit.encode(&mut encoder, &bind, &snapshot_view, None);
        copy_pixel(&mut encoder, 3, 3, 0);
        let input = FrameInput { list: &dl, scale: 1.0, sample: None };
        let (mut frame, profile) = gpu.record(encoder, linear_target(&texture), input, Some(Box::new(|_| {}))).unwrap();
        let profile = profile.unwrap();
        assert!(frame.paint_receipts().iter().all(|receipt| receipt.outcome == PaintOutcome::Rendered));
        assert_eq!(profile.render_passes, if copyable { 5 } else { 6 });
        assert_eq!(profile.output_pixels, if copyable { 0 } else { 32 * 24 });
        blit.encode(frame.encoder(), &bind, &snapshot_view, None);
        copy_pixel(frame.encoder(), 3, 3, 4);
        copy_pixel(frame.encoder(), 8, 7, 8);
        copy_pixel(frame.encoder(), 10, 9, 12);
        let submission = frame.submit();
        readback.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
        device.poll(wgpu::PollType::Wait { submission_index: Some(submission), timeout: None }).unwrap();
        assert_eq!(&*readback.get_mapped_range(..).unwrap(), expected.as_flattened(), "copyable target: {copyable}");
        readback.unmap();
    }
}

#[test]
fn backdrop_crops_to_nested_clips_and_the_resized_attachment() {
    let Some(mut gpu) = WgpuRenderer::new() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let transfer = backdrop(|_| {
        |frame: &mut CanvasFrame<'_>, sample: Backdrop<'_>| {
            let scale = frame.scale as u32;
            let size = |pixels: [u32; 2]| pixels.map(|axis| axis * scale);
            assert_eq!(frame.size, size([21, 19]));
            assert_eq!(sample.size, size([frame.input.params[0] as u32, 13]));
            assert_eq!(sample.offset, size([6, 4]).map(|axis| axis as i32));
            copy_sample(frame, &sample, size([2, 4]), size([14, 9]), size([3, 3]));
            copy_sample(frame, &sample, size([2, 4]), size([6, 4]), size([3, 3]));
        }
    });
    for (width, scale, crop_width) in [(160.0, 1_u32, 15.0), (88.0, 2, 11.0)] {
        let mut dl = picture([
            Item::PushClip { source: NodeId::default(), rect: Rect::new(75.0, 11.0, 18.0, 20.0), radius: [0.0; 4] },
            Item::PushLayer { source: NodeId::default(), opacity: 0.5 },
            Item::Quad(solid(Rect::new(74.0, 9.0, 21.0, 24.0), rgb(0x00ff00))),
            Item::PushClip { source: NodeId::default(), rect: Rect::new(77.0, 10.0, 15.0, 14.0), radius: [4.0; 4] },
            Item::PushLayer { source: NodeId::default(), opacity: 0.5 },
            Item::Quad(solid(Rect::new(79.0, 15.0, 4.0, 4.0), rgb(0xff0000))),
            Item::Custom(program_box(Rect::new(71.25, 7.25, 21.0, 19.0), transfer.clone(), [crop_width, 0.0, 0.0, 0.0])),
            Item::Quad(solid(Rect::new(79.0, 15.0, 4.0, 4.0), rgb(0x0000ff))),
            Item::PopLayer,
            Item::PopClip,
            Item::PopLayer,
            Item::PopClip,
        ]);
        dl.size = (width, 64.0).into();
        dl.background = Color::WHITE;
        let frame = gpu.render(&dl, scale as f32);
        let pixel = |x, y| frame.pixel(x * scale, y * scale).unwrap();
        close(pixel(74, 15), [255; 4]);
        close(pixel(76, 11), [188, 255, 188, 255]);
        close(pixel(80, 16), [188, 225, 225, 255]);
        close(pixel(86, 17), [225, 225, 188, 255]);
    }
}

#[test]
fn backdrop_exit_replays_only_the_accepted_image_of_its_mount() {
    let Some(mut gpu) = WgpuRenderer::new() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let draws = Arc::new(AtomicUsize::new(0));
    let transfer = backdrop({
        let draws = draws.clone();
        move |device| {
            let invalid_clear = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 4,
                usage: wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let draws = draws.clone();
            move |frame: &mut CanvasFrame<'_>, sample: Backdrop<'_>| {
                draws.fetch_add(1, Ordering::Relaxed);
                if frame.input.params[0] < 0.0 {
                    frame.encoder.clear_buffer(&invalid_clear, 0, None);
                } else {
                    copy_sample(frame, &sample, [0, 0], [0, 0], sample.size);
                }
            }
        }
    });
    let mount = PaintMount::default();
    let replacement = PaintMount::default();
    let effect = |mount: PaintMount, x, exiting, params| {
        Item::Custom(CustomPaint { mount, exiting, ..program_box(Rect::new(x, 2.0, 8.0, 8.0), transfer.clone(), params) })
    };
    let live = |background, params| {
        let mut dl = picture([effect(mount.clone(), 2.0, false, params)]);
        dl.background = rgb(background);
        dl
    };
    let device = gpu.device().clone();
    let target = device_target(&device, wgpu::TextureUsages::COPY_SRC);
    let encoder = || device.create_command_encoder(&Default::default());
    fn sampled(list: &DisplayList, revision: u64) -> FrameInput<'_> {
        FrameInput { list, scale: 1.0, sample: Some(PaintSample { revision, sampled_at: Duration::from_millis(revision) }) }
    }
    for (background, revision) in [(0xffff00, 10), (0xff0000, 20)] {
        let (frame, _) = gpu.record(encoder(), linear_target(&target), sampled(&live(background, [0.0; 4]), revision), None).unwrap();
        assert_eq!(frame.paint_receipts()[0].outcome, PaintOutcome::Rendered);
        let (_, pending) = frame.submit_for_presentation();
        gpu.accept_presentation(pending);
    }
    // The deferred frame can use the previous allocation. Subsequent frames must leave
    // those pixels available until the host accepts it.
    let (frame, _) = gpu.record(encoder(), linear_target(&target), sampled(&live(0x00ff00, [0.0; 4]), 30), None).unwrap();
    assert_eq!(frame.paint_receipts()[0].outcome, PaintOutcome::Rendered);
    let (_, pending) = frame.submit_for_presentation();
    let (frame, _) = gpu.record(encoder(), linear_target(&target), sampled(&live(0x0000ff, [0.0; 4]), 31), None).unwrap();
    assert_eq!(frame.paint_receipts()[0].outcome, PaintOutcome::Rendered);
    frame.submit();
    drop(gpu.record(encoder(), linear_target(&target), sampled(&live(0xffff00, [0.0; 4]), 32), None).unwrap());
    let capture = gpu.render(&live(0x0000ff, [0.0; 4]), 1.0);
    close(capture.pixel(5, 5).unwrap(), [0, 0, 255, 255]);
    let render_only = device_target(&device, wgpu::TextureUsages::empty());
    let (frame, _) = gpu.record(encoder(), linear_target(&render_only), sampled(&live(0x000000, [0.0; 4]), 40), None).unwrap();
    assert_eq!(frame.paint_receipts()[0].outcome, PaintOutcome::Rendered);
    // An intermediate attachment still needs host acceptance to publish an output.
    frame.submit();
    let mut failing = live(0x00ff00, [-1.0; 4]);
    failing.parts.push(Part::Item(Item::Quad(solid(Rect::new(8.0, 3.0, 1.0, 2.0), rgb(0x0000ff)))));
    let failure = gpu.render(&failing, 1.0);
    close(failure.pixel(5, 5).unwrap(), [255, 0, 255, 255]);
    close(failure.pixel(8, 3).unwrap(), [0, 0, 255, 255]);
    let (frame, _) = gpu.record(encoder(), linear_target(&target), sampled(&failing, 50), None).unwrap();
    assert_eq!(frame.paint_receipts()[0].outcome, PaintOutcome::Fallback { reason: PaintFallback::EncodingFailed });
    let (_, failed) = frame.submit_for_presentation();
    gpu.accept_presentation(failed);
    let offscreen = gpu.render(&picture([effect(mount.clone(), 34.0, false, [0.0; 4])]), 1.0);
    close(offscreen.pixel(31, 5).unwrap(), [0; 4]);

    let exiting = picture([
        Item::PushLayer { source: NodeId::default(), opacity: 0.5 },
        effect(mount.clone(), 2.0, true, [0.0; 4]),
        effect(replacement, 13.0, true, [0.0; 4]),
        Item::PopLayer,
    ]);
    let exit = gpu.paint_with_receipts(&exiting, 1.0, Some(PaintSample { revision: 60, sampled_at: Duration::from_millis(60) })).unwrap();
    close(exit.picture.pixel(5, 5).unwrap(), [255, 0, 0, 128]);
    close(exit.picture.pixel(16, 5).unwrap(), [255, 0, 255, 128]);
    assert_eq!(exit.effects[0].outcome, PaintOutcome::Retained { revision: Some(20) });
    assert_eq!(exit.effects[1].outcome, PaintOutcome::Fallback { reason: PaintFallback::HistoryUnavailable });
    gpu.accept_presentation(pending);
    let exit = gpu.paint_with_receipts(&exiting, 1.0, None).unwrap();
    close(exit.picture.pixel(5, 5).unwrap(), [0, 255, 0, 128]);
    assert_eq!(exit.effects[0].outcome, PaintOutcome::Retained { revision: Some(30) });
    assert_eq!(draws.load(Ordering::Relaxed), 9);
    drop(exiting);
    drop(failing);
    drop(mount);
    gpu.render(&picture([]), 1.0);
    assert!(gpu.presentation_history.is_empty());
}

#[test]
fn profiled_frames_measure_recorded_work_and_read_back_asynchronously() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let Ok(adapter) = crate::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())) else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let features = adapter.features() & (wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS);
    let Ok((device, queue)) =
        crate::block_on(adapter.request_device(&wgpu::DeviceDescriptor { required_features: features, ..Default::default() }))
    else {
        eprintln!("no GPU device: skipped");
        return;
    };
    let mut gpu = WgpuRenderer::with_device(device.clone(), queue);
    let timestamps_supported = features.contains(wgpu::Features::TIMESTAMP_QUERY);
    if timestamps_supported {
        let mut held: Vec<_> = (0..3).map(|_| gpu.timestamps.begin(&device, &gpu.queue).unwrap()).collect();
        assert!(gpu.timestamps.begin(&device, &gpu.queue).is_none());
        drop(held.pop());
        assert!(gpu.timestamps.begin(&device, &gpu.queue).is_some());
        drop(held);
    }
    let target = device_target(&device, wgpu::TextureUsages::COPY_SRC);
    let quad = |x, y| Item::Quad(solid(Rect::new(x, y, 4.0, 4.0), Color::WHITE));
    let material = material("fn material(uv: vec2f, params: vec4f) -> vec4f { return params; }");
    let render = |frame: &mut CanvasFrame<'_>, name| {
        drop(frame.begin_render_pass(
            name,
            &wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: frame.target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            },
        ));
    };
    let canvas_program = canvas(move |device| {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl("@compute @workgroup_size(1) fn main() {}".into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        move |frame: &mut CanvasFrame<'_>| {
            {
                let mut pass = frame.begin_compute_pass("test_compute", &Default::default());
                pass.set_pipeline(&pipeline);
                pass.dispatch_workgroups(1, 1, 1);
            }
            render(frame, "test_render");
        }
    });
    let failing_canvas = canvas(move |device| {
        let source = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 4,
            usage: wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        move |frame: &mut CanvasFrame<'_>| {
            render(frame, "test_discarded");
            frame.encoder.clear_buffer(&source, 0, None);
        }
    });
    let backdrop =
        backdrop(|_| |frame: &mut CanvasFrame<'_>, sample: Backdrop<'_>| copy_sample(frame, &sample, [0, 0], [0, 0], sample.size));
    let material_box = |x| Item::Custom(program_box(Rect::new(x, 6.0, 4.0, 4.0), material.clone(), [1.0; 4]));
    let effect = program_box(Rect::new(3.0, 4.0, 8.0, 8.0), backdrop, [0.0; 4]);
    let dl = picture([
        quad(0.0, 0.0),
        Item::PushLayer { source: NodeId::default(), opacity: 0.5 },
        quad(10.0, 0.0),
        Item::PopLayer,
        material_box(0.0),
        material_box(5.0),
        Item::Custom(program_box(Rect::new(2.0, 2.0, 4.0, 4.0), canvas_program, [0.0; 4])),
        Item::Custom(effect.clone()),
        quad(0.0, 0.0),
        Item::Custom(program_box(Rect::new(2.0, 2.0, 4.0, 4.0), failing_canvas, [0.0; 4])),
    ]);
    let encoder = || device.create_command_encoder(&Default::default());
    let input = FrameInput { list: &dl, scale: 1.0, sample: None };
    let (discard, _) =
        gpu.record(encoder(), linear_target(&target), input, Some(Box::new(|_| panic!("discarded frame reported a GPU sample")))).unwrap();
    drop(discard);
    assert!(!gpu.profile_pending());

    let (send, receive) = std::sync::mpsc::channel();
    let (frame, profile) =
        gpu.record(encoder(), linear_target(&target), input, Some(Box::new(move |sample| send.send(sample).unwrap()))).unwrap();
    let profile = profile.expect("a profiled frame");
    assert_eq!((profile.draw_batches, profile.material_batches, profile.render_passes), (7, 1, 5));
    assert_eq!((profile.layer_pixels, profile.canvas_pixels, profile.backdrop_copy_pixels), (768, 80, 64));
    assert_eq!(profile.retained_backdrops, 0);
    assert_eq!(profile.gpu_sampled, timestamps_supported);
    let (_, pending) = frame.submit_for_presentation();
    gpu.accept_presentation(pending);
    let deadline = Instant::now() + Duration::from_secs(5);
    while gpu.poll_profile() {
        assert!(Instant::now() < deadline, "timestamp mapping did not complete");
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    }
    if profile.gpu_sampled {
        let sample = receive.try_recv().expect("submitted frame reported GPU timings");
        eprintln!("{:?} GPU timings: {sample:?}", adapter.get_info().backend);
        let kinds: Vec<_> = sample.passes.iter().map(|pass| pass.kind).collect();
        assert_eq!(kinds.iter().filter(|&&kind| kind == GpuPassKind::Ui).count(), 4);
        assert!(kinds.contains(&GpuPassKind::Layer));
        if features.contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS) {
            assert_eq!(
                kinds,
                [
                    GpuPassKind::Canvas,
                    GpuPassKind::Author("test_compute"),
                    GpuPassKind::Author("test_render"),
                    GpuPassKind::Layer,
                    GpuPassKind::Ui, // Opaque cores of the prefix.
                    GpuPassKind::Ui,
                    GpuPassKind::BackdropCopy,
                    GpuPassKind::Backdrop,
                    GpuPassKind::Ui, // Backdrop composite.
                    GpuPassKind::Ui  // Opaque cores of the next segment.
                ]
            );
            assert!(sample.span.is_some());
        } else {
            assert_eq!(
                kinds,
                [
                    GpuPassKind::CanvasClear,
                    GpuPassKind::Author("test_compute"),
                    GpuPassKind::Author("test_render"),
                    GpuPassKind::Layer,
                    GpuPassKind::Ui, // Opaque cores of the prefix.
                    GpuPassKind::Ui,
                    GpuPassKind::BackdropClear,
                    GpuPassKind::Ui, // Backdrop composite.
                    GpuPassKind::Ui  // Opaque cores of the next segment.
                ]
            );
            assert!(sample.span.is_none());
        }
        assert!(!sample.truncated);
    } else {
        assert!(receive.try_recv().is_err());
    }

    let exit = picture([Item::Custom(CustomPaint { exiting: true, ..effect })]);
    let input = FrameInput { list: &exit, scale: 1.0, sample: None };
    let (frame, profile) = gpu.record(encoder(), linear_target(&target), input, Some(Box::new(|_| {}))).unwrap();
    let profile = profile.expect("a profiled frame");
    assert_eq!((profile.retained_backdrops, profile.backdrop_copy_pixels, profile.canvas_pixels), (1, 0, 0));
    drop(frame);
    assert!(!gpu.profile_pending());
}

#[test]
fn metal_profiles_share_following_work_and_flush_when_idle() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let Ok(adapter) = crate::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())) else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    if adapter.get_info().backend != wgpu::Backend::Metal || !adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
        return;
    }
    let (device, queue) = crate::block_on(
        adapter.request_device(&wgpu::DeviceDescriptor { required_features: wgpu::Features::TIMESTAMP_QUERY, ..Default::default() }),
    )
    .unwrap();
    let mut gpu = WgpuRenderer::with_device(device.clone(), queue);
    let target = device_target(&device, wgpu::TextureUsages::empty());
    let list = picture([Item::Quad(solid(Rect::new(2.0, 2.0, 8.0, 8.0), Color::WHITE))]);
    let wait = |submission_index| {
        device.poll(wgpu::PollType::Wait { submission_index, timeout: Some(Duration::from_secs(2)) }).unwrap();
    };
    let profile = |gpu: &mut WgpuRenderer| {
        let (send, receive) = mpsc::channel();
        let encoder = device.create_command_encoder(&Default::default());
        let input = FrameInput { list: &list, scale: 1.0, sample: None };
        let (frame, profile) =
            gpu.record(encoder, linear_target(&target), input, Some(Box::new(move |sample| send.send(sample).unwrap()))).unwrap();
        assert!(profile.unwrap().gpu_sampled);
        wait(Some(frame.submit()));
        assert!(gpu.profile_resolves_ready());
        assert!(!gpu.profile_needs_poll());
        receive
    };

    let first = profile(&mut gpu);
    assert!(first.try_recv().is_err());
    let green = picture([Item::Quad(solid(Rect::new(2.0, 2.0, 8.0, 8.0), rgb(0x00ff00)))]);
    let mut capture = gpu.record_capture(&green, 1.0, None).unwrap().submit().unwrap();
    assert!(!gpu.profile_resolves_ready());
    wait(Some(capture.submission().clone()));
    let captured = capture.take_mapped().unwrap().unwrap().finish().unwrap();
    assert_eq!(captured.picture.pixel(4, 4).unwrap(), [0, 255, 0, 255]);
    assert_eq!(first.try_recv().expect("the following capture carried the resolve").passes[0].kind, GpuPassKind::Ui);

    let idle = profile(&mut gpu);
    assert_eq!(gpu.flush_profile_resolves().unwrap(), 1);
    assert_eq!(gpu.flush_profile_resolves().unwrap(), 0);
    wait(None);
    assert_eq!(idle.try_recv().expect("the idle flush delivered the sample").passes[0].kind, GpuPassKind::Ui);

    let rejected = profile(&mut gpu);
    let (commands, error) = custom::with_error_scopes(&device, || {
        let mut encoder = device.create_command_encoder(&Default::default());
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 4,
            usage: wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        encoder.clear_buffer(&buffer, 0, None);
        encoder.finish()
    });
    assert!(error.is_some());
    assert!(gpu.submit_commands([commands]).is_err());
    wait(None);
    assert!(matches!(rejected.try_recv(), Err(mpsc::TryRecvError::Disconnected)));
    assert!(!gpu.profile_pending());
}

#[test]
fn offscreen_readbacks_keep_their_own_pixels_and_receipts() {
    let Some(mut gpu) = WgpuRenderer::new() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let paint = program_box(Rect::new(0.0, 0.0, 4.0, 4.0), PaintProgram::new(()), [0.0; 4]);
    let source = paint.fallback.source;
    let red = DisplayList { background: rgb(0xff0000), ..picture([Item::Custom(paint)]) };
    let blue = DisplayList { background: rgb(0x0000ff), ..picture([]) };
    let mut first = gpu.record_capture(&red, 1.0, None).unwrap().submit().unwrap();
    let mut second = gpu.record_capture(&blue, 2.0, None).unwrap().submit().unwrap();

    gpu.device().poll(wgpu::PollType::Wait { submission_index: Some(second.submission().clone()), timeout: None }).unwrap();
    let blue = second.take_mapped().expect("second buffer mapped").unwrap().finish().unwrap();
    assert_eq!((blue.picture.width(), blue.picture.height()), (64, 48));
    assert_eq!(blue.picture.pixel(63, 47).unwrap(), [0, 0, 255, 255]);
    assert!(blue.effects.is_empty());
    let red = first.take_mapped().expect("first buffer mapped").unwrap().finish().unwrap();
    assert_eq!((red.picture.width(), red.picture.height()), (32, 24));
    assert_eq!(red.picture.pixel(31, 23).unwrap(), [255, 0, 0, 255]);
    assert_eq!(red.effects, [PaintReceipt { source, outcome: PaintOutcome::Fallback { reason: PaintFallback::UnsupportedProgram } }]);
    assert!(first.take_mapped().is_none());
    assert!(second.take_mapped().is_none());
}

mod damage;

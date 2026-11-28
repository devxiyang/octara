use super::*;
use crate::gpu::recording::FrameRecording;
use octara_model::geometry::{Size, Vec2};
use octara_model::identity::NativeId;
use octara_model::native::NativeSlot;
use octara_model::paint::{Image, ImagePaint, ScrollPlacement, ScrollPlacements, Shadow};

const EXTENT: [u32; 2] = [4632, 2560];

fn target(device: &wgpu::Device, extent: [u32; 2], copy: bool) -> wgpu::Texture {
    let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC;
    if copy {
        usage |= wgpu::TextureUsages::COPY_DST;
    }
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("damage test drawable"),
        size: wgpu::Extent3d { width: extent[0], height: extent[1], depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8UnormSrgb,
        usage,
        view_formats: &[],
    })
}

fn record<'a>(
    gpu: &'a mut GpuRenderer,
    texture: &wgpu::Texture,
    list: &DisplayList,
    color: OutputColor,
) -> (RecordedFrame<'a>, FrameProfile) {
    let encoder = gpu.device().create_command_encoder(&Default::default());
    let sample = Some(PaintSample { revision: 1, sampled_at: Duration::ZERO });
    let target = FrameTarget { texture, color };
    let (frame, profile) = gpu.record(encoder, target, FrameInput { list, scale: 2.0, sample }, Some(Box::new(|_| {}))).unwrap();
    (frame, profile.unwrap())
}

fn pixels(device: &wgpu::Device, mut frame: RecordedFrame<'_>, texture: &wgpu::Texture) -> Vec<u8> {
    let row = texture.width() * 4;
    let stride = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("damage test pixels"),
        size: u64::from(stride) * u64::from(texture.height()),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    frame.encoder().copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(stride), rows_per_image: None },
        },
        texture.size(),
    );
    let submission = frame.submit();
    buffer.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
    device.poll(wgpu::PollType::Wait { submission_index: Some(submission), timeout: None }).unwrap();
    let mapped = buffer.get_mapped_range(..).unwrap();
    mapped.chunks_exact(stride as usize).flat_map(|row_data| &row_data[..row as usize]).copied().collect()
}

fn full_pixels(gpu: &mut GpuRenderer, device: &wgpu::Device, texture: &wgpu::Texture, list: &DisplayList, color: OutputColor) -> Vec<u8> {
    let encoder = device.create_command_encoder(&Default::default());
    let output = FrameTarget { texture, color };
    let input = FrameInput { list, scale: 2.0, sample: None };
    let frame = pollster::block_on(gpu.record_frame(encoder, output, input, FrameRecording::default())).unwrap();
    pixels(device, frame, texture)
}

fn parity(current: &[u8], full: &[u8], width: u32) {
    assert_eq!(current.len(), full.len());
    if let Some((i, (current, full))) =
        current.as_chunks::<4>().0.iter().zip(full.as_chunks::<4>().0).enumerate().find(|(_, (a, b))| a != b)
    {
        panic!("damage mismatch at ({}, {}): {current:?} vs {full:?}", i % width as usize, i / width as usize);
    }
}

#[test]
fn five_k_damage_matches_full_raster_for_caret_hover_and_sidebar_scroll() {
    let Some(mut incremental) = GpuRenderer::new() else { return };
    let device = incremental.device().clone();
    let mut reference = GpuRenderer::with_device(device.clone(), incremental.queue().clone());
    let source = NodeId::default();
    let clip = Rect::new(0.0, 0.0, 260.0, 1280.0);
    let sidebar: Arc<[Part]> = Arc::from([
        Part::Item(Item::PushClip { rect: clip, radius: [9.25; 4], source }),
        Part::ScrollContent {
            source,
            spatial_id: 17,
            scrollbar_offset: Vec2::default(),
            parts: (0..96)
                .map(|i| {
                    let mut row = solid(Rect::new(1.75, i as f32 * 17.0 + 1.25, 255.0, 14.5), rgb(0x285d8c).with_alpha(0.63));
                    row.radius = [4.75; 4];
                    Part::Item(Item::Quad(row))
                })
                .collect(),
        },
        Part::Item(Item::PopClip),
    ]);
    let parts = |caret: bool, hover: bool| {
        let rect = Rect::new(399.25, 53.75, 120.75, 16.5);
        let mut row = solid(rect, rgb(0x8c7cb1).with_alpha(if hover { 0.63 } else { 0.31 }));
        row.radius = [7.25; 4];
        let shadow = Part::Item(Item::Shadow(Shadow { source, rect, radius: [7.25; 4], color: Color::BLACK.with_alpha(0.65), blur: 6.5 }));
        let caret_part = Part::Item(Item::Quad(solid(Rect::new(475.25, 56.5, 1.0, 11.5), Color::WHITE.with_alpha(0.61))));
        Arc::from([
            Part::Group(sidebar.clone()),
            Part::Group(Arc::from([shadow, Part::Item(Item::Quad(row))])),
            Part::Group(if caret { Arc::from([caret_part]) } else { Arc::from([]) }),
        ])
    };
    let scene = |parts: Arc<[Part]>, offset: f32, background: Color| {
        let mut scrolls = ScrollPlacements::default();
        scrolls.set(source, Some(ScrollPlacement { offset: Vec2::new(0.0, offset), scrollbars: [None, None] }));
        DisplayList { size: Size::new(2316.0, 1280.0), background, parts: vec![Part::ScrollFrame { scrolls, parts }] }
    };
    for (copy, color, background) in
        [(true, OutputColor::SrgbOpaque, rgb(0x179a8c)), (false, OutputColor::SrgbPremultiplied, rgb(0x562963).with_alpha(0.37))]
    {
        incremental.set_target_generation(if copy { 1 } else { 2 });
        let reference_target = target(&device, EXTENT, copy);
        let first = scene(parts(true, false), 0.0, background);
        let first_target = target(&device, EXTENT, copy);
        let (frame, profile) = record(&mut incremental, &first_target, &first, color);
        assert_eq!(profile.damage_area_ratio, 1.0);
        frame.submit();
        let hover_parts = parts(false, true);
        let blinking = scene(parts(false, false), 0.0, background);
        let hovered = scene(hover_parts.clone(), 0.0, background);
        let scrolled = scene(hover_parts, -35.6, background);
        let discarded_target = target(&device, EXTENT, copy);
        let (discarded, _) = record(&mut incremental, &discarded_target, &hovered, color);
        drop(discarded);
        for (action, list, range) in
            [("caret", &blinking, 0.0..0.0001), ("hover", &hovered, 0.0001..0.002), ("sidebar", &scrolled, 0.11..0.12)]
        {
            let drawable = target(&device, EXTENT, copy);
            let (frame, profile) = record(&mut incremental, &drawable, list, color);
            eprintln!("5K {color:?} {action}: damage_area_ratio={:.12}, draw_batches={}", profile.damage_area_ratio, profile.draw_batches);
            assert!(profile.partial_raster && profile.damage_area_ratio > 0.0 && range.contains(&profile.damage_area_ratio), "{profile:?}");
            if std::ptr::eq(list, &scrolled) {
                assert!(profile.retained_prepare);
                assert_eq!(profile.spatial_upload_bytes, 16);
            }
            let current = pixels(&device, frame, &drawable);
            let full = full_pixels(&mut reference, &device, &reference_target, list, color);
            parity(&current, &full, EXTENT[0]);
        }
        let drawable = target(&device, EXTENT, copy);
        let (frame, reuse) = record(&mut incremental, &drawable, &scrolled, color);
        assert_eq!((reuse.damage_area_ratio, reuse.draw_batches), (0.0, 0));
        assert!(reuse.partial_raster);
        frame.submit();
    }
}

#[test]
fn damage_invalidates_for_large_edits_targets_and_complex_effects() {
    let Some(mut gpu) = GpuRenderer::new() else { return };
    let device = gpu.device().clone();
    let base = picture([]);
    let large = picture([Item::Quad(solid(Rect::new(0.0, 0.0, 32.0, 24.0), Color::WHITE))]);
    let layered = picture([
        Item::PushLayer { source: NodeId::default(), opacity: 0.5 },
        Item::Quad(solid(Rect::new(1.5, 1.0, 8.0, 8.0), Color::BLACK)),
        Item::PopLayer,
    ]);
    let usage = OutputColor::SrgbOpaque;
    let baseline = target(&device, [64, 48], true);
    let larger = target(&device, [66, 50], true);
    for (list, drawable) in [(&base, &baseline), (&large, &baseline), (&large, &larger), (&layered, &larger), (&base, &larger)] {
        let (frame, profile) = record(&mut gpu, drawable, list, usage);
        assert!(!profile.partial_raster);
        assert_eq!(profile.damage_area_ratio, 1.0);
        if std::ptr::eq(list, &large) && std::ptr::eq(drawable, &baseline) {
            assert_eq!(profile.output_pixels, 0);
        }
        frame.submit();
    }
    gpu.set_target_generation(4);
    let (frame, profile) = record(&mut gpu, &larger, &base, usage);
    assert_eq!(profile.damage_area_ratio, 1.0);
    frame.submit();
}

#[test]
fn five_k_caret_hover_and_scroll_match_the_cpu_reference() {
    let Some(mut gpu) = GpuRenderer::new() else { return };
    let device = gpu.device().clone();
    let port = NodeId::default();
    let sidebar = Part::Group(Arc::from([
        Part::Item(Item::PushClip { source: port, rect: Rect::new(0.0, 0.0, 260.0, 1280.0), radius: [0.0; 4] }),
        Part::ScrollContent {
            source: port,
            spatial_id: 19,
            scrollbar_offset: Vec2::default(),
            parts: Arc::from([Part::Item(Item::Quad(solid(Rect::new(24.0, 88.0, 130.0, 14.0), Color::WHITE)))]),
        },
        Part::Item(Item::PopClip),
    ]));
    let parts = |caret, hover| {
        let row = Part::Item(Item::Quad(solid(Rect::new(414.0, 68.0, 120.0, 20.0), if hover { Color::WHITE } else { Color::BLACK })));
        let caret =
            if caret { Arc::from([Part::Item(Item::Quad(solid(Rect::new(502.0, 58.0, 1.0, 13.0), Color::WHITE)))]) } else { Arc::from([]) };
        Arc::from([sidebar.clone(), row, Part::Group(caret)])
    };
    let scene = |parts, offset| {
        let mut scrolls = ScrollPlacements::default();
        scrolls.set(port, Some(ScrollPlacement { offset: Vec2::new(0.0, offset), scrollbars: [None, None] }));
        DisplayList { size: Size::new(2316.0, 1280.0), background: Color::BLACK, parts: vec![Part::ScrollFrame { scrolls, parts }] }
    };
    let seed = scene(parts(true, false), 0.0);
    let drawable = target(&device, EXTENT, true);
    record(&mut gpu, &drawable, &seed, OutputColor::SrgbOpaque).0.submit();
    let mut cpu = crate::CpuRenderer::new();
    let hover = parts(false, true);
    for list in [scene(parts(false, false), 0.0), scene(hover.clone(), 0.0), scene(hover, -38.3)] {
        let (frame, profile) = record(&mut gpu, &drawable, &list, OutputColor::SrgbOpaque);
        assert!(profile.partial_raster);
        let actual = pixels(&device, frame, &drawable);
        let reference = cpu.render(&list, 2.0);
        parity(&actual, reference.rgba(), EXTENT[0]);
    }
}

fn assert_edge_changes(old: &DisplayList, new: &DisplayList, at: [u32; 2]) {
    let Some(mut gpu) = GpuRenderer::new() else { return };
    let device = gpu.device().clone();
    let mut reference = GpuRenderer::with_device(device.clone(), gpu.queue().clone());
    let extent = [64, 48];
    let color = OutputColor::SrgbOpaque;
    let first = target(&device, extent, true);
    let (frame, _) = record(&mut gpu, &first, old, color);
    let previous = pixels(&device, frame, &first);
    let drawable = target(&device, extent, true);
    let (frame, profile) = record(&mut gpu, &drawable, new, color);
    let actual = pixels(&device, frame, &drawable);
    let full = full_pixels(&mut reference, &device, &drawable, new, color);
    let i = ((at[1] * extent[0] + at[0]) * 4) as usize;
    assert_ne!(&previous[i..i + 4], &full[i..i + 4], "fixture must change a visible pixel");
    assert!(profile.partial_raster && profile.damage_area_ratio > 0.0, "{profile:?}");
    parity(&actual, &full, extent[0]);
}

#[test]
fn nested_scroll_damage_tracks_cumulative_snap_with_unchanged_child_placement() {
    let outer = NativeId(1).node();
    let inner = NativeId(2).node();
    let parts = Arc::from([
        Part::Item(Item::PushClip { source: outer, rect: Rect::new(1.0, 1.0, 14.0, 19.0), radius: [0.0; 4] }),
        Part::ScrollContent {
            source: outer,
            spatial_id: 90,
            scrollbar_offset: Vec2::default(),
            parts: Arc::from([
                Part::Item(Item::PushClip { source: inner, rect: Rect::new(2.0, 2.0, 11.0, 11.0), radius: [0.0; 4] }),
                Part::ScrollContent {
                    source: inner,
                    spatial_id: 91,
                    scrollbar_offset: Vec2::default(),
                    parts: Arc::from([Part::Item(Item::Quad(solid(Rect::new(6.0, 7.0, 3.0, 2.0), Color::WHITE)))]),
                },
                Part::Item(Item::PopClip),
            ]),
        },
        Part::Item(Item::PopClip),
    ]);
    let mut scrolls = ScrollPlacements::default();
    scrolls.set(inner, Some(ScrollPlacement { offset: Vec2::new(0.0, -0.04), scrollbars: [None, None] }));
    let scene = |mut scrolls: ScrollPlacements, offset| {
        scrolls.set(outer, Some(ScrollPlacement { offset: Vec2::new(0.0, offset), scrollbars: [None, None] }));
        DisplayList { parts: vec![Part::ScrollFrame { scrolls, parts: Arc::clone(&parts) }], ..picture([]) }
    };
    let old = scene(scrolls.clone(), -0.20);
    let new = scene(scrolls, -0.24);
    assert_edge_changes(&old, &new, [13, 13]);
}

#[test]
fn nominally_disjoint_image_rects_can_overlap_at_an_antialiased_pixel() {
    let source = NodeId::default();
    let old = picture([Item::Image(ImagePaint {
        source,
        image: Image::rgba(1, 1, [255, 0, 0, 255]).sample(),
        rect: Rect::new(3.25, 2.0, 1.9, 5.0),
        bounds: Rect::new(5.35, 2.0, 4.0, 5.0),
        radius: [0.0; 4],
        opacity: 1.0,
    })]);
    assert_edge_changes(&old, &picture([]), [10, 7]);
}

#[test]
fn native_overlay_snapshot_damages_its_pixels_and_reuses_scroll_spatials() {
    let Some(mut gpu) = GpuRenderer::new() else { return };
    let device = gpu.device().clone();
    let mut reference = GpuRenderer::with_device(device.clone(), gpu.queue().clone());
    let source = NodeId::default();
    let extent = [64, 48];
    let color = OutputColor::SrgbOpaque;
    let drawable = target(&device, extent, true);
    let rect = Rect::new(3.0, 7.0, 8.0, 7.0);
    let clip = Rect::new(1.0, 3.0, 13.0, 12.0);
    let structure = |item| -> Arc<[Part]> {
        Arc::from([
            Part::Item(Item::PushClip { source, rect: clip, radius: [0.0; 4] }),
            Part::ScrollContent { source, spatial_id: 17, scrollbar_offset: Vec2::default(), parts: Arc::from([Part::Item(item)]) },
            Part::Item(Item::PopClip),
            Part::Item(Item::Quad(solid(Rect::new(10.0, 8.0, 5.0, 5.0), Color::WHITE))),
        ])
    };
    let live = structure(Item::Native { source, rect, radius: [0.0; 4], slot: NativeSlot::Overlay, cut: false });
    let frozen = structure(Item::Image(ImagePaint {
        source,
        image: Image::rgba(1, 1, [74, 115, 215, 255]).sample(),
        rect,
        bounds: rect,
        radius: [0.0; 4],
        opacity: 1.0,
    }));
    let scene = |parts, offset| {
        let mut scrolls = ScrollPlacements::default();
        scrolls.set(source, Some(ScrollPlacement { offset: Vec2::new(0.0, offset), scrollbars: [None, None] }));
        DisplayList { parts: vec![Part::ScrollFrame { scrolls, parts }], ..picture([]) }
    };
    let before = scene(live.clone(), 0.0);
    record(&mut gpu, &drawable, &before, color).0.submit();
    for (index, list) in [scene(frozen.clone(), 0.0), scene(frozen, -3.0), scene(live, -3.0)].into_iter().enumerate() {
        let (frame, profile) = record(&mut gpu, &drawable, &list, color);
        assert!(profile.partial_raster, "{profile:?}");
        if index == 1 {
            assert_eq!(profile.spatial_upload_bytes, 16);
        }
        let actual = pixels(&device, frame, &drawable);
        let full = full_pixels(&mut reference, &device, &drawable, &list, color);
        parity(&actual, &full, extent[0]);
    }
}

#[test]
fn retained_culled_groups_can_draw_into_a_clip_fringe() {
    let source = NodeId::default();
    let mut scrolls = ScrollPlacements::default();
    scrolls.set(source, Some(ScrollPlacement { offset: Vec2::default(), scrollbars: [None, None] }));
    let rect = Rect::new(3.25, 2.0, 1.9, 5.0);
    let scene = |color| {
        let parts = Arc::from([
            Part::Item(Item::PushClip { source, rect: Rect::new(5.25, 0.0, 10.0, 12.0), radius: [0.0; 4] }),
            Part::ScrollContent {
                source,
                spatial_id: 81,
                scrollbar_offset: Vec2::default(),
                parts: Arc::from([Part::CulledGroup { bounds: rect, parts: Arc::from([Part::Item(Item::Quad(solid(rect, color)))]) }]),
            },
            Part::Item(Item::PopClip),
        ]);
        DisplayList { parts: vec![Part::ScrollFrame { scrolls: scrolls.clone(), parts }], ..picture([]) }
    };
    assert_edge_changes(&scene(Color::WHITE), &scene(Color::BLACK), [10, 7]);
}

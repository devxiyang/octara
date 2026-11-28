//! Offscreen Metal render and readback on a real device.

#![cfg(target_os = "macos")]

#[path = "../../octara-render/tests/integration/fixtures.rs"]
mod fixtures;
#[path = "../../octara-render/tests/integration/parity.rs"]
mod parity;

use objc2::{
    rc::{Retained, autoreleasepool},
    runtime::ProtocolObject,
};
use objc2_metal::*;
use octara_model::capture::{PaintOutcome, PaintReceipt, PaintSample};
use octara_model::geometry::{Rect, Size, Vec2};
use octara_model::identity::NodeId;
use octara_model::paint::{
    CustomPaint, DisplayList, Image, Item, PaintMount, PaintProgram, Part, Picture, ScrollPlacement, ScrollPlacements, TransitionPaint,
};
use octara_model::theme::{Color, rgb};
use octara_render::{CpuRenderer, OutputColor};
use octara_render_metal::{MetalEncodedFrame, MetalRenderer};
use std::{sync::Arc, time::Duration};

struct Metal {
    renderer: MetalRenderer,
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
}

impl Metal {
    fn new() -> Option<Self> {
        let Some(device) = MTLCreateSystemDefaultDevice() else {
            assert!(std::env::var_os("OCTARA_REQUIRE_GPU").is_none(), "a Metal device is required");
            eprintln!("no Metal device: skipped");
            return None;
        };
        let queue = device.newCommandQueue().expect("Metal command queue");
        let renderer = MetalRenderer::new(device.clone()).expect("Metal renderer");
        Some(Self { renderer, device, queue })
    }

    fn capture(&mut self, list: &DisplayList, scale: f32, sample: Option<PaintSample>) -> (Picture, MetalEncodedFrame) {
        let width = (list.size.width * scale).ceil().max(1.0) as usize;
        let height = (list.size.height * scale).ceil().max(1.0) as usize;
        // These fixtures use 2D extents within the Metal device's limits.
        let descriptor = unsafe {
            MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                MTLPixelFormat::RGBA8Unorm,
                width,
                height,
                false,
            )
        };
        descriptor.setStorageMode(MTLStorageMode::Private);
        descriptor.setUsage(MTLTextureUsage::RenderTarget);
        let target = self.device.newTextureWithDescriptor(&descriptor).expect("offscreen Metal target");
        let command = self.queue.commandBuffer().expect("Metal frame command buffer");
        let frame = self.renderer.encode(&command, &target, list, scale, OutputColor::SrgbStraight, sample).expect("encode Metal frame");
        let row_bytes = width * 4;
        let pitch = row_bytes.next_multiple_of(256);
        let length = pitch * height;
        let buffer = self.device.newBufferWithLength_options(length, MTLResourceOptions::StorageModeShared).expect("Metal readback buffer");
        let blit = command.blitCommandEncoder().expect("Metal readback blit");
        // SAFETY: the copy covers the full texture and the buffer has a padded row for every row.
        unsafe {
            blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
                &target, 0, 0, MTLOrigin { x: 0, y: 0, z: 0 }, MTLSize { width, height, depth: 1 }, &buffer, 0, pitch, 0,
            );
        }
        blit.endEncoding();
        command.commit();
        command.waitUntilCompleted();
        assert_eq!(command.status(), MTLCommandBufferStatus::Completed, "{:?}", command.error());
        // SAFETY: the completed blit initialized each row's pixel bytes in the retained shared buffer.
        let mut rgba = Vec::with_capacity(row_bytes * height);
        for y in 0..height {
            let row = unsafe { std::slice::from_raw_parts(buffer.contents().cast::<u8>().as_ptr().add(y * pitch), row_bytes) };
            rgba.extend_from_slice(row);
        }
        (Picture::new(width as u32, height as u32, rgba).unwrap(), frame)
    }
}

#[test]
fn mixed_primitives_tiled_paths_and_image_mips_match_the_reference() {
    autoreleasepool(|_| {
        let Some(mut metal) = Metal::new() else { return };
        let mut cpu = CpuRenderer::new();
        #[cfg(feature = "wgpu")]
        let mut wgpu = wgpu_or_skip();
        for (name, list, scales) in [
            ("opaque mixed scene", parity::scene(Color::WHITE), [1.0, 1.25]),
            ("translucent mixed scene", parity::scene(Color::TRANSPARENT), [1.0, 1.25]),
            ("path fills, strokes, gradients and clips", parity::paths_list(), [1.0, 2.0]),
            ("minified premultiplied image mips", parity::minified_images(), [1.0, 1.25]),
            ("odd dedicated image mips", dedicated_image(), [1.0, 1.25]),
        ] {
            for scale in scales {
                let (native, _) = metal.capture(&list, scale, None);
                let reference = cpu.render(&list, scale);
                parity::compare(&format!("{name} Metal/CPU at {scale}x"), &reference, &native);
                #[cfg(feature = "wgpu")]
                if let Some(wgpu) = &mut wgpu {
                    let portable = wgpu.render(&list, scale);
                    parity::compare(&format!("{name} wgpu/CPU at {scale}x"), &reference, &portable);
                    parity::compare(&format!("{name} Metal/wgpu at {scale}x"), &portable, &native);
                }
            }
        }
    });
}

fn dedicated_image() -> DisplayList {
    let row: Vec<_> = (0..2049).flat_map(|x| if x < 1024 { [255, 64, 0, 128] } else { [0, 32, 255, 200] }).collect();
    let image = Image::rgba(2049, 7, row.repeat(7));
    let rect = Rect::new(2.25, 3.25, 21.0, 8.0);
    fixtures::list((28.0, 16.0), Color::TRANSPARENT, [parity::sample(&image, rect, rect, 2.0, 1.0)])
}

#[test]
fn atlas_slices_and_image_versions_retain_valid_pixels() {
    autoreleasepool(|_| {
        let Some(mut metal) = Metal::new() else { return };
        let colors = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 0, 255]];
        // Each padded image fills more than half a slice in both dimensions.
        let images = colors.map(|color| Image::rgba(1025, 1025, color.repeat(1025 * 1025)));
        let scene = || {
            fixtures::list(
                (32.0, 8.0),
                Color::BLACK,
                images.iter().enumerate().map(|(index, image)| {
                    let rect = Rect::new(index as f32 * 8.0, 0.0, 8.0, 8.0);
                    parity::sample(image, rect, rect, 0.0, 1.0)
                }),
            )
        };
        let list = scene();
        let (first, first_frame) = metal.capture(&list, 1.0, None);
        let (reused, reused_frame) = metal.capture(&list, 1.0, None);
        assert!(first_frame.profile.atlas_upload_bytes > 0);
        assert_eq!(reused_frame.profile.atlas_upload_bytes, 0);
        exact(&first, &reused);
        let row = |picture: &Picture| std::array::from_fn(|i| picture.pixel(i as u32 * 8 + 4, 4).unwrap());
        assert_eq!(row(&first), colors);
        let replacement = [255, 0, 255, 255];
        images[1].set(1025, 1025, replacement.repeat(1025 * 1025));
        let (updated, updated_frame) = metal.capture(&scene(), 1.0, None);
        assert!(updated_frame.profile.atlas_upload_bytes > 0);
        assert_eq!(row(&updated), [colors[0], replacement, colors[2], colors[3]]);
        exact(&first, &metal.capture(&list, 1.0, None).0);
    });
}

#[test]
fn five_k_damage_matches_full_metal_and_cpu_byte_for_byte() {
    autoreleasepool(|_| {
        let Some(mut metal) = Metal::new() else { return };
        let mut cpu = CpuRenderer::new();
        #[cfg(feature = "wgpu")]
        let mut wgpu = wgpu_or_skip();
        let sidebar = damage_sidebar();
        let seed = damage_parts(&sidebar, true, false);
        let blinking = damage_parts(&sidebar, false, false);
        let hovered = damage_parts(&sidebar, false, true);
        let scale = 2.0;
        for (revision, (parts, offset)) in
            [(seed, 0.0), (blinking, 0.0), (hovered.clone(), 0.0), (hovered.clone(), -38.3)].into_iter().enumerate()
        {
            let list = damage_scene(parts, offset);
            let sample = PaintSample { revision: revision as u64, sampled_at: Duration::ZERO };
            let (native, frame) = metal.capture(&list, scale, Some(sample));
            let profile = frame.profile;
            if revision == 0 {
                assert_eq!(profile.damage_area_ratio, 1.0);
            } else {
                assert!(
                    profile.partial_raster && (0.0..0.15).contains(&profile.damage_area_ratio) && profile.damage_area_ratio > 0.0,
                    "{profile:?}"
                );
            }
            if revision == 3 {
                assert!(profile.retained_prepare, "{profile:?}");
                assert_eq!(profile.instance_upload_bytes, 0);
                assert!((16..=32).contains(&profile.spatial_upload_bytes), "{profile:?}");
            }
            let reference = cpu.render(&list, scale);
            exact(&reference, &native);
            #[cfg(feature = "wgpu")]
            if let Some(wgpu) = &mut wgpu {
                exact(&reference, &wgpu.render(&list, scale));
            }
        }
        let last = damage_scene(hovered, -38.3);
        let (reused, frame) = metal.capture(&last, scale, Some(PaintSample { revision: 4, sampled_at: Duration::ZERO }));
        let profile = frame.profile;
        assert!(profile.partial_raster);
        assert_eq!((profile.damage_area_ratio, profile.draw_batches), (0.0, 0));
        exact(&metal.capture(&last, scale, None).0, &reused);
    });
}

#[test]
fn native_msl_material_draws_and_reports_success() {
    check_material(octara_render_metal::material_msl("float4 material(float2 uv, float4 params) { return params; }"));
}

#[cfg(feature = "custom-wgsl")]
#[test]
fn wgsl_material_translates_and_draws_when_enabled() {
    let program = octara_render::plan::Material::new("fn material(uv: vec2f, params: vec4f) -> vec4f { return params; }");
    check_material(PaintProgram::new(program));
}

fn check_material(program: PaintProgram) {
    autoreleasepool(|_| {
        let Some(mut metal) = Metal::new() else { return };
        let list = fixtures::list(
            (32.0, 32.0),
            Color::TRANSPARENT,
            [Item::Custom(CustomPaint {
                fallback: fixtures::quad(Rect::new(4.0, 4.0, 24.0, 24.0), 6.0, Color::BLACK),
                program,
                mount: Default::default(),
                exiting: false,
                input: [0.0, 1.0, 0.0, 1.0].into(),
                opacity: 1.0,
            })],
        );
        for revision in [1, 2] {
            let sample = PaintSample { revision, sampled_at: Duration::ZERO };
            let (pixels, frame) = metal.capture(&list, 1.0, Some(sample));
            assert_eq!(frame.receipts.len(), 1);
            assert_eq!(frame.receipts[0].outcome, PaintOutcome::Rendered, "revision {revision}");
            assert_eq!(frame.profile.material_batches, 1, "revision {revision}");
            assert_eq!(pixels.pixel(16, 16), Some([0, 255, 0, 255]));
            assert_eq!(pixels.pixel(4, 4), Some([0; 4]));
        }
    });
}

#[test]
fn crossfade_pins_the_host_accepted_source_for_the_run() {
    autoreleasepool(|_| {
        let Some(mut metal) = Metal::new() else { return };
        let sample = |revision| PaintSample { revision, sampled_at: Duration::from_millis(revision) };
        let settled = TransitionPaint {
            source: NodeId::default(),
            mount: PaintMount::default(),
            run: PaintMount::default(),
            revision: 0,
            program: octara_render_metal::transition_crossfade(),
            progress: 1.0,
            opacity: 1.0,
        };
        let scene = |paint, rect, color| {
            fixtures::list(
                (32.0, 24.0),
                Color::BLACK,
                [Item::PushTransition(paint), Item::Quad(fixtures::quad(rect, 0.0, color)), Item::PopLayer],
            )
        };
        let old_rect = Rect::new(4.0, 4.0, 16.0, 16.0);
        let new_rect = Rect::new(8.0, 4.0, 16.0, 16.0);
        let seed = scene(settled.clone(), old_rect, rgb(0xff0000));
        let (pixels, frame) = metal.capture(&seed, 1.0, Some(sample(10)));
        assert_eq!(pixels.pixel(12, 12), Some([255, 0, 0, 255]));
        assert!(frame.receipts.is_empty());
        metal.renderer.accept_presentation(frame.presentation);

        // Submission alone does not publish the alternate source.
        let discarded = scene(settled.clone(), old_rect, rgb(0x00ff00));
        let (pixels, frame) = metal.capture(&discarded, 1.0, Some(sample(11)));
        assert_eq!(pixels.pixel(12, 12), Some([0, 255, 0, 255]));
        drop(frame.presentation);

        let active = TransitionPaint { run: PaintMount::default(), revision: 1, progress: 0.5, ..settled };
        let outcome = PaintReceipt { source: active.source, outcome: PaintOutcome::Transition { old: sample(10) } };
        let (pixels, frame) = metal.capture(&scene(active.clone(), new_rect, rgb(0x0000ff)), 1.0, Some(sample(20)));
        assert_eq!(frame.receipts, [outcome]);
        assert_eq!(pixels.pixel(12, 12), Some([188, 0, 188, 255]));
        assert_eq!(pixels.pixel(6, 12), Some([188, 0, 0, 255]));
        assert_eq!(pixels.pixel(22, 12), Some([0, 0, 188, 255]));
        metal.renderer.accept_presentation(frame.presentation);

        let active = TransitionPaint { progress: 0.75, ..active };
        let (pixels, frame) = metal.capture(&scene(active, new_rect, rgb(0x0000ff)), 1.0, Some(sample(21)));
        assert_eq!(frame.receipts, [outcome]);
        assert_eq!(pixels.pixel(12, 12), Some([137, 0, 225, 255]));
    });
}

fn damage_sidebar() -> Part {
    let source = NodeId::default();
    Part::Group(Arc::from([
        Part::Item(Item::PushClip { source, rect: Rect::new(0.0, 0.0, 260.0, 1440.0), radius: [0.0; 4] }),
        Part::ScrollContent {
            source,
            spatial_id: 19,
            scrollbar_offset: Vec2::default(),
            parts: Arc::from([Part::Item(Item::Quad(fixtures::quad(Rect::new(24.0, 88.0, 130.0, 14.0), 0.0, Color::WHITE)))]),
        },
        Part::Item(Item::PopClip),
    ]))
}

fn damage_parts(sidebar: &Part, caret: bool, hover: bool) -> Arc<[Part]> {
    let row =
        Part::Item(Item::Quad(fixtures::quad(Rect::new(414.0, 68.0, 120.0, 20.0), 0.0, if hover { Color::WHITE } else { Color::BLACK })));
    let caret = caret.then(|| Part::Item(Item::Quad(fixtures::quad(Rect::new(502.0, 58.0, 1.0, 13.0), 0.0, Color::WHITE))));
    [sidebar.clone(), row].into_iter().chain(caret).collect()
}

fn damage_scene(parts: Arc<[Part]>, offset: f32) -> DisplayList {
    let source = NodeId::default();
    let mut scrolls = ScrollPlacements::default();
    scrolls.set(source, Some(ScrollPlacement { offset: Vec2::new(0.0, offset), scrollbars: [None, None] }));
    DisplayList { size: Size::new(2560.0, 1440.0), background: Color::BLACK, parts: vec![Part::ScrollFrame { scrolls, parts }] }
}

fn exact(reference: &Picture, actual: &Picture) {
    assert_eq!((reference.width(), reference.height()), (actual.width(), actual.height()));
    if let Some((index, (a, b))) =
        reference.rgba().as_chunks::<4>().0.iter().zip(actual.rgba().as_chunks::<4>().0).enumerate().find(|(_, (a, b))| a != b)
    {
        let width = reference.width() as usize;
        panic!("pixel at ({}, {}): expected {a:?}, got {b:?}", index % width, index / width);
    }
}

#[cfg(feature = "wgpu")]
fn wgpu_or_skip() -> Option<octara_render::WgpuRenderer> {
    let renderer = octara_render::WgpuRenderer::new();
    if renderer.is_none() {
        assert!(std::env::var_os("OCTARA_REQUIRE_GPU").is_none(), "a wgpu adapter is required");
        eprintln!("no wgpu adapter: skipped");
    }
    renderer
}

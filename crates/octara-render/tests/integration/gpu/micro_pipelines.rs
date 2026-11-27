use super::{compare, glyphs, gpu_or_skip, list, picture, quad, sample};
use octara_model::geometry::{Rect, point};
use octara_model::identity::NodeId;
use octara_model::motion::{TextReveal, TextRevealKind, TextRevealTrack};
use octara_model::paint::{DisplayList, Image, Item, Quad, Shadow};
use octara_model::theme::{Color, Gradient, rgb};
use octara_render::gpu::{FrameInput, FrameTarget, OutputColor};
use octara_render::{CpuRenderer, wgpu};

#[test]
fn opaque_cores_preserve_order_at_edges_and_inside_nested_clips() {
    let Some(mut gpu) = gpu_or_skip() else { return };
    let source = NodeId::default();
    let card = Rect::new(19.25, 18.75, 88.0, 54.5);
    let corners = [12.0, 8.0, 18.0, 5.0];
    let card_color = rgb(0x28a58c);
    let image_color = [253, 173, 42, 255];
    let image = Image::rgba(11, 9, image_color.repeat(99));
    let dl = list(
        (168.0, 94.0),
        Color::TRANSPARENT,
        [
            Item::Quad(quad(Rect::new(4.25, 4.25, 160.0, 86.0), 0.0, rgb(0x3566cd).with_alpha(0.75))),
            Item::PushClip { source, rect: Rect::new(8.25, 7.75, 152.0, 83.0), radius: [0.0; 4] },
            Item::PushClip { source, rect: Rect::new(14.125, 11.125, 140.0, 77.0), radius: [17.0, 4.0, 15.0, 22.0] },
            Item::PushClip { source, rect: Rect::new(17.75, 10.625, 122.0, 73.75), radius: [0.0; 4] },
            Item::PushClip { source, rect: Rect::new(8.0, 13.0, 173.0, 81.0), radius: [20.0; 4] },
            Item::Quad(quad(Rect::new(4.25, 10.25, 130.0, 53.0), 3.0, rgb(0xeb6f5b).with_alpha(0.45))),
            glyphs("你好世界", point(15.5, 46.0), 25.0, Color::WHITE),
            Item::Shadow(Shadow { source, rect: card, radius: corners, color: Color::BLACK.with_alpha(0.6), blur: 10.0 }),
            Item::Quad(Quad {
                radius: corners,
                border: [1.25, 3.25, 2.25, 1.25],
                border_color: rgb(0x353d68).with_alpha(0.8),
                ..quad(card, 0.0, card_color)
            }),
            sample(&image, Rect::new(84.75, 27.25, 46.0, 44.0), Rect::new(88.25, 26.5, 40.0, 41.0), 11.0, 1.0),
            Item::Quad(Quad {
                gradient: Some(Gradient { angle: -75.0, from: (rgb(0xaed88c), 0.13), to: (rgb(0x6b40d0), 0.9) }),
                ..quad(Rect::new(116.75, 42.25, 38.0, 30.0), 6.0, Color::TRANSPARENT)
            }),
            Item::Quad(Quad {
                gradient: Some(Gradient {
                    angle: 38.0,
                    from: (rgb(0x3c6ae8).with_alpha(0.3), 0.15),
                    to: (rgb(0xf83e70).with_alpha(0.75), 0.88),
                }),
                ..quad(Rect::new(53.5, 56.25, 93.0, 30.0), 8.0, Color::TRANSPARENT)
            }),
            Item::PopClip,
            Item::PopClip,
            Item::PopClip,
            Item::PopClip,
        ],
    );
    let mut cpu = CpuRenderer::new();
    let reference = cpu.render(&dl, 1.0);
    assert_eq!(reference.pixel(50, 53), Some(card_color.to_rgba8().to_u8_array()));
    assert_eq!(reference.pixel(109, 52), Some(image_color));
    compare("opaque cores and nested clips at 1x", &reference, &gpu.render(&dl, 1.0));
    compare("opaque cores and nested clips at 1.25x", &cpu.render(&dl, 1.25), &gpu.render(&dl, 1.25));
}

#[test]
fn thin_primitives_retain_fractional_coverage() {
    let Some(mut gpu) = gpu_or_skip() else { return };
    let stripe = Image::rgba(1, 8, [48, 176, 241, 255].repeat(8));
    let dl = list(
        (32.0, 28.0),
        Color::TRANSPARENT,
        [
            Item::Quad(quad(Rect::new(4.5, 3.0, 0.0, 9.0), 0.0, rgb(0xff463e))),
            sample(&stripe, Rect::new(14.25, 3.0, 0.5, 9.0), Rect::new(12.0, 1.0, 8.0, 14.0), 0.0, 1.0),
            Item::Shadow(Shadow {
                source: NodeId::default(),
                rect: Rect::new(3.0, 20.5, 9.0, 0.0),
                radius: [0.0; 4],
                color: rgb(0x5e2a99),
                blur: 0.0,
            }),
        ],
    );
    let reference = CpuRenderer::new().render(&dl, 1.0);
    assert_eq!(reference.pixel(4, 8).unwrap()[3], 128);
    assert_eq!(reference.pixel(14, 8).unwrap()[3], 191);
    assert_eq!(reference.pixel(8, 20).unwrap()[3], 128);
    compare("thin quad, image and sharp shadow", &reference, &gpu.render(&dl, 1.0));
}

#[test]
fn large_fractional_radii_keep_authored_edge_coverage() {
    let Some(mut gpu) = gpu_or_skip() else { return };
    let dl = list(
        (176.0, 176.0),
        Color::TRANSPARENT,
        [Item::Quad(quad(Rect::new(0.125, 0.125, 1024.0, 1024.0), 511.875, rgb(0x62a894).with_alpha(0.9)))],
    );
    compare("large fractional corner", &CpuRenderer::new().render(&dl, 1.0), &gpu.render(&dl, 1.0));
}

#[test]
fn hollow_shadows_keep_the_portions_visible_through_cards() {
    let Some(mut gpu) = gpu_or_skip() else { return };
    let source = NodeId::default();
    let rect = Rect::new(14.25, 13.75, 52.0, 37.0);
    let corners = [3.0, 13.0, 17.0, 8.0];
    let shadow = Item::Shadow(Shadow { source, rect, radius: corners, color: rgb(0x596177).with_alpha(0.82), blur: 9.0 });
    let mut cpu = CpuRenderer::new();
    for (name, fill) in [("opaque", Some(Color::WHITE)), ("translucent", Some(Color::WHITE.with_alpha(0.45))), ("border only", None)] {
        let card = Item::Quad(Quad {
            radius: corners,
            border: [2.0; 4],
            border_color: rgb(0x737fa0),
            fill,
            ..quad(rect, 0.0, Color::TRANSPARENT)
        });
        let dl = list((84.0, 68.0), Color::TRANSPARENT, [shadow.clone(), card]);
        let reference = cpu.render(&dl, 1.0);
        assert!(reference.pixel(40, 55).unwrap()[3] > 0, "the exterior shadow is visible");
        assert!(reference.pixel(40, 32).unwrap()[3] > 0, "the shadow or fill covers the center");
        compare(name, &reference, &gpu.render(&dl, 1.0));
    }
}

#[test]
fn minified_atlas_images_keep_premultiplied_mips_and_isolated_edges() {
    let Some(mut gpu) = gpu_or_skip() else { return };
    let stripes = (0..64 * 64).flat_map(|i| if i / 64 % 4 == 0 { [255, 0, 0, 255] } else { [0, 255, 255, 0] }).collect::<Vec<_>>();
    let red = Image::rgba(64, 64, stripes);
    let green = Image::rgba(64, 64, [0, 255, 0, 255].repeat(64 * 64));
    let blue = Image::rgba(64, 64, [0, 0, 255, 255].repeat(64 * 64));
    let dl = list(
        (66.0, 24.0),
        Color::TRANSPARENT,
        [&green, &red, &blue].into_iter().enumerate().map(|(i, image)| {
            let x = 1.25 + 22.0 * i as f32;
            sample(image, Rect::new(x, 2.0, 16.0, 16.0), Rect::new(x - 1.0, 1.0, 18.0, 18.0), 0.0, 1.0)
        }),
    );
    let mut cpu = CpuRenderer::new();
    let reference = cpu.render(&dl, 1.0);
    assert_eq!(reference.pixel(30, 8), Some([255, 0, 0, 64]), "one opaque red row per 4×4 mip texel");
    for x in [23, 39] {
        let [r, g, b, a] = reference.pixel(x, 8).unwrap();
        assert_eq!([r, g, b], [255, 0, 0]);
        assert!(a > 0 && a < 64, "partial coverage at x={x}: {a}");
    }
    compare("packed minified images and gutters at 1x", &reference, &gpu.render(&dl, 1.0));
    compare("packed minified images and gutters at 1.25x", &cpu.render(&dl, 1.25), &gpu.render(&dl, 1.25));
}

#[test]
fn glyph_pipeline_matches_a_sampled_text_reveal() {
    let Some(mut gpu) = gpu_or_skip() else { return };
    let tint = rgb(0x4789f8);
    let gradient = TextRevealKind::Gradient { trail_bytes: 6, tint: Some(tint.into()), glide_bytes_per_sec: 12.0 };
    let fade = TextRevealKind::Fade { duration_secs: 1.75 };
    for (kind, expected, name) in [(gradient, [1.0, 0.75, 0.25, 0.0], "gradient"), (fade, [0.5, 0.5, 0.5, 0.0], "fade")] {
        let track = TextRevealTrack::update(None, TextReveal { kind, revealed_bytes: 9, finished: false }, 0.0);
        let Item::Glyphs(mut run) = glyphs("你好世界", point(5.25, 25.5), 19.5, rgb(0xed6941)) else { unreachable!() };
        let colors: Vec<_> = run.glyphs.iter().map(|glyph| track.color(glyph.source_end, 0.875, run.color, Some(tint))).collect();
        assert_eq!(colors.iter().map(|color| color.components[3]).collect::<Vec<_>>(), expected);
        let positions = run.glyphs.clone();
        run.colors = Some(colors.into());
        assert!(std::sync::Arc::ptr_eq(&positions, &run.glyphs));
        let dl = list((88.0, 34.0), Color::TRANSPARENT, [Item::Glyphs(run)]);
        compare(name, &CpuRenderer::new().render(&dl, 1.25), &gpu.render(&dl, 1.25));
    }
}

fn chat_showcase() -> DisplayList {
    let preview = picture();
    let source = NodeId::default();
    let viewport = Rect::new(8.0, 6.0, 392.0, 195.0);
    let mut items = vec![Item::PushClip { source, rect: viewport, radius: [0.0; 4] }];
    for (left, top, message, fill) in
        [(178.0, 10.0, "你好世界", rgb(0xdae8ff)), (16.0, 74.0, "世界你好", Color::WHITE), (178.0, 138.0, "你好", rgb(0xdae8ff))]
    {
        let bubble = Rect::new(left, top, 208.0, 42.0);
        let attachment = Rect::new(left + 12.0, top + 8.0, 31.0, 26.0);
        items.extend([
            Item::Quad(Quad { border: [1.0; 4], border_color: rgb(0xc2cfe7), ..quad(bubble, 9.0, fill) }),
            sample(&preview, attachment, attachment, 5.0, 0.8),
            glyphs(message, point(left + 54.25, top + 28.0), 15.0, rgb(0x24324b)),
        ]);
    }
    let composer = Rect::new(16.0, 202.0, 370.0, 42.0);
    items.extend([
        Item::PopClip,
        Item::Quad(Quad { border: [1.0; 4], border_color: rgb(0xa7b6d1), ..quad(composer, 8.0, Color::WHITE) }),
        glyphs("你好", point(28.25, 228.0), 15.0, rgb(0x273852)),
    ]);
    list((408.0, 256.0), rgb(0xf2f5fb), items)
}

#[test]
fn mixed_chat_showcase_draws_in_at_most_four_calls() {
    let Some(mut gpu) = gpu_or_skip() else { return };
    let scene = chat_showcase();
    let target = gpu.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("chat draw count"),
        size: wgpu::Extent3d { width: scene.size.width as u32, height: scene.size.height as u32, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let encoder = gpu.device().create_command_encoder(&Default::default());
    let target = FrameTarget { texture: &target, color: OutputColor::Linear };
    let input = FrameInput { list: &scene, scale: 1.0, sample: None };
    let (frame, profile) = gpu.record(encoder, target, input, Some(Box::new(|_| {}))).unwrap();
    let profile = profile.unwrap();
    frame.submit();
    eprintln!("mixed chat showcase: {} draw calls", profile.draw_batches);
    assert_eq!(profile.output_pixels, 0);
    assert_eq!(profile.render_passes, 2, "opaque core and blended coverage passes");
    assert!((1..=4).contains(&profile.draw_batches), "{profile:?}");
    compare("mixed chat showcase", &CpuRenderer::new().render(&scene, 1.0), &gpu.render(&scene, 1.0));
}

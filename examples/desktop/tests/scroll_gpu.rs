//! Exercise the live Metal backend with retained native-pipeline scroll output.

use octara::prelude::*;
use octara::{App, Input, InteractionEvent, WheelPhase};
use octara_pipeline::{NativeEngine, NativeEngineConfig, NativePublication};
use octara_render::gpu::{FrameInput, FrameTarget, OutputColor};
use octara_render::{CpuRenderer, GpuRenderer, wgpu};

#[test]
fn retained_scroll_uploads_only_spatials_and_matches_cpu_at_retina_scales() {
    let Some(mut gpu) = GpuRenderer::new() else {
        assert!(std::env::var_os("OCTARA_REQUIRE_GPU").is_none(), "a GPU adapter is required");
        return;
    };
    let mut cpu = CpuRenderer::new();
    for scale in [2.0, 3.0] {
        let publication = NativePublication::default();
        let output = publication.clone();
        let mut app = App::new(move |_, _| {
            Box::new(NativeEngine::with_publication(NativeEngineConfig { check_paint_cache: true, ..Default::default() }, output.clone()))
        });
        let options =
            WindowOptions { viewport: WindowViewport { size: Size::new(168.0, 112.0), scale, ..Default::default() }, ..Default::default() };
        let window = app
            .open_window(options, |_| {
                col()
                    .w(160)
                    .h(100)
                    .overflow_x(Overflow::Hidden)
                    .overflow_y(Overflow::Scroll)
                    .scrollbar(ScrollbarVisibility::Always)
                    .rounded(6)
                    .children((0..160).map(|row| {
                        col()
                            .h(18)
                            .shrink(0.0)
                            .px(4)
                            .bg(if row % 2 == 0 { ColorToken::Surface } else { ColorToken::SurfaceHover })
                            .child(text(format!("Retained row {row}")))
                    }))
            })
            .unwrap();
        let device = gpu.device().clone();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Retained scroll validation"),
            size: wgpu::Extent3d { width: (168.0 * scale) as u32, height: (112.0 * scale) as u32, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let mut initial_upload = 0;
        for step in 0..3 {
            if step != 0 {
                let event = InteractionEvent::Wheel {
                    pos: Point::new(30.0, 30.0),
                    delta: Vec2::new(0.0, 17.75),
                    phase: WheelPhase::Discrete,
                    mods: Modifiers::empty(),
                };
                assert!(app.dispatch(window, Input::Interaction(event)).unwrap().handled);
            }
            let frame = publication.latest().unwrap();
            let encoder = device.create_command_encoder(&Default::default());
            let target = FrameTarget { texture: &texture, color: OutputColor::SrgbOpaque };
            let input = FrameInput { list: &frame.draw, scale, sample: None };
            let (recorded, _) = gpu.record(encoder, target, input, None).unwrap();
            let uploaded = recorded.upload_bytes();
            if step == 0 {
                initial_upload = uploaded;
            } else {
                assert!(uploaded < 1024 && uploaded < initial_upload, "unexpected scroll upload {uploaded}/{initial_upload} bytes");
            }
            let (_, pending) = recorded.submit_for_presentation();
            gpu.accept_presentation(pending);
            if step == 2 {
                let expected = cpu.render(&frame.draw, scale);
                let actual = gpu.render(&frame.draw, scale);
                assert_eq!((expected.width(), expected.height()), (actual.width(), actual.height()));
                let worst = expected.rgba().iter().zip(actual.rgba()).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
                assert!(worst <= 2, "largest CPU/GPU channel difference at {scale}x is {worst}");
                eprintln!("{} at {scale}x: {initial_upload} bytes cold, {uploaded} bytes scrolled", gpu.renderer());
            }
        }
    }
}

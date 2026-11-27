//! Render the display list of an authored rich span with both backends.

use octara::model::geometry::{Point, Size, WindowViewport};
use octara::prelude::*;
use octara_pipeline::{NativeEngine, NativeEngineConfig};
use octara_render::{CpuRenderer, GpuRenderer};

#[test]
fn rich_span_pill_has_cpu_gpu_parity() {
    let mut app = App::new(|_, _| Box::new(NativeEngine::new(NativeEngineConfig::default())));
    let viewport = WindowViewport { size: Size::new(180.0, 64.0), scale: 1.0, safe_area: None, keyboard_inset: 0.0 };
    let options = WindowOptions { viewport, ..Default::default() };
    let background = rgb(0xd5e8fb);
    let border_color = rgb(0x284572);
    let text_color = rgb(0x301566);
    app.open_window(options, move |_| {
        col().p(px(8.0)).child(
            text(
                rich()
                    .text("Use ")
                    .span(
                        "label",
                        Span {
                            mono: true,
                            color: Some(text_color.into()),
                            background: Some(background.into()),
                            border_color: Some(border_color.into()),
                            radius: Some(4.0),
                            padding_x: Some(8.0),
                            ..Default::default()
                        },
                    )
                    .text(" here"),
            )
            .test_id("pill"),
        )
    })
    .unwrap();

    let window = app.runtime().windows().next().unwrap().1;
    let label = window.tree().test_ids("pill")[0];
    let drawing = window.last_commit().unwrap().draw.as_ref().unwrap();
    let pill = drawing.quads().find(|quad| quad.source == label).expect("the span paints a quad");
    assert_eq!(pill.radius, [4.0; 4]);
    assert_eq!(pill.fill, Some(background));
    assert_eq!(pill.border, [1.0; 4]);
    assert_eq!(pill.border_color, border_color);
    let mut glyphs = drawing.glyph_runs().filter(|run| run.source == label);
    let before = glyphs.next().unwrap();
    let inside = glyphs.find(|run| run.color == text_color).unwrap();
    let after = glyphs.find(|run| run.color != text_color).unwrap();
    let first_x = |run: &octara::model::paint::GlyphRun| run.origin.x + run.glyphs[0].x;
    assert!(first_x(&before) < pill.rect.x);
    assert!((first_x(&inside) - pill.rect.x - 9.0).abs() < 0.1);
    assert!(first_x(&after) >= pill.rect.right() - 0.1, "the following run begins outside the pill");

    let mut cpu = CpuRenderer::new();
    let mut gpu = GpuRenderer::new();
    if let Some(gpu) = &gpu {
        eprintln!("RichSpan CPU/GPU parity on {:?}", gpu.device().adapter_info().backend);
    } else {
        assert!(std::env::var_os("OCTARA_REQUIRE_GPU").is_none(), "a GPU adapter is required");
    }
    for scale in [1.0, 2.0] {
        let expected = cpu.render(drawing, scale);
        let sample = |point: Point| expected.pixel((point.x * scale).floor() as u32, (point.y * scale).floor() as u32).unwrap();
        assert_eq!(sample(Point::new(pill.rect.x + 5.0, pill.rect.y + 8.0)), [213, 232, 251, 255]);
        assert_eq!(sample(Point::new(pill.rect.x + 5.0, pill.rect.y + 0.25)), [40, 69, 114, 255]);
        assert_ne!(sample(Point::new(pill.rect.x, pill.rect.y)), [40, 69, 114, 255], "rounded corner");
        if let Some(gpu) = &mut gpu {
            let actual = gpu.render(drawing, scale);
            assert_eq!((actual.width(), actual.height()), (expected.width(), expected.height()));
            let worst = expected.rgba().iter().zip(actual.rgba()).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
            assert!(worst <= 2, "largest CPU/GPU channel difference at {scale}x is {worst}");
        }
    }
}

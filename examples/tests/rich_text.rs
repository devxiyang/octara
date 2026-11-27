//! Render authored rich text with both backends.

use octara::model::geometry::{Point, Rect, Size, WindowViewport};
use octara::model::paint::{DisplayList, GlyphRun, Item, Picture};
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
    let first_x = |run: &GlyphRun| run.origin.x + run.glyphs[0].x;
    assert!(first_x(&before) < pill.rect.x);
    assert!((first_x(&inside) - pill.rect.x - 9.0).abs() < 0.1);
    assert!(first_x(&after) >= pill.rect.right() - 0.1, "the following run begins outside the pill");

    assert_cpu_gpu_parity(drawing, |expected, scale| {
        let sample = |point: Point| expected.pixel((point.x * scale).floor() as u32, (point.y * scale).floor() as u32).unwrap();
        assert_eq!(sample(Point::new(pill.rect.x + 5.0, pill.rect.y + 8.0)), [213, 232, 251, 255]);
        assert_eq!(sample(Point::new(pill.rect.x + 5.0, pill.rect.y + 0.25)), [40, 69, 114, 255]);
        assert_ne!(sample(Point::new(pill.rect.x, pill.rect.y)), [40, 69, 114, 255], "rounded corner");
    });
}

#[test]
fn inline_pill_has_cpu_gpu_parity() {
    let mut app = App::new(|_, _| Box::new(NativeEngine::new(NativeEngineConfig::default())));
    let viewport = WindowViewport { size: Size::new(220.0, 64.0), scale: 1.0, safe_area: None, keyboard_inset: 0.0 };
    let background = rgb(0xd5e8fb);
    let border = rgb(0x284572);
    let tint = rgb(0x301566);
    let theme = Theme::default().color(ColorToken::Surface, background).color(ColorToken::Accent, tint);
    let presentation = WindowPresentation { theme, ..Default::default() };
    let options = WindowOptions { viewport, presentation, ..Default::default() };
    let style = PillStyle {
        size: 14.0,
        weight: 600,
        color: Some(ColorToken::Accent.into()),
        background: Some(ColorToken::Surface.into()),
        border_color: Some(border.into()),
        radius: 6.0,
        padding_x: 8.0,
        padding_y: 4.0,
        gap: 4.0,
    };
    let logical = "界";
    let source = "Use 界 here";
    let range = 4..4 + logical.len();
    let pill = InlinePill {
        icon: Some(NodeIcon::new(&Icon::path(Path::rect(Rect::new(5.0, 5.0, 14.0, 14.0))), None)),
        label: "Wide ffi".into(),
        style,
    };
    app.open_window(options, {
        let range = range.clone();
        move |_| {
            col().p(px(8.0)).child(
                text(rich().text(source).attachment(pill.attachment(range.clone())).link_range(range, "https://example.test"))
                    .test_id("pill"),
            )
        }
    })
    .unwrap();

    let window = app.runtime().windows().next().unwrap().1;
    let label = window.tree().test_ids("pill")[0];
    let slot = window.tree().text(label).unwrap();
    assert_eq!(&*slot.text, source);
    let drawing = window.last_commit().unwrap().draw.as_ref().unwrap();
    let quads: Vec<_> = drawing.quads().filter(|quad| quad.source == label).collect();
    let [pill] = quads.as_slice() else { panic!("the pill must paint one quad") };
    assert_eq!(pill.radius, [6.0; 4]);
    assert_eq!(pill.fill, Some(background));
    assert_eq!(pill.border, [1.0; 4]);
    assert_eq!(pill.border_color, border);
    let center = Point::new(pill.rect.x + pill.rect.w * 0.5, pill.rect.y + pill.rect.h * 0.5);
    assert_eq!(window.hit_test(center).unwrap().hit.node(), Some(slot.links[0]));

    let drawing_items = drawing.items();
    let icon = drawing_items
        .iter()
        .find_map(|item| match item {
            Item::Path(path) if path.source == label => Some(path),
            _ => None,
        })
        .unwrap();
    assert_eq!(icon.color, tint);
    assert_eq!((icon.bounds.w, icon.bounds.h), (style.size, style.size));
    assert!((icon.bounds.x - pill.rect.x - style.padding_x).abs() < 0.01);
    assert!((icon.bounds.y + style.size * 0.5 - center.y).abs() < 0.01);
    let runs: Vec<_> = drawing.glyph_runs().filter(|run| run.source == label).collect();
    let inside = runs.iter().find(|run| run.color == tint).unwrap();
    assert!(inside.glyphs.iter().all(|glyph| glyph.source_end == 0));
    let before = runs.iter().find(|run| run.color != tint).unwrap();
    let y = |run: &GlyphRun| run.origin.y + run.glyphs[0].y;
    assert!((y(inside) - y(before)).abs() < 0.01, "label and surrounding text share the baseline");
    let first_x = inside.origin.x + inside.glyphs[0].x;
    assert!((first_x - icon.bounds.right() - style.gap).abs() < 0.01);

    assert_cpu_gpu_parity(drawing, |frame, scale| {
        let sample = |point: Point| frame.pixel((point.x * scale).floor() as u32, (point.y * scale).floor() as u32).unwrap();
        assert_eq!(sample(Point::new(center.x, pill.rect.y + 2.25)), [213, 232, 251, 255]);
        assert_ne!(sample(Point::new(pill.rect.x, pill.rect.y)), [40, 69, 114, 255], "rounded corner");
        assert_eq!(sample(Point::new(icon.bounds.x + style.size * 0.5, center.y)), [48, 21, 102, 255]);
    });
}

fn assert_cpu_gpu_parity(drawing: &DisplayList, inspect_cpu: impl Fn(&Picture, f32)) {
    let mut cpu = CpuRenderer::new();
    let mut gpu = GpuRenderer::new();
    if let Some(gpu) = &gpu {
        eprintln!("Rich text CPU/GPU parity on {:?}", gpu.device().adapter_info().backend);
    } else {
        assert!(std::env::var_os("OCTARA_REQUIRE_GPU").is_none(), "a GPU adapter is required");
    }
    for scale in [1.0, 2.0] {
        let expected = cpu.render(drawing, scale);
        inspect_cpu(&expected, scale);
        if let Some(gpu) = &mut gpu {
            let actual = gpu.render(drawing, scale);
            assert_eq!((actual.width(), actual.height()), (expected.width(), expected.height()));
            let worst = expected.rgba().iter().zip(actual.rgba()).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
            assert!(worst <= 2, "largest CPU/GPU channel difference at {scale}x is {worst}");
        }
    }
}

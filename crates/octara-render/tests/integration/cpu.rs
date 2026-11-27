//! Deterministic reference pixels for quads, borders, text, effects and clips.

use super::fixtures::{glyphs, list, quad};
use octara_model::capture::{PaintFallback, PaintOutcome};
use octara_model::geometry::{Rect, point};
use octara_model::identity::NodeId;
use octara_model::native::NativeSlot;
use octara_model::paint::{CustomPaint, Item, PaintProgram, TransitionPaint};
use octara_model::theme::{Color, Gradient, rgb};
use octara_render::CpuRenderer;

#[test]
fn rounds_only_the_authored_corners_in_fills_and_clips() {
    let source = NodeId::default();
    let rect = Rect::new(0.0, 0.0, 30.0, 20.0);
    let radii = [16.0, 0.0, 8.0, 0.0];
    let lists = [
        list((30.0, 20.0), Color::WHITE, [Item::Quad(octara_model::paint::Quad { radius: radii, ..quad(rect, 0.0, Color::BLACK) })]),
        list(
            (30.0, 20.0),
            Color::WHITE,
            [Item::PushClip { source, rect, radius: radii }, Item::Quad(quad(rect, 0.0, Color::BLACK)), Item::PopClip],
        ),
    ];
    let mut renderer = CpuRenderer::new();
    for list in lists {
        let rendered = renderer.render(&list, 1.0);
        for (x, y) in [(0, 0), (4, 1), (29, 19)] {
            assert_eq!(rendered.pixel(x, y), Some([255; 4]));
        }
        for (x, y) in [(29, 0), (0, 19), (15, 10)] {
            assert_eq!(rendered.pixel(x, y), Some([0, 0, 0, 255]));
        }
    }
}

#[test]
fn a_wide_border_does_not_flatten_corners_on_the_opposite_side() {
    let panel = octara_model::paint::Quad {
        radius: [0.0, 0.0, 0.0, 30.0],
        border: [0.0, 24.0, 0.0, 8.0],
        border_color: Color::BLACK,
        ..quad(Rect::new(0.0, 0.0, 80.0, 80.0), 0.0, Color::WHITE)
    };
    let scene = list((80.0, 80.0), Color::WHITE, [Item::Quad(panel)]);
    assert_eq!(CpuRenderer::new().render(&scene, 1.0).pixel(8, 69), Some([0, 0, 0, 255]));
}

#[test]
fn deterministic_and_pixels_where_expected() {
    let card = Rect::new(8.0, 8.0, 140.0, 68.0);
    let rows = Rect::new(8.0, 80.0, 140.0, 34.0);
    let accent = rgb(0x2455bb);
    let border = rgb(0xb52233);
    let ink = Color::WHITE;
    let background = rgb(0xeeeeee);
    let source = NodeId::default();
    let list = list(
        (160.0, 120.0),
        background,
        [
            Item::Quad(octara_model::paint::Quad { border: [2.0; 4], border_color: border, ..quad(card, 12.0, accent) }),
            glyphs("你好", point(20.0, 52.0), 30.0, ink),
            Item::Quad(quad(rows, 0.0, accent)),
            Item::PushClip { source, rect: rows, radius: [0.0; 4] },
            Item::Quad(quad(Rect::new(40.0, 106.0, 60.0, 14.0), 0.0, border)),
            Item::PopClip,
        ],
    );
    let a = CpuRenderer::new().render(&list, 2.0);
    let b = CpuRenderer::new().render(&list, 2.0);
    assert_eq!((a.width(), a.height()), (320, 240));
    assert_eq!(a, b);

    let px = |x: u32, y: u32| a.pixel(2 * x, 2 * y).unwrap();
    assert_eq!(px(130, 60), accent.to_rgba8().to_u8_array());
    assert_eq!(px(147, 40), border.to_rgba8().to_u8_array());
    assert_eq!(px(8, 8), background.to_rgba8().to_u8_array());
    let inked = (40..104).flat_map(|x| (50..105).map(move |y| (x, y))).filter(|&(x, y)| a.pixel(x, y).unwrap() == [255; 4]).count();
    assert!(inked > 200, "glyph pixels: {inked}");
    assert_eq!(px(80, 110), border.to_rgba8().to_u8_array());
    assert_eq!(px(80, 116), background.to_rgba8().to_u8_array());
}

#[test]
fn synthetic_italic_uses_a_separate_glyph_image() {
    let mut renderer = CpuRenderer::new();
    let Item::Glyphs(run) = glyphs("你", point(8.0, 30.0), 24.0, Color::BLACK) else { panic!("glyph fixture must return a run") };
    let normal = renderer.render(&list((48.0, 36.0), Color::WHITE, [Item::Glyphs(run.clone())]), 1.0);
    let italic =
        renderer.render(&list((48.0, 36.0), Color::WHITE, [Item::Glyphs(octara_model::paint::GlyphRun { skew: 14.0, ..run })]), 1.0);
    assert_ne!(normal, italic);
}

#[test]
fn per_glyph_colors_match_individually_colored_runs() {
    let Item::Glyphs(run) = glyphs("你好世界", point(4.25, 26.0), 20.0, Color::BLACK) else {
        panic!("glyph fixture must return a run")
    };
    let colors = [rgb(0xe75d46), rgb(0x1677c8).with_alpha(0.5), Color::TRANSPARENT, rgb(0x29ae63)];
    let individual = run
        .glyphs
        .iter()
        .zip(colors)
        .map(|(&glyph, color)| Item::Glyphs(octara_model::paint::GlyphRun { color, glyphs: [glyph].into(), ..run.clone() }));
    let expected = list((96.0, 32.0), Color::TRANSPARENT, individual);
    let styled =
        list((96.0, 32.0), Color::TRANSPARENT, [Item::Glyphs(octara_model::paint::GlyphRun { colors: Some(colors.into()), ..run })]);
    let mut cpu = CpuRenderer::new();
    assert_eq!(cpu.render(&styled, 1.25), cpu.render(&expected, 1.25));
}

#[test]
fn custom_paint_fallback_matches_a_clipped_quad_in_an_opacity_group() {
    let source = NodeId::default();
    let base = quad(Rect::new(20.0, 20.0, 56.0, 56.0), 12.0, Color::BLACK);
    let fallback = octara_model::paint::Quad {
        border: [2.0; 4],
        border_color: rgb(0xc12222),
        gradient: Some(Gradient { angle: 90.0, from: (rgb(0x3355bb), 0.0), to: (Color::WHITE, 1.0) }),
        ..base
    };
    let render = |item| {
        let list = list(
            (80.0, 80.0),
            Color::WHITE,
            [
                Item::PushClip { source, rect: Rect::new(12.0, 12.0, 48.0, 48.0), radius: [12.0; 4] },
                Item::PushLayer { source, opacity: 0.5 },
                item,
                Item::Quad(quad(Rect::new(24.0, 24.0, 12.0, 12.0), 0.0, Color::BLACK)),
                Item::PopLayer,
                Item::PopClip,
            ],
        );
        CpuRenderer::new().paint_with_receipts(&list, 1.0, None).unwrap()
    };
    let painted = render(Item::Custom(CustomPaint {
        fallback,
        program: PaintProgram::new(()),
        mount: Default::default(),
        exiting: false,
        input: Default::default(),
        opacity: 1.0,
    }));
    let [effect] = painted.effects.as_slice() else { panic!("expected a custom paint outcome") };
    assert_eq!(effect.outcome, PaintOutcome::Fallback { reason: PaintFallback::ReferenceRenderer });
    assert_ne!(painted.picture.pixel(40, 40), painted.picture.pixel(0, 0));
    assert_eq!(painted.picture.pixel(66, 40), painted.picture.pixel(0, 0));
    assert_eq!(painted.picture, render(Item::Quad(fallback)).picture);
}

#[test]
fn render_and_capture_preserve_straight_alpha() {
    let mut cpu = CpuRenderer::new();
    for (background, expected) in [(Color::new([1.0, 0.25, 0.5, 0.5]), [255, 64, 128, 128]), (Color::new([1.0, 0.0, 1.0, 0.0]), [0; 4])] {
        let display = list((3.25, 2.5), background, []);
        let picture = cpu.render(&display, 2.0);
        assert_eq!((picture.width(), picture.height()), (7, 5));
        assert_eq!(picture.pixel(0, 0), Some(expected));
        assert_eq!(picture, cpu.paint(&display, 2.0).unwrap());
    }
}

#[test]
fn a_transition_fallback_keeps_an_underlay_cutout_in_the_parent() {
    let source = NodeId::default();
    let list = list(
        (32.0, 32.0),
        Color::WHITE,
        [
            Item::PushTransition(TransitionPaint {
                source,
                mount: Default::default(),
                run: Default::default(),
                revision: 1,
                program: PaintProgram::new(()),
                progress: 0.5,
                opacity: 1.0,
            }),
            Item::Native { source, rect: Rect::new(10.0, 10.0, 12.0, 12.0), radius: [0.0; 4], slot: NativeSlot::Underlay, cut: true },
            Item::PopLayer,
        ],
    );
    let painted = CpuRenderer::new().paint_with_receipts(&list, 1.0, None).unwrap();
    assert_eq!(painted.picture.pixel(16, 16), Some([0; 4]));
    assert_eq!(painted.picture.pixel(4, 4), Some([255; 4]));
    assert_eq!(painted.effects[0].outcome, PaintOutcome::Fallback { reason: PaintFallback::ReferenceRenderer });
}

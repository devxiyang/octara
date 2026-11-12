//! The GPU renderer against the reference renderer: same display list, same pixels within two
//! levels per channel (premultiplied). Skipped when there is no GPU.

use octara::prelude::*;
use octara_render::{Bitmap, CpuRenderer, GpuRenderer};

fn compare(name: &str, cpu: &Bitmap, gpu: &Bitmap) {
    assert_eq!((cpu.width, cpu.height), (gpu.width, gpu.height), "{name}: size");
    let (mut worst, mut off, mut at) = (0u8, 0usize, (0, 0));
    for y in 0..cpu.height {
        for x in 0..cpu.width {
            // Premultiplied: a nearly transparent pixel's straight color is ill-conditioned
            // (8-bit premultiplied targets keep few levels of it), its contribution is not.
            let pre = |p: [u8; 4]| [0, 1, 2].map(|i| ((p[i] as u32 * p[3] as u32 + 127) / 255) as u8);
            let (a, b) = (cpu.pixel(x, y), gpu.pixel(x, y));
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
        cpu.pixel(at.0, at.1),
        gpu.pixel(at.0, at.1)
    );
}

fn card() -> View {
    col()
        .size_full()
        .p_4()
        .gap_4()
        .child(
            row()
                .w(240)
                .h(80)
                .p_3()
                .rounded_lg()
                .border_2()
                .bg(Token::Accent)
                .border_color(Token::Danger)
                .child(text("Hello, 世界 ✓").text_color(Token::AccentText).text_2xl()),
        )
        .child(el().size_16().rounded_full().bg(Token::Accent).opacity(0.5))
        .child(
            col()
                .w(200)
                .h(60)
                .rounded_xl()
                .overflow_y_scroll()
                .bg(Token::Surface)
                .children((0..10).map(|i| text(format!("row {i} with some words")))),
        )
        .into()
}

fn shadows() -> View {
    row()
        .size_full()
        .p_8()
        .gap_8()
        .flex_wrap()
        .bg(Token::Background)
        .child(el().size_16().rounded_md().bg(Token::Surface).shadow_sm())
        .child(el().size_16().rounded_lg().bg(Token::Surface).shadow_lg())
        .child(el().size_16().rounded_full().bg(Token::Surface).shadow_2xl())
        .child(el().size_16().bg(Token::Surface).shadow([BoxShadow::new(6.0, 6.0, rgb(0x2563eb)).spread_radius(2.0)]))
        .child(
            el().w(80)
                .h(40)
                .rounded_md()
                .overflow_hidden()
                .bg(Token::Surface)
                .child(el().size_8().m_2().rounded_sm().bg(Token::Accent).shadow_md()),
        )
        .into()
}

fn gradients() -> View {
    row()
        .size_full()
        .p_6()
        .gap_4()
        .flex_wrap()
        .child(el().w(120).h(60).rounded_lg().bg(linear_gradient(
            90.0,
            linear_color_stop(rgb(0x3b82f6), 0.0),
            linear_color_stop(rgb(0xec4899), 1.0),
        )))
        .child(el().w(120).h(60).rounded_full().bg(linear_gradient(
            180.0,
            linear_color_stop(Token::Accent, 0.2),
            linear_color_stop(Token::Surface, 0.8),
        )))
        .child(el().size_16().border_2().border_color(Token::Border).bg(linear_gradient(
            45.0,
            linear_color_stop(rgb(0x10b981), 0.0),
            linear_color_stop(Color::TRANSPARENT, 1.0),
        )))
        .child(el().w(80).h(40).opacity(0.5).bg(linear_gradient(
            315.0,
            linear_color_stop(rgb(0xf59e0b), 0.0),
            linear_color_stop(rgb(0x7c3aed), 1.0),
        )))
        .into()
}

/// A 24×16 test image: color ramps, a transparent corner, a hard checker.
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

fn images() -> View {
    let img = picture();
    row()
        .size_full()
        .p_4()
        .gap_3()
        .flex_wrap()
        .items_start()
        .child(el().img(&img))
        .child(el().w(96).img(&img).rounded_lg())
        .child(el().size_16().img(&img).object_fit(ObjectFit::Contain).bg(Token::Surface))
        .child(el().size_16().img(&img).object_fit(ObjectFit::Cover).rounded_full())
        .child(el().w(120).h(40).img(&img).object_fit(ObjectFit::None).border_2().border_color(Token::Border))
        .child(el().w(12).img(&img).opacity(0.6))
        .into()
}

fn layers() -> View {
    let overlap = || {
        el().w(120)
            .h(70)
            .child(el().absolute().top_0().left_0().size_12().rounded_md().bg(rgb(0xef4444)))
            .child(el().absolute().top_4().left_8().size_12().rounded_full().bg(rgb(0x3b82f6)))
            .child(text("Group").absolute().bottom_0().right_0().text_lg())
    };
    row()
        .size_full()
        .p_4()
        .gap_4()
        .flex_wrap()
        .bg(Token::Background)
        .child(overlap().opacity(0.5))
        .child(overlap().opacity(0.8).child(el().absolute().top_2().left_2().size_6().bg(rgb(0x10b981)).opacity(0.5)))
        .child(el().w(80).h(50).rounded_lg().overflow_hidden().child(overlap().opacity(0.6)))
        .child(el().size_12().rounded_md().bg(Token::Accent).opacity(0.4))
        .into()
}

#[test]
fn gpu_matches_the_reference_renderer() {
    let Some(mut gpu) = GpuRenderer::new() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let mut cpu = CpuRenderer::new();
    type Case = (&'static str, fn() -> View);
    let cases: [Case; 8] = [
        ("card", card),
        ("todo", || octara_examples_like::todo()),
        ("settings", || octara_examples_like::settings()),
        ("dark", card),
        ("shadows", shadows),
        ("gradients", gradients),
        ("images", images),
        ("layers", layers),
    ];
    for (name, view) in cases {
        let app = Window::headless(view);
        if name == "dark" {
            app.app().set_theme(Theme::dark());
        }
        for scale in [1.0, 2.0] {
            let dl = app.display_list();
            compare(&format!("{name}@{scale}x"), &cpu.render(&dl, scale), &gpu.render(&dl, scale));
        }
    }
}

/// Paths of every kind, and one larger than an atlas tile.
fn paths_list() -> octara::paint::DisplayList {
    use octara::kernel::NodeId;
    use octara::paint::{DisplayList, Gradient, Item, Part, PathPaint, Rect};
    use octara::path::{FillRule, LineCap, LineJoin, Path, PathStyle, Stroke, Transform, point};
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
        Item::PushClip { source: NodeId::default(), rect: Rect { x: 20.0, y: 120.0, w: 120.0, h: 60.0 }, radius: 16.0 },
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
    DisplayList { size: (640.0, 400.0), background: Color::WHITE, parts: items.into_iter().map(Part::Item).collect() }
}

#[test]
fn gpu_paths_match_the_reference_renderer() {
    let Some(mut gpu) = GpuRenderer::new() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let mut cpu = CpuRenderer::new();
    let dl = paths_list();
    for scale in [1.0, 2.0] {
        compare(&format!("paths@{scale}x"), &cpu.render(&dl, scale), &gpu.render(&dl, scale));
    }
}

/// Small stand-ins for the example apps (this crate does not depend on the examples).
mod octara_examples_like {
    use octara::prelude::*;

    pub fn todo() -> View {
        col()
            .size_full()
            .p_4()
            .gap_2()
            .children((1..=8).map(|i| {
                row()
                    .gap_2()
                    .items_center()
                    .child(el().size_4().rounded_sm().border().bg(Token::Background))
                    .child(text(format!("Item {i}")))
                    .child(text("Delete").text_sm().px_3().py_1p5().rounded_md())
            }))
            .into()
    }

    pub fn settings() -> View {
        col()
            .size_full()
            .p_4()
            .gap_3()
            .child(row().w_8().h_5().p(1.0).rounded_full().bg(Token::Border).child(el().size_4().rounded_full().bg(Token::Background)))
            .child(text("notifications=off sound=on volume=50 theme=system").text_color(Token::TextMuted))
            .into()
    }
}

/// An underlay view in a card, a button painted over it: the card is cut to transparent where
/// the view is, the button stays.
fn underlay() -> View {
    el().size_full()
        .bg(Token::Surface)
        .child(
            el().absolute()
                .top(px(10.0))
                .left(px(10.0))
                .p_4()
                .rounded_xl()
                .bg(Token::Background)
                .child(native_view("map").slot(octara::native::Slot::Underlay).label("Map").w(px(160.0)).h(px(90.0)).rounded_lg()),
        )
        .child(el().absolute().top(px(40.0)).left(px(40.0)).size_8().rounded_full().bg(Token::Accent))
        .into()
}

#[test]
fn underlay_cutouts_clear_to_transparent() {
    let app = Window::headless_on(
        octara::platform::HeadlessPlatform::with_window(octara::platform::HeadlessWindow { shows_native: true, ..Default::default() }),
        underlay,
    );
    let dl = app.display_list();
    let cpu = CpuRenderer::new().render(&dl, 1.0);
    assert_eq!(cpu.pixel(150, 80)[3], 0, "the view shows through");
    assert_eq!(cpu.pixel(56, 56)[3], 255, "the button painted after it stays");
    assert_eq!(cpu.pixel(5, 5)[3], 255, "the page around it stays");
    let Some(mut gpu) = GpuRenderer::new() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    for scale in [1.0, 2.0] {
        compare(&format!("underlay@{scale}x"), &CpuRenderer::new().render(&dl, scale), &gpu.render(&dl, scale));
    }
}

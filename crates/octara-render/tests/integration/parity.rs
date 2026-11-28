//! Scenes and the premultiplied pixel comparator used by both GPU backends.

use super::fixtures::{glyphs, list, quad};
use octara_model::geometry::{Rect, point};
use octara_model::identity::NodeId;
use octara_model::paint::{DisplayList, Image, ImagePaint, Item, Part, Picture, Quad, Shadow};
use octara_model::theme::{Color, Gradient, rgb};

pub fn compare(name: &str, cpu: &Picture, gpu: &Picture) {
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

pub fn picture() -> Image {
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

pub fn sample(image: &Image, rect: Rect, bounds: Rect, radius: f32, opacity: f32) -> Item {
    Item::Image(ImagePaint { source: NodeId::default(), image: image.sample(), rect, bounds, radius: [radius; 4], opacity })
}

pub fn scene(background: Color) -> DisplayList {
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

/// Paths of every kind, and one larger than an atlas tile.
pub fn paths_list() -> DisplayList {
    use octara_model::paint::PathPaint;
    use octara_model::paint::{FillRule, LineCap, LineJoin, Path, PathStyle, Stroke, Transform};
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

pub fn minified_images() -> DisplayList {
    let stripes = (0..64 * 64).flat_map(|i| if i / 64 % 4 == 0 { [255, 0, 0, 255] } else { [0, 255, 255, 0] }).collect::<Vec<_>>();
    let red = Image::rgba(64, 64, stripes);
    let green = Image::rgba(64, 64, [0, 255, 0, 255].repeat(64 * 64));
    let blue = Image::rgba(64, 64, [0, 0, 255, 255].repeat(64 * 64));
    list(
        (66.0, 24.0),
        Color::TRANSPARENT,
        [&green, &red, &blue].into_iter().enumerate().map(|(i, image)| {
            let x = 1.25 + 22.0 * i as f32;
            sample(image, Rect::new(x, 2.0, 16.0, 16.0), Rect::new(x - 1.0, 1.0, 18.0, 18.0), 0.0, 1.0)
        }),
    )
}

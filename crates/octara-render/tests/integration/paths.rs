//! Paths in the reference renderer: fills with both rules, strokes with caps, joins and dashes,
//! gradients, transforms, clips; masks cached by content and dropped when unused.

use octara::kernel::{Color, NodeId};
use octara::paint::{DisplayList, Gradient, Item, PathPaint, Rect};
use octara::path::{FillRule, LineCap, LineJoin, Path, PathStyle, Stroke, Transform, point};
use octara::prelude::rgb;
use octara_render::{Bitmap, CpuRenderer};

fn paint(path: Path, style: PathStyle, color: Color) -> Item {
    let bounds = path.bounds().unwrap_or_default();
    Item::Path(PathPaint { source: NodeId::default(), path, transform: Transform::IDENTITY, style, color, gradient: None, bounds })
}

fn list(items: Vec<Item>) -> DisplayList {
    DisplayList { size: (100.0, 60.0), background: Color::WHITE, items }
}

fn rgb8(b: &Bitmap, x: u32, y: u32) -> [u8; 3] {
    let p = b.pixel(x, y);
    [p[0], p[1], p[2]]
}

const RED: [u8; 3] = [255, 0, 0];
const WHITE: [u8; 3] = [255, 255, 255];

#[test]
fn fills_follow_their_rule() {
    let ring = Path::circle(point(30.0, 30.0), 20.0).append(&Path::circle(point(30.0, 30.0), 10.0));
    let red = rgb(0xff0000);
    let mut cpu = CpuRenderer::new();
    let even_odd = cpu.render(&list(vec![paint(ring.clone(), PathStyle::Fill(FillRule::EvenOdd), red)]), 1.0);
    assert_eq!(rgb8(&even_odd, 30, 30), WHITE, "the hole");
    assert_eq!(rgb8(&even_odd, 30, 15), RED, "the ring");
    assert_eq!(rgb8(&even_odd, 80, 30), WHITE, "outside");
    let non_zero = cpu.render(&list(vec![paint(ring, PathStyle::Fill(FillRule::NonZero), red)]), 1.0);
    assert_eq!(rgb8(&non_zero, 30, 30), RED, "same winding: filled");
    // Edges are antialiased: some pixel on the outer circle is between the two colors.
    assert!((0..60).any(|y| {
        let p = rgb8(&non_zero, 10, y);
        p[1] > 0 && p[1] < 255
    }));
}

#[test]
fn strokes_have_caps_joins_and_dashes() {
    let red = rgb(0xff0000);
    let line = Path::new().move_to(point(20.0, 30.0)).line_to(point(80.0, 30.0));
    let mut cpu = CpuRenderer::new();
    let butt = cpu.render(&list(vec![paint(line.clone(), PathStyle::Stroke(Stroke::new(10.0)), red)]), 1.0);
    let round = cpu.render(&list(vec![paint(line.clone(), PathStyle::Stroke(Stroke::new(10.0).cap(LineCap::Round)), red)]), 1.0);
    assert_eq!(rgb8(&butt, 50, 30), RED);
    assert_eq!(rgb8(&butt, 50, 37), WHITE, "half the width each side");
    assert_eq!(rgb8(&butt, 17, 30), WHITE, "a butt cap ends at the point");
    assert_eq!(rgb8(&round, 17, 30), RED, "a round cap goes past it");
    let dashed = cpu.render(&list(vec![paint(line, PathStyle::Stroke(Stroke::new(4.0).dash(&[10.0, 10.0])), red)]), 1.0);
    assert_eq!(rgb8(&dashed, 25, 30), RED, "a dash");
    assert_eq!(rgb8(&dashed, 35, 30), WHITE, "a gap");

    let corner = Path::new().move_to(point(20.0, 50.0)).line_to(point(50.0, 10.0)).line_to(point(80.0, 50.0));
    let miter = cpu.render(&list(vec![paint(corner.clone(), PathStyle::Stroke(Stroke::new(8.0)), red)]), 1.0);
    let bevel = cpu.render(&list(vec![paint(corner, PathStyle::Stroke(Stroke::new(8.0).join(LineJoin::Bevel)), red)]), 1.0);
    assert_eq!(rgb8(&miter, 50, 6), RED, "a miter reaches the tip");
    assert_eq!(rgb8(&bevel, 50, 6), WHITE, "a bevel cuts it");
}

#[test]
fn gradients_transforms_clips_and_scale() {
    let square = Path::rect(Rect { x: 0.0, y: 0.0, w: 20.0, h: 20.0 });
    let mut item = PathPaint {
        source: NodeId::default(),
        path: square.clone(),
        transform: Transform::translate(10.0, 10.0),
        style: PathStyle::Fill(FillRule::NonZero),
        color: Color::BLACK,
        gradient: Some(Gradient { angle: 90.0, from: (rgb(0xff0000), 0.0), to: (rgb(0x0000ff), 1.0) }),
        bounds: Rect { x: 10.0, y: 10.0, w: 20.0, h: 20.0 },
    };
    let mut cpu = CpuRenderer::new();
    let b = cpu.render(&list(vec![Item::Path(item.clone())]), 1.0);
    let (left, right) = (rgb8(&b, 11, 20), rgb8(&b, 28, 20));
    assert!(left[0] > 200 && left[2] < 60 && right[2] > 200 && right[0] < 60, "{left:?} {right:?}");

    // Rotated 45° about its center: a diamond whose corners reach past the square's sides.
    item.gradient = None;
    item.color = rgb(0xff0000);
    item.transform = Transform::translate(-10.0, -10.0).then(&Transform::rotate(45.0)).then(&Transform::translate(50.0, 30.0));
    let d = cpu.render(&list(vec![Item::Path(item.clone())]), 1.0);
    assert_eq!(rgb8(&d, 50, 30), RED);
    assert_eq!(rgb8(&d, 50, 18), RED, "the top corner, past where the square's side was");
    assert_eq!(rgb8(&d, 41, 21), WHITE, "the square's corner is cut");

    // Clipped to the left half; at 2× the same picture, twice as big.
    let clip = Item::PushClip { source: NodeId::default(), rect: Rect { x: 0.0, y: 0.0, w: 50.0, h: 60.0 }, radius: 0.0 };
    let c = cpu.render(&list(vec![clip.clone(), Item::Path(item.clone()), Item::PopClip]), 1.0);
    assert_eq!((rgb8(&c, 45, 30), rgb8(&c, 55, 30)), (RED, WHITE));
    let c2 = cpu.render(&list(vec![clip, Item::Path(item), Item::PopClip]), 2.0);
    assert_eq!((c2.width, rgb8(&c2, 90, 60), rgb8(&c2, 110, 60)), (200, RED, WHITE));
}

#[test]
fn masks_are_cached_by_content_and_dropped_when_unused() {
    let mut cpu = CpuRenderer::new();
    let red = rgb(0xff0000);
    let a = |x: f32| {
        let mut i = paint(Path::circle(point(10.0, 10.0), 8.0), PathStyle::Fill(FillRule::NonZero), red);
        if let Item::Path(p) = &mut i {
            p.transform = Transform::translate(x, 0.0);
        }
        i
    };
    // The same circle rebuilt and moved by whole pixels: one mask.
    let first = cpu.render(&list(vec![a(0.0), a(20.0), a(40.0)]), 1.0);
    assert_eq!(cpu.cached_paths(), 1);
    // A quarter-pixel offset is another phase.
    cpu.render(&list(vec![a(0.25)]), 1.0);
    assert_eq!(cpu.cached_paths(), 2);
    // Moved by a whole pixel, the picture moves by a pixel.
    let moved = cpu.render(&list(vec![a(1.0)]), 1.0);
    assert_eq!(rgb8(&first, 2, 10), rgb8(&moved, 3, 10));
    for _ in 0..200 {
        cpu.render(&list(vec![]), 1.0);
    }
    assert_eq!(cpu.cached_paths(), 0, "unused masks are dropped");
}

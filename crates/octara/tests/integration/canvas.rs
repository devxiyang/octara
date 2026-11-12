//! Canvas: a recording made by a binding over the element's size and what it reads; drawn in
//! the content box, clipped to it; an image to assistive technology.

use octara::canvas::CanvasOp;
use octara::paint::Item;
use octara::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

fn bars(runs: Rc<Cell<u32>>) -> impl Fn() -> View {
    move || {
        let runs = runs.clone();
        let n = state("n", || 2usize);
        col()
            .size_full()
            .child(el().id("more").role(Role::Button).label("More").on_press(move || n.update(|v| *v += 1)).size_8())
            .child(
                canvas(move |c| {
                    runs.set(runs.get() + 1);
                    let (w, h) = c.size();
                    let count = n.get();
                    let bw = w / count as f32;
                    for i in 0..count {
                        let bh = h * (i + 1) as f32 / count as f32;
                        c.rect(Rect::new(i as f32 * bw, h - bh, bw - 2.0, bh), Token::Accent);
                    }
                    c.stroke(&Path::new().move_to((0.0, h)).line_to((w, h)), &Stroke::new(1.0), Token::Border);
                })
                .id("chart")
                .label("Sales")
                .w_full()
                .h(100)
                .p_2(),
            )
            .into()
    }
}

#[test]
fn the_recording_follows_size_and_reads() {
    let runs = Rc::new(Cell::new(0));
    let mut app = App::new(bars(runs.clone()));
    let k = app.kernel();
    let chart = k.find("chart").unwrap();
    // The content box: 800 wide less 2 × 8 padding, 100 high less 16.
    let ops = k.canvas(chart).unwrap().to_vec();
    assert_eq!(ops.len(), 3, "two bars and a baseline");
    assert!(matches!(&ops[1], CanvasOp::Path { path, .. } if path.bounds().unwrap().right() == 784.0 - 2.0));
    drop(k);
    let after_mount = runs.get();

    // Laid out again with nothing changed: no rerun.
    app.resize(800.0, 600.0);
    assert_eq!(runs.get(), after_mount);
    // What it reads changed: one rerun, three bars.
    app.tap("more").unwrap();
    assert_eq!(runs.get(), after_mount + 1);
    assert_eq!(app.kernel().canvas(chart).unwrap().len(), 4);
    // Its size changed: one rerun, at the new width.
    app.resize(400.0, 600.0);
    assert_eq!(runs.get(), after_mount + 2);
    let k = app.kernel();
    assert!(matches!(&k.canvas(chart).unwrap()[3], CanvasOp::Path { path, .. } if path.bounds().unwrap().w == 384.0));
}

#[test]
fn drawn_in_the_content_box_clipped_and_themed() {
    let mut app = App::new(bars(Rc::default()));
    let dl = app.display_list();
    let k = app.kernel();
    let chart = k.find("chart").unwrap();
    let (x, y, ..) = k.rect(chart);
    drop(k);
    let items: Vec<&Item> = dl
        .items
        .iter()
        .filter(|i| matches!(i, Item::Path(p) if p.source == chart) || matches!(i, Item::PushClip { source, .. } if *source == chart))
        .collect();
    let Item::PushClip { rect, .. } = items[0] else { panic!("the canvas clips to its content box first") };
    assert_eq!((rect.x, rect.y, rect.w, rect.h), (x + 8.0, y + 8.0, 784.0, 84.0));
    let Item::Path(bar) = items[1] else { panic!() };
    assert_eq!((bar.transform.x, bar.transform.y), (x + 8.0, y + 8.0), "offset to the content box");
    assert_eq!(bar.color, Theme::light().get(Token::Accent));
    app.set_theme(Theme::dark());
    let dark = app.display_list();
    assert!(dark.paths().any(|p| p.source == chart && p.color == Theme::dark().get(Token::Accent)), "tokens follow the theme");
}

#[test]
fn transforms_clips_gradients_and_semantics() {
    let app = App::new(|| {
        row()
            .child(
                canvas(|c| {
                    c.clip(Rect::new(0.0, 0.0, 20.0, 20.0), 4.0, |c| {
                        c.transform(Transform::translate(10.0, 0.0), |c| {
                            c.transform(Transform::scale(2.0, 2.0), |c| {
                                c.fill(
                                    &Path::circle((5.0, 5.0), 5.0),
                                    linear_gradient(90.0, linear_color_stop(rgb(0xff0000), 0.0), linear_color_stop(rgb(0x0000ff), 1.0)),
                                );
                            });
                        });
                    });
                })
                .id("art")
                .size_16(),
            )
            .child(canvas(|c| c.rect(Rect::new(0.0, 0.0, 4.0, 4.0), Token::Border)).id("deco").size_4().a11y_hidden())
            .into()
    });
    let k = app.kernel();
    let art = k.find("art").unwrap();
    let ops = k.canvas(art).unwrap();
    assert!(matches!(ops[0], CanvasOp::PushClip { rect, radius: 4.0 } if rect == Rect::new(0.0, 0.0, 20.0, 20.0)));
    // Scaled, then moved: the circle spans 10..30 across.
    let CanvasOp::Path { path, transform, .. } = &ops[1] else { panic!() };
    let b = path.bounds().unwrap();
    let (a, z) = (transform.apply(point(b.x, b.y)), transform.apply(point(b.right(), b.bottom())));
    assert_eq!((a.x, a.y, z.x, z.y), (10.0, 0.0, 30.0, 20.0));
    drop(k);
    let dl = app.display_list();
    let p = dl.paths().find(|p| p.source == art).unwrap();
    let (x, y, ..) = app.kernel().rect(art);
    assert_eq!((p.bounds.x, p.bounds.y, p.bounds.w, p.bounds.h), (x + 10.0, y, 20.0, 20.0), "a gradient spans the shape");
    assert!(p.gradient.is_some());

    // An image to assistive technology; unlabeled, `check` reports it; hidden decoration is fine.
    let snap = app.inspect();
    assert_eq!(snap.find("art").unwrap().role, "image");
    let issues = app.kernel().accessibility_issues();
    let codes: Vec<_> = issues.iter().map(|i| (i.code.id, i.test_id.as_deref())).collect();
    assert_eq!(codes, [("UNLABELED_CANVAS", Some("art"))]);
}

#[test]
fn text_takes_the_canvas_style_and_its_anchor() {
    let app = App::new(|| {
        row()
            .child(
                canvas(|c| {
                    let (w, h) = c.size();
                    c.text("Mid", (w / 2.0, h / 2.0)).align(Align::Center).valign(Align::Center);
                    c.text("Big", (0.0, 0.0)).size(28.0).weight(600).color(rgb(0xff0000));
                    c.transform(Transform::scale(2.0, 2.0), |c| {
                        c.text("Twice", (10.0, 10.0));
                    });
                })
                .id("labels")
                .label("Labels")
                .w(200)
                .h(100)
                .text_sm()
                .text_color(Token::TextMuted),
            )
            .into()
    });
    let dl = app.display_list();
    let k = app.kernel();
    let n = k.find("labels").unwrap();
    let (x, y, ..) = k.rect(n);
    drop(k);
    let runs: Vec<_> = dl.glyph_runs().filter(|r| r.source == n).collect();
    assert_eq!(runs.len(), 3);
    // "Mid": 14 px in the muted color, centered on the middle of the box.
    let mid = runs[0];
    assert_eq!((mid.size, mid.color), (14.0, Theme::light().get(Token::TextMuted)));
    let (left, right) = (mid.glyphs[0].x, mid.glyphs.last().unwrap().x);
    assert!(left > x + 80.0 && left < x + 100.0 && right > x + 100.0 && right < x + 120.0, "{left} {right}");
    // "Big": its own size and color, at the top-left.
    let big = runs[1];
    assert_eq!((big.size, big.color), (28.0, rgb(0xff0000)));
    assert!((big.glyphs[0].x - x).abs() < 2.0 && big.glyphs[0].y > y && big.glyphs[0].y < y + 40.0);
    // Under a 2× scale: moved to (20, 20) and twice the size.
    let twice = runs[2];
    assert_eq!(twice.size, 28.0);
    assert!((twice.glyphs[0].x - (x + 20.0)).abs() < 2.0);
}

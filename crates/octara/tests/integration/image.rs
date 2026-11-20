//! Images: sizing like a replaced element, fitting, PNG decoding, and live pixels.

use octara::prelude::*;
use octara::view::canvas;
use octara_primitives::paint::{DisplayList, Item};

fn solid(w: u32, h: u32, c: [u8; 4]) -> Image {
    Image::rgba(w, h, c.repeat((w * h) as usize))
}

fn size(app: &Window, id: &str) -> (f32, f32) {
    let k = app.kernel();
    let (_, _, w, h) = k.rect(k.find(id).unwrap());
    (w, h)
}

#[test]
fn an_image_sizes_its_box() {
    let img = solid(40, 20, [255, 0, 0, 255]);
    let mut app = Window::headless(move || {
        col()
            .items_start()
            .child(image(&img).id("natural"))
            .child(image(&img).id("wide").w(100))
            .child(image(&img).id("tall").h(50))
            .child(image(&img).id("square").w(30).aspect_square())
            .child(image(&img).id("both").w(10).h(10))
    });
    assert_eq!(size(&app, "natural"), (40.0, 20.0));
    assert_eq!(size(&app, "wide"), (100.0, 50.0), "the height follows the aspect ratio");
    assert_eq!(size(&app, "tall"), (100.0, 50.0));
    assert_eq!(size(&app, "square"), (30.0, 30.0), "an explicit aspect ratio wins");
    assert_eq!(size(&app, "both"), (10.0, 10.0));
    let _ = &mut app;
}

#[test]
fn fits_place_the_image() {
    let b = (0.0, 0.0, 100.0, 50.0);
    assert_eq!(ObjectFit::Fill.place((20.0, 20.0), b), (0.0, 0.0, 100.0, 50.0));
    assert_eq!(ObjectFit::Contain.place((20.0, 20.0), b), (25.0, 0.0, 50.0, 50.0));
    assert_eq!(ObjectFit::Cover.place((20.0, 20.0), b), (0.0, -25.0, 100.0, 100.0));
    assert_eq!(ObjectFit::ScaleDown.place((20.0, 20.0), b), (40.0, 15.0, 20.0, 20.0));
    assert_eq!(ObjectFit::None.place((200.0, 20.0), b), (-50.0, 15.0, 200.0, 20.0));
}

#[test]
fn the_display_list_draws_it_clipped_to_the_box() {
    let img = solid(20, 20, [0, 0, 255, 255]);
    let app = Window::headless(move || {
        col().child(image(&img).id("pic").w(100).h(50).p_2().border_2().rounded_lg().object_fit(ObjectFit::Cover))
    });
    let n = app.kernel().find("pic").unwrap();
    let (x, y, w, h) = app.kernel().rect(n);
    let p = app.display_list().items().clone().into_iter().find_map(|i| if let Item::Image(p) = i { Some(p) } else { None }).unwrap();
    assert_eq!(p.source, n);
    // Inside the border (padding does not inset it, as CSS backgrounds); cover crops vertically.
    assert_eq!((p.bounds.x, p.bounds.y, p.bounds.w, p.bounds.h), (x + 2.0, y + 2.0, w - 4.0, h - 4.0));
    assert_eq!(p.rect.w, w - 4.0);
    assert!(p.rect.h > p.bounds.h);
    assert_eq!(p.radius, 8.0 - 2.0);
}

#[test]
fn png_decodes() {
    let mut bytes = Vec::new();
    {
        let mut e = png::Encoder::new(&mut bytes, 2, 1);
        e.set_color(png::ColorType::Rgb);
        e.set_depth(png::BitDepth::Eight);
        e.write_header().unwrap().write_image_data(&[255, 0, 0, 0, 255, 0]).unwrap();
    }
    let img = Image::decode(&bytes).unwrap();
    assert_eq!(img.size(), (2, 1));
    assert_eq!(&img.frame().rgba[..], &[255, 0, 0, 255, 0, 255, 0, 255]);
    assert!(Image::decode(b"not a png").unwrap_err().starts_with("PNG"));
}

#[test]
fn new_pixels_draw_without_rerunning_the_view() {
    let img = solid(4, 4, [0, 0, 0, 255]);
    let shown = img.clone();
    let app = Window::headless(move || col().child(image(&shown).id("pic").w(40)));
    app.app().reset_work();
    img.set(4, 4, [255u8, 255, 255, 255].repeat(16));
    app.app().settle();
    let frame = app
        .display_list()
        .items()
        .into_iter()
        .find_map(|i| if let Item::Image(p) = i { Some(p.image.frame().clone()) } else { None })
        .unwrap();
    assert_eq!(frame.version, 1);
    assert_eq!(frame.rgba[0], 255);
    assert_eq!(app.app().work().closures_run, 0);
}

#[test]
fn layout_and_kept_drawings_use_the_same_image_frame() {
    let img = solid(40, 20, [255, 0, 0, 255]);
    let empty = Image::rgba(0, 0, []);
    let (shown, initially_empty) = (img.clone(), empty.clone());
    let app = Window::headless(move || {
        let painted = shown.clone();
        col()
            .items_start()
            .child(image(&shown).id("natural"))
            .child(image(&shown).id("scaled").w(40))
            .child(canvas(move |c| c.image(&painted, Rect::new(0.0, 0.0, 30.0, 30.0))).size(30))
            .child(image(&initially_empty).id("empty"))
    });
    assert_eq!(size(&app, "empty"), (0.0, 0.0));
    let old = app.display_list();

    img.set(20, 40, [0, 255, 0, 255].repeat(800));
    empty.set(10, 12, [0, 0, 255, 255].repeat(120));
    assert!(std::sync::Arc::ptr_eq(&old, &app.display_list()), "reads return the prepared frame until the next update");
    app.app().settle();
    assert_eq!(size(&app, "natural"), (20.0, 40.0));
    assert_eq!(size(&app, "scaled"), (40.0, 80.0));
    assert_eq!(size(&app, "empty"), (10.0, 12.0));

    let samples = |list: &DisplayList| {
        list.items()
            .into_iter()
            .filter_map(|item| {
                if let Item::Image(paint) = item {
                    let f = paint.image.frame();
                    Some((paint.image.source().id(), f.version, f.width, f.height, <[u8; 4]>::try_from(&f.rgba[..4]).unwrap()))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
    };
    let mut expected = vec![(img.id(), 1, 20, 40, [0, 255, 0, 255]); 3];
    expected.push((empty.id(), 1, 10, 12, [0, 0, 255, 255]));
    assert_eq!(samples(&app.display_list()), expected);
    assert_eq!(samples(&old), vec![(img.id(), 0, 40, 20, [255, 0, 0, 255]); 3]);

    img.set(10, 20, [0, 255, 0, 255].repeat(200));
    app.app().settle();
    assert_eq!(size(&app, "natural"), (10.0, 20.0), "intrinsic size can change without changing aspect ratio");
    assert_eq!(size(&app, "scaled"), (40.0, 80.0));
}

#[test]
fn hidden_images_release_invalidated_cached_pixels() {
    let picture = solid(8, 8, [255, 0, 0, 255]);
    let old_pixels = std::sync::Arc::downgrade(&picture.frame().rgba);
    let shown = picture.clone();
    let mut app = Window::headless(move || {
        let visible = state("visible", || true);
        on_command("hide-picture", move || visible.set(false));
        image(&shown).id("picture").style_with(move |p| if visible.get() { p.flex() } else { p.hidden() })
    });
    let node = app.kernel().find("picture").unwrap();
    assert!(app.display_list().items().iter().any(|item| matches!(item, Item::Image(_))));
    app.invoke("hide-picture").unwrap();
    picture.set(8, 8, [0, 255, 0, 255].repeat(64));
    app.app().settle();
    assert!(app.kernel().is_alive(node), "hiding preserves the mounted element");
    assert!(!app.display_list().items().iter().any(|item| matches!(item, Item::Image(_))));
    assert!(old_pixels.upgrade().is_none(), "an invisible node must not retain an invalid image drawing");
}

//! The reference renderer: deterministic pixels for quads, borders, text and clips.

use octara::prelude::*;
use octara_render::CpuRenderer;

fn rgb(c: Color) -> [u8; 3] {
    let [r, g, b, _] = c.to_rgba8().to_u8_array();
    [r, g, b]
}

fn view() -> View {
    col()
        .size_full()
        .p_4()
        .gap_4()
        .child(
            row()
                .id("card")
                .w(200)
                .h(80)
                .p_3()
                .rounded_lg()
                .border_2()
                .bg(Token::Accent)
                .border_color(Token::Danger)
                .child(text("Hello").id("hello").text_color(Token::AccentText).text_2xl()),
        )
        .child(col().id("list").w(200).h(40).overflow_y_scroll().bg(Token::Surface).children((0..10).map(|i| text(format!("row {i}")))))
        .into()
}

#[test]
fn deterministic_and_pixels_where_expected() {
    let app = Window::headless(view);
    let list = app.display_list();
    let a = CpuRenderer::new().render(&list, 2.0);
    let b = CpuRenderer::new().render(&list, 2.0);
    assert_eq!((a.width, a.height), (1600, 1200));
    assert!(a == b, "the same frame renders to the same bytes");

    let t = Theme::light();
    let k = app.kernel();
    let card = k.rect(k.find("card").unwrap());
    drop(k);
    // Inside the card, away from text: the fill.
    let px = |x: f32, y: f32| a.pixel((x * 2.0) as u32, (y * 2.0) as u32);
    let inside = px(card.0 + card.2 - 10.0, card.1 + card.3 - 10.0);
    assert_eq!(inside[..3], rgb(t.get(Token::Accent)));
    // On the border (2 px wide, right edge, mid height).
    let edge = px(card.0 + card.2 - 1.0, card.1 + card.3 / 2.0);
    assert_eq!(edge[..3], rgb(t.get(Token::Danger)));
    // The rounded corner: the very corner pixel shows the page background.
    let corner = px(card.0 + 0.2, card.1 + 0.2);
    assert_eq!(corner[..3], rgb(t.get(Token::Background)));
    // Text was drawn: some pixels in the text's box are close to its color.
    let k = app.kernel();
    let hello = k.rect(k.find("hello").unwrap());
    drop(k);
    let text_color = rgb(t.get(Token::AccentText));
    let mut inked = 0;
    for y in (hello.1 * 2.0) as u32..((hello.1 + hello.3) * 2.0) as u32 {
        for x in (hello.0 * 2.0) as u32..((hello.0 + hello.2) * 2.0) as u32 {
            let p = a.pixel(x, y);
            if p[..3].iter().zip(text_color).all(|(a, b)| a.abs_diff(b) < 40) {
                inked += 1;
            }
        }
    }
    assert!(inked > 200, "glyph pixels: {inked}");

    // The scroll container clips its rows: just below it, the page background.
    let k = app.kernel();
    let list = k.rect(k.find("list").unwrap());
    drop(k);
    assert_eq!(px(list.0 + 20.0, list.1 + list.3 + 2.0)[..3], rgb(t.get(Token::Background)));
    assert_eq!(px(list.0 + 150.0, list.1 + 10.0)[..3], rgb(t.get(Token::Surface)));

    let png = a.encode_png().unwrap();
    assert_eq!(&png[1..4], b"PNG");
}

#[test]
fn custom_paint_fallback_matches_a_clipped_quad_in_an_opacity_group() {
    let render = |custom| {
        let mut window = Window::headless(move || {
            let card = el()
                .size_16()
                .flex_shrink_0()
                .rounded_lg()
                .border_2()
                .border_color(Token::Danger)
                .bg(linear_gradient(90.0, linear_color_stop(Token::Accent, 0.0), linear_color_stop(Token::Surface, 1.0)))
                .opacity(0.5)
                .child(el().size_8().bg(Token::Accent));
            let card = if custom { card.paint(PaintProgram::new(()), [0.0; 4]) } else { card };
            col().p_4().child(col().size_12().rounded_lg().overflow_clip().child(card))
        });
        window.resize(80.0, 80.0);
        CpuRenderer::new().render(&window.display_list(), 1.0)
    };
    let fallback = render(true);
    assert_ne!(fallback.pixel(40, 40), fallback.pixel(0, 0));
    assert_eq!(fallback.pixel(66, 40), fallback.pixel(0, 0));
    assert!(fallback == render(false));
}

#[test]
fn render_and_capture_preserve_straight_alpha_in_pngs() {
    use octara_primitives::capture::{Painter, Picture};
    use octara_primitives::paint::DisplayList;

    let mut cpu = CpuRenderer::new();
    for (background, expected) in [(Color::new([1.0, 0.25, 0.5, 0.5]), [255, 64, 128, 128]), (Color::new([1.0, 0.0, 1.0, 0.0]), [0; 4])] {
        let list = DisplayList { size: (3.25, 2.5), background, parts: vec![] };
        let picture: Picture = cpu.render(&list, 2.0);
        assert_eq!(picture.pixel(0, 0), expected);
        assert_eq!(picture, cpu.paint(&list, 2.0).unwrap());
        let frame = Image::decode(&picture.encode_png().unwrap()).unwrap().frame();
        assert_eq!((frame.width, frame.height), (7, 5));
        assert_eq!(frame.rgba.as_ref(), picture.rgba);
    }
}

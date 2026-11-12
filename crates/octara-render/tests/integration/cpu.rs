//! The reference renderer: deterministic pixels for quads, borders, text and clips.

use octara::prelude::*;
use octara_render::{CpuRenderer, screenshot};

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
    let a = screenshot(&app, 2.0);
    let b = CpuRenderer::new().render(&app.display_list(), 2.0);
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

    let png = a.encode_png();
    assert_eq!(&png[1..4], b"PNG");
}

//! Images: natural size, cover in a circle, contain.

use super::section;
use octara::prelude::*;

/// A generated landscape (no image files needed).
fn landscape() -> Image {
    let (w, h) = (240u32, 150u32);
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = (x as f32 - 170.0, y as f32 - 50.0);
            let sun = (dx * dx + dy * dy).sqrt() < 24.0;
            let hill = (y as f32) > 105.0 + 14.0 * ((x as f32) / 38.0).sin();
            let c = if sun {
                [255, 204, 77]
            } else if hill {
                [52, 132 - (y.saturating_sub(100).min(60) as u8), 82]
            } else {
                [64 + (y / 2) as u8, 120 + (y / 3) as u8, 235]
            };
            px.extend([c[0], c[1], c[2], 255]);
        }
    }
    Image::rgba(w, h, px)
}

pub fn card() -> El {
    let picture = landscape();
    section("Images and fit")
        .child(
            row()
                .gap_4()
                .flex_wrap()
                .items_end()
                .child(el().w(200).img(&picture).rounded_lg().shadow_md())
                .child(el().size_24().img(&picture).object_fit(ObjectFit::Cover).rounded_full().shadow_md())
                .child(el().w(120).h(96).img(&picture).bg(Token::Surface).rounded_md().object_fit(ObjectFit::Contain)),
        )
        .child(text("200 wide (height from the image) · cover in a circle · contain").text_xs().text_color(Token::TextMuted))
}

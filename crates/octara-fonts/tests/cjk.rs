//! With the CJK subset installed, hanzi, kana and CJK punctuation render from it (no missing
//! glyphs), in the right weight, next to Latin text in the theme's family.

use octara::paint::{GlyphRun, Item};
use octara::prelude::*;

fn runs(app: &App, id: &str) -> Vec<GlyphRun> {
    let n = app.kernel().find(id).unwrap();
    app.display_list().items.into_iter().filter_map(|i| if let Item::Glyphs(g) = i { (g.source == n).then_some(g) } else { None }).collect()
}

fn is(run: &GlyphRun, font: &[u8]) -> bool {
    run.font.data.as_ref().len() == font.len()
}

fn view() -> View {
    col()
        .child(text("Hello 你好，世界。ひらがな カタカナ").id("mixed"))
        .child(text("加粗的标题").id("bold").font_semibold())
        .child(text_input().id("field").w(px(240.0)).value("输入框").on_input(|_| {}))
        .into()
}

#[test]
fn cjk_falls_back_to_the_subset() {
    let mut app = App::new(view);
    // Without it: missing glyphs (glyph 0) for every CJK character.
    let missing = |app: &App, id: &str| runs(app, id).iter().flat_map(|r| r.glyphs.iter()).filter(|g| g.id == 0).count();
    assert!(missing(&app, "mixed") > 0);
    octara_fonts::cjk::install(&mut app);
    for id in ["mixed", "bold", "field"] {
        assert_eq!(missing(&app, id), 0, "{id} has missing glyphs");
    }
    let mixed = runs(&app, "mixed");
    assert!(mixed.iter().any(|r| is(r, octara_fonts::cjk::REGULAR)), "hanzi come from Noto Sans SC");
    assert!(mixed.iter().any(|r| !is(r, octara_fonts::cjk::REGULAR)), "Latin stays in Inter");
    assert!(runs(&app, "bold").iter().all(|r| is(r, octara_fonts::cjk::SEMIBOLD)), "600 uses the SemiBold face");
    assert!(runs(&app, "field").iter().all(|r| is(r, octara_fonts::cjk::REGULAR)));
}

#[test]
fn screenshots_are_deterministic_with_it() {
    let shot = || {
        let mut app = App::new(view);
        octara_fonts::cjk::install(&mut app);
        octara_render::screenshot(&app, 2.0)
    };
    let (a, b) = (shot(), shot());
    assert!(a == b, "the same pixels every time");
    if let Ok(dir) = std::env::var("OCTARA_SHOT_DIR") {
        a.save_png(format!("{dir}/cjk.png")).unwrap();
    }
}

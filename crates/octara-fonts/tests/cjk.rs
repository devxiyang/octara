//! With the CJK subset installed, hanzi, kana and CJK punctuation render from it (no missing
//! glyphs), in the right weight, next to Latin text in the theme's family.

#![cfg(feature = "cjk")]

use octara::app::AppConfig;
use octara::prelude::*;
use octara_primitives::paint::{GlyphRun, Item};

fn runs(app: &Window, id: &str) -> Vec<GlyphRun> {
    let n = app.kernel().find(id).unwrap();
    app.display_list()
        .items()
        .iter()
        .filter_map(|i| if let Item::Glyphs(g) = i { (g.source == n).then(|| g.clone()) } else { None })
        .collect()
}

fn is(run: &GlyphRun, font: &[u8]) -> bool {
    run.font.data.as_ref() == font
}

#[ui]
fn view() -> View {
    col()
        .child(text("Hello 你好，世界。ひらがな カタカナ").id("mixed"))
        .child(text("加粗的标题").id("bold").font_semibold())
        .child(text_input().id("field").w(px(240.0)).value("输入框").on_input(|_| {}))
        .into()
}

fn configured_window() -> Window {
    let mut config = AppConfig::default();
    for data in [octara_fonts::cjk::REGULAR, octara_fonts::cjk::SEMIBOLD] {
        config.text_engine.register_fallback_font(data, &octara_fonts::cjk::SCRIPTS);
    }
    App::with_config(octara::platform::HeadlessPlatform::default(), config).open_window(WindowOptions::new("CJK"), view).unwrap()
}

#[test]
fn cjk_falls_back_to_the_subset_on_the_first_frame() {
    let app = Window::headless(view);
    // Without it: missing glyphs (glyph 0) for every CJK character.
    let missing = |app: &Window, id: &str| runs(app, id).iter().flat_map(|r| r.glyphs.iter()).filter(|g| g.id == 0).count();
    assert!(missing(&app, "mixed") > 0);
    app.app().shutdown();
    let app = configured_window();
    for id in ["mixed", "bold", "field"] {
        assert_eq!(missing(&app, id), 0, "{id} has missing glyphs");
    }
    let mixed = runs(&app, "mixed");
    assert!(mixed.iter().any(|r| is(r, octara_fonts::cjk::REGULAR)), "hanzi come from Noto Sans SC");
    assert!(mixed.iter().any(|r| !is(r, octara_fonts::cjk::REGULAR)), "Latin stays in Inter");
    assert!(runs(&app, "bold").iter().all(|r| is(r, octara_fonts::cjk::SEMIBOLD)), "600 uses the SemiBold face");
    assert!(runs(&app, "field").iter().all(|r| is(r, octara_fonts::cjk::REGULAR)));
    assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
    app.app().shutdown();
}

#[test]
fn screenshots_are_deterministic_with_it() {
    let shot = || {
        let app = configured_window();
        let picture = octara_render::CpuRenderer::new().render(&app.display_list(), 2.0);
        app.app().shutdown();
        picture
    };
    let (a, b) = (shot(), shot());
    assert!(a == b, "the same pixels every time");
    if let Ok(dir) = std::env::var("OCTARA_SHOT_DIR") {
        a.save_png(format!("{dir}/cjk.png")).unwrap();
    }
}

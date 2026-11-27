//! With the CJK subset installed, hanzi, kana and CJK punctuation render from it (no missing
//! glyphs), in the right weight, next to Latin text in the theme's family.

use octara::model::paint::{DisplayList, GlyphRun};
use octara::prelude::*;
use octara_pipeline::{NativeEngine, NativeEngineConfig};
use std::sync::Arc;

fn runs(app: &App, id: &str) -> Vec<GlyphRun> {
    let window = app.runtime().windows().next().unwrap().1;
    let node = window.tree().test_ids(id)[0];
    window.last_commit().unwrap().draw.as_ref().unwrap().glyph_runs().filter(|run| run.source == node).collect()
}

fn drawing(app: &App) -> &DisplayList {
    app.runtime().windows().next().unwrap().1.last_commit().unwrap().draw.as_deref().unwrap()
}

fn is(run: &GlyphRun, font: &[u8]) -> bool {
    run.font.data.as_ref() == font
}

#[ui]
fn view(_: &mut BuildCx<'_>) -> El {
    col()
        .child(text("Hello 你好，世界。ひらがな カタカナ").test_id("mixed"))
        .child(text("加粗的标题").test_id("bold").text_weight(600))
        .child(input().test_id("field").w(px(240.0)).value("输入框").on_input(|_, _| {}))
}

fn configured_window(bundled: bool) -> App {
    let mut app = App::new(move |_, _| {
        let mut engine = NativeEngine::new(NativeEngineConfig::default());
        if bundled {
            for data in [octara_fonts::cjk::REGULAR, octara_fonts::cjk::SEMIBOLD] {
                engine.register_font(Arc::from(data), &octara_fonts::cjk::SCRIPTS);
            }
        }
        Box::new(engine)
    });
    app.open_window(WindowOptions::default(), view).unwrap();
    app
}

#[test]
fn cjk_falls_back_to_the_subset_on_the_first_frame() {
    let app = configured_window(false);
    // Without it: missing glyphs (glyph 0) for every CJK character.
    let missing = |app: &App, id: &str| runs(app, id).iter().flat_map(|r| r.glyphs.iter()).filter(|g| g.id == 0).count();
    assert!(missing(&app, "mixed") > 0);
    let mut app = configured_window(true);
    for id in ["mixed", "bold", "field"] {
        assert_eq!(missing(&app, id), 0, "{id} has missing glyphs");
    }
    let mixed = runs(&app, "mixed");
    assert!(mixed.iter().any(|r| is(r, octara_fonts::cjk::REGULAR)), "hanzi come from Noto Sans SC");
    assert!(mixed.iter().any(|r| !is(r, octara_fonts::cjk::REGULAR)), "Latin stays in Inter");
    assert!(runs(&app, "bold").iter().all(|r| is(r, octara_fonts::cjk::SEMIBOLD)), "600 uses the SemiBold face");
    assert!(runs(&app, "field").iter().all(|r| is(r, octara_fonts::cjk::REGULAR)));
    assert!(app.runtime_mut().take_diagnostics().is_empty());
}

#[test]
fn screenshots_are_deterministic_with_it() {
    let shot = || {
        let app = configured_window(true);
        octara_render::CpuRenderer::new().render(drawing(&app), 2.0)
    };
    let (a, b) = (shot(), shot());
    assert!(a == b, "the same pixels every time");
}

//! The platform's fonts and fallback (feature `system-fonts`): CJK text gets real glyphs.
#![cfg(all(feature = "system-fonts", target_os = "macos"))]

use octara::kernel::text::ParleyEngine;
use octara::prelude::*;

#[test]
fn cjk_falls_back_to_a_system_font_and_system_ui_resolves() {
    let mut app =
        Window::headless(|| col().child(text("Hello 世界").id("mixed")).child(text("System").id("ui").font_family("system-ui")).into());
    app.kernel_mut().set_text_engine(Box::new(ParleyEngine::system()));
    app.resize(800.0, 600.0);
    let dl = app.display_list();
    let k = app.kernel();
    let mixed: Vec<_> = dl.glyph_runs().filter(|g| g.source == k.find("mixed").unwrap()).collect();
    assert!(mixed.len() >= 2, "Latin from Inter, CJK from a fallback font: {} runs", mixed.len());
    assert!(mixed.iter().all(|r| r.glyphs.iter().all(|g| g.id != 0)), "no missing glyphs (.notdef)");
    let ui = dl.glyph_runs().find(|g| g.source == k.find("ui").unwrap()).unwrap();
    assert!(ui.glyphs.iter().all(|g| g.id != 0));
}

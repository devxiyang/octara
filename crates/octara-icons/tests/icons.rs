//! Verifies the bundled shapes and their rendering through the v2 native pipeline.

use octara::prelude::*;
use octara_icons as icons;
use octara_model::{paint::PathStyle, theme::rgb};
use octara_pipeline::{NativeEngine, NativeEngineConfig};

#[test]
fn the_set_is_found_by_name_and_tag() {
    assert!(icons::ALL.len() > 2000);
    assert!(icons::ALL.windows(2).all(|w| w[0].name < w[1].name));
    assert_eq!(icons::by_name("search").unwrap().name(), "search");
    assert!(icons::by_name("no-such-icon").is_none());
    assert!(icons::search("magnifier").iter().any(|e| e.name == "search"));
    assert_eq!(icons::search("house")[0].name, "house");
    for e in icons::ALL {
        let icon = e.icon.to_icon();
        assert_eq!(icon.name(), e.name);
        assert_eq!(icon.view_box(), Rect::new(0.0, 0.0, 24.0, 24.0));
        assert!(!icon.shapes().is_empty(), "{}", e.name);
        assert!(icon.shapes().iter().all(|shape| shape.path.bounds().is_some()), "{}", e.name);
    }
}

#[test]
fn icons_draw_in_the_text_color() {
    let mut app = App::new(|_, _| Box::new(NativeEngine::new(NativeEngineConfig::default())));
    let window = app
        .open_window(WindowOptions::default(), |_| {
            row()
                .p(sp(4.0))
                .gap(sp(4.0))
                .child(icon(icons::SEARCH.into()).test_id("glass"))
                .child(icon(icons::HOUSE.into()).test_id("home").label("Home"))
                .child(icon(icons::X.into()).test_id("x").size(sp(8.0)).text_color(rgb(0xff0000)))
                .child(row().text_color(rgb(0x0000ff)).child(icon(icons::CHECK.with_stroke_width(3.0)).test_id("check").size(sp(6.0))))
        })
        .unwrap();
    let window = app.runtime().window(window).unwrap();
    let tree = window.tree();
    let x = tree.test_ids("x")[0];
    let check = tree.test_ids("check")[0];
    let home = tree.test_ids("home")[0];
    let glass = tree.test_ids("glass")[0];
    assert_eq!(window.geometry(glass).unwrap().rect.w, 16.0);
    assert_eq!(tree.semantics(home).unwrap().label.as_deref(), Some("Home"));
    let display = window.last_commit().unwrap().draw.as_deref().unwrap();
    let red: Vec<_> = display.paths().filter(|p| p.source == x).collect();
    assert_eq!(red.len(), 2);
    assert!(red.iter().all(|p| p.color == rgb(0xff0000) && (p.transform.xx - 32.0 / 24.0).abs() < 1e-6));
    let blue: Vec<_> = display.paths().filter(|p| p.source == check).collect();
    assert!(blue.iter().all(|p| p.color == rgb(0x0000ff)));
    assert!(matches!(&blue[0].style, PathStyle::Stroke(s) if s.width == 3.0));

    let shot = octara_render::CpuRenderer::new().paint(display, 2.0).unwrap();
    let r = window.geometry(x).unwrap().rect;
    let c = shot.pixel(((r.x + r.w / 2.0) * 2.0) as u32, ((r.y + r.h / 2.0) * 2.0) as u32).unwrap();
    assert!(c[0] > 200 && c[1] < 60 && c[2] < 60, "{c:?}");
}

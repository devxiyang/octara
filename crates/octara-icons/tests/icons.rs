//! Icons as elements: sized, tinted by the text color, decorative unless labeled; the set found
//! by name and by tag.

use octara::prelude::*;
use octara_icons as icons;
use octara_render::screenshot;

#[test]
fn the_set_is_found_by_name_and_tag() {
    assert!(icons::ALL.len() > 2000);
    assert!(icons::ALL.windows(2).all(|w| w[0].name < w[1].name), "sorted, so by_name can search");
    assert_eq!(icons::by_name("search").unwrap().name(), "search");
    assert!(icons::by_name("no-such-icon").is_none());
    let found = icons::search("magnifier");
    assert!(found.iter().any(|e| e.name == "search"), "found by tag");
    assert_eq!(icons::search("house")[0].name, "house", "names first, the shortest first");
    // Every icon makes shapes (the path data parses).
    for e in icons::ALL {
        assert!(!e.icon.shapes(None).is_empty(), "{}", e.name);
    }
}

fn view() -> View {
    row()
        .p_4()
        .gap_4()
        .child(el().id("b").role(Role::Button).focusable().on_press(|| {}).p_2().child(icon(icons::SEARCH).id("glass")))
        .child(icon(icons::HOUSE).id("home").label("Home"))
        .child(icon(icons::X).id("x").size_8().text_color(rgb(0xff0000)))
        .child(row().text_color(rgb(0x0000ff)).child(icon(icons::CHECK).id("check").size_6().stroke_width(3.0)))
        .into()
}

#[test]
fn icons_draw_in_the_text_color_and_hide_unless_labeled() {
    let app = App::new(view);
    let k = app.kernel();
    let (x, check) = (k.find("x").unwrap(), k.find("check").unwrap());
    assert_eq!(k.rect(k.find("glass").unwrap()).2, 16.0, "16 px by default");
    assert_eq!(k.rect(x).2, 32.0);
    drop(k);

    // The X: two strokes from the icon's node, red, scaled from the 24 grid to 32 px.
    let dl = app.display_list();
    let red: Vec<_> = dl.paths().filter(|p| p.source == x).collect();
    assert_eq!(red.len(), 2);
    assert!(red.iter().all(|p| p.color == rgb(0xff0000) && (p.transform.xx - 32.0 / 24.0).abs() < 1e-6));
    // The check inherits its row's color and takes its stroke width.
    let blue: Vec<_> = dl.paths().filter(|p| p.source == check).collect();
    assert!(blue.iter().all(|p| p.color == rgb(0x0000ff)));
    assert!(matches!(&blue[0].style, octara::path::PathStyle::Stroke(s) if s.width == 3.0));

    // Pixels: the X's diagonal crosses its center in red.
    let shot = screenshot(&app, 2.0);
    let r = app.kernel().rect(x);
    let c = shot.pixel(((r.0 + r.2 / 2.0) * 2.0) as u32, ((r.1 + r.3 / 2.0) * 2.0) as u32);
    assert!(c[0] > 200 && c[1] < 60 && c[2] < 60, "{c:?}");

    // Semantics: a labeled icon is an image named by its label; the others are hidden, so the
    // icon-only button has no name.
    let snap = app.inspect();
    let home = snap.find("home").unwrap();
    assert_eq!((home.role.as_str(), home.name.as_deref(), home.icon.as_deref()), ("image", Some("Home"), Some("house")));
    assert!(snap.find("glass").is_none() && snap.find("x").is_none());
    let issues = app.kernel().accessibility_issues();
    assert!(issues.iter().any(|i| i.code.id == "UNNAMED_CONTROL" && i.test_id.as_deref() == Some("b")), "{issues:?}");
}

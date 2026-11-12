//! Skipping subtrees that cannot show changes nothing: the display list and hit testing are the
//! same with and without it, for content that reaches past its box in every way.

use octara::paint::Item;
use octara::prelude::*;

fn tricky() -> View {
    col()
        .size_full()
        .child(
            // A long scrolled list: most rows are off screen.
            col()
                .id("list")
                .h(300)
                .overflow_y_scroll()
                .children((0..400).map(|i| row().id(format!("row-{i}")).h(24).px_2().on_press(|| {}).child(text(format!("Row {i}"))))),
        )
        .child(
            // An off-screen box whose child is placed back into view.
            el().w(100).h(40).ml(-300.0).child(el().id("back").absolute().left(320.0).top_0().size_8().bg(Token::Accent).on_press(|| {})),
        )
        .child(
            // An off-screen box whose shadow reaches into view.
            el().w(100)
                .h(40)
                .ml(-140.0)
                .bg(Token::Surface)
                .shadow([BoxShadow::new(0.0, 0.0, rgb(0x000000)).blur_radius(40.0).spread_radius(20.0)]),
        )
        .child(
            // A narrow off-screen text whose one line runs past its box into view.
            text("A line that does not wrap and runs on").whitespace_nowrap().w(10).ml(-60.0).id("long"),
        )
        .into()
}

fn describe(dl: &octara::paint::DisplayList) -> Vec<String> {
    dl.items
        .iter()
        .map(|i| match i {
            Item::Glyphs(g) => format!("glyphs {:?} {} {:?}", g.source, g.glyphs.len(), g.glyphs.first().map(|g| (g.x, g.y))),
            other => format!("{other:?}"),
        })
        .collect()
}

#[test]
fn skipping_off_screen_subtrees_changes_nothing() {
    let mut app = App::new(tricky);
    app.resize(600.0, 500.0);
    let list = app.kernel().find("list").unwrap();
    app.kernel_mut().scroll_to(list, 0.0, 4000.0);
    for culling in [true, false] {
        app.kernel_mut().set_culling(culling);
        let _ = app.display_list();
    }
    let culled = {
        app.kernel_mut().set_culling(true);
        app.display_list()
    };
    let full = {
        app.kernel_mut().set_culling(false);
        app.display_list()
    };
    assert_eq!(describe(&culled), describe(&full));
    assert!(culled.items.iter().any(|i| matches!(i, Item::Shadow(_))), "the shadow reaching in is drawn");
    let long = app.kernel().find("long").unwrap();
    assert!(culled.glyph_runs().any(|g| g.source == long), "the overflowing text is drawn");

    // Hit testing finds the same nodes everywhere.
    for y in (0..500).step_by(7) {
        for x in (0..600).step_by(11) {
            let (x, y) = (x as f32, y as f32);
            app.kernel_mut().set_culling(true);
            let a = app.kernel().hit_test(x, y);
            app.kernel_mut().set_culling(false);
            let b = app.kernel().hit_test(x, y);
            assert_eq!(a, b, "at ({x}, {y})");
        }
    }
    app.kernel_mut().set_culling(true);
    let back = app.kernel().find("back").unwrap();
    let (bx, by, ..) = app.kernel().rect(back);
    assert_eq!(app.kernel().hit_test(bx + 4.0, by + 4.0), Some(back), "a child placed back into view is hit");
}

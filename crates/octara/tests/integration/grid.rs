//! Grid layout with GPUI's methods: grid(), grid_cols(n), col_span(n), col_span_full().

use octara::prelude::*;

fn rect(app: &Window, id: &str) -> (f32, f32, f32, f32) {
    let k = app.kernel();
    k.rect(k.find(id).unwrap())
}

#[test]
fn equal_columns_spans_and_full_rows() {
    let app = Window::headless(|| {
        el().id("grid")
            .grid()
            .grid_cols(3)
            .gap_4()
            .w(316)
            .child(text("a").id("a"))
            .child(text("b").id("b"))
            .child(text("c").id("c"))
            .child(text("wide").id("wide").col_span(2))
            .child(text("d").id("d"))
            .child(text("footer").id("footer").col_span_full())
            .into()
    });
    // 316 px, two 16 px gaps: three 94.67 px columns.
    let (ax, ay, aw, _) = rect(&app, "a");
    let (bx, by, bw, _) = rect(&app, "b");
    let (cx, ..) = rect(&app, "c");
    assert_eq!(ay, by);
    assert!((aw - bw).abs() <= 1.0 && (aw - 94.67).abs() <= 1.0, "equal columns: {aw} {bw}");
    assert!((bx - ax - (aw + 16.0)).abs() <= 1.0 && cx > bx);
    // Row 2: `wide` spans two columns, `d` takes the third.
    let (wx, wy, ww, _) = rect(&app, "wide");
    let (dx, dy, ..) = rect(&app, "d");
    assert_eq!((wx, wy == dy), (ax, true));
    assert!((ww - (2.0 * aw + 16.0)).abs() <= 1.0, "two columns and a gap: {ww}");
    assert!((dx - cx).abs() <= 1.0);
    // Row 3: the footer spans the whole grid.
    let (fx, fy, fw, _) = rect(&app, "footer");
    assert_eq!(fx, ax);
    assert!(fy > dy && (fw - 316.0).abs() <= 1.0);
    assert!(app.layout_issues().is_empty(), "{:#?}", app.layout_issues());
}

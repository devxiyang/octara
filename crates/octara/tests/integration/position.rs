//! Positioning: an absolute child is placed against its parent (Rune, GPUI, Yoga), wherever the
//! parent is.

use octara::prelude::*;

#[test]
fn absolute_children_are_placed_against_their_parent() {
    let app = App::new(|| {
        col()
            .p_8()
            .child(el().h(40))
            .child(
                el().id("box")
                    .w(120)
                    .h(80)
                    .child(el().id("tl").absolute().top_0().left_0().size_4())
                    .child(text("Group").id("br").absolute().bottom_0().right_0())
                    .child(el().id("inset").absolute().inset_0()),
            )
            .child(el().id("nudged").relative().top(5.0).left(3.0).size_4())
            .into()
    });
    let k = app.kernel();
    let r = |id: &str| k.rect(k.find(id).unwrap());
    let (bx, by, bw, bh) = r("box");
    assert_eq!((bx, by), (32.0, 72.0));
    assert_eq!((r("tl").0, r("tl").1), (bx, by));
    let br = r("br");
    assert_eq!((br.0 + br.2, br.1 + br.3), (bx + bw, by + bh));
    assert_eq!(r("inset"), (bx, by, bw, bh));
    // `relative` offsets from where the flow put it.
    assert_eq!((r("nudged").0, r("nudged").1), (32.0 + 3.0, 72.0 + 80.0 + 5.0));
}

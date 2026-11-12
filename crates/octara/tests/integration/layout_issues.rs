//! Layout diagnostics: overflow, collapse, truncation, clamped overlays, missing tokens.

use octara::diagnostics::Severity;
use octara::kernel::LayoutIssue;
use octara::prelude::*;

fn issues(view: fn() -> View) -> Vec<LayoutIssue> {
    Window::headless(view).layout_issues()
}

fn codes(v: &[LayoutIssue]) -> Vec<(&'static str, Option<String>)> {
    v.iter().map(|i| (i.code.id, i.test_id.clone())).collect()
}

#[test]
fn a_clean_layout_has_no_issues() {
    let v = issues(|| col().child(text("hello").id("t")).child(row().child(text("a")).child(text("b")).gap_2()).p_4().into());
    assert!(v.is_empty(), "{v:#?}");
}

#[test]
fn overflow_names_the_element_where_it_was_made_and_the_fix() {
    let v = issues(|| el().child(text("a long line that does not wrap").id("t").whitespace_nowrap()).id("box").w(60).into());
    assert_eq!(codes(&v), [("OVERFLOW", Some("box".into()))]);
    let i = &v[0];
    assert!(i.message.contains("content of #box overflows its right edge") && i.message.contains("(#t)"), "{}", i.message);
    assert!(i.hint.contains("truncate"), "{}", i.hint);
    assert!(i.at.as_deref().unwrap().contains("layout_issues.rs"), "{:?}", i.at);
    assert_eq!(i.path, "#box");

    // The fixes clear it: wrapping (the default) or truncating.
    assert!(issues(|| el().child(text("a long line that does not wrap").id("t")).w(60).into()).is_empty());
    let v = issues(|| el().child(text("a long line that does not wrap").id("t").truncate()).w(60).into());
    assert_eq!(codes(&v), [("TEXT_TRUNCATED", Some("t".into()))]);
    assert_eq!(v[0].code.severity, Severity::Info);
}

#[test]
fn content_taller_than_the_window_overflows_it() {
    // A full-size column of 40 rows of 20 px: 30 fit in 600 px.
    let v = issues(|| col().children((0..40).map(|i| text(format!("row {i}")).id(format!("r{i}")))).id("list").size_full().into());
    assert_eq!(codes(&v), [("OVERFLOW", Some("list".into()))]);
    assert!(v[0].message.contains("overflows its bottom by 200 px") && v[0].message.contains("and 7 more"), "{}", v[0].message);
}

#[test]
fn collapsed_elements_with_content_are_reported() {
    // `flex_1` + `min_h_0` in a column sized by its content: no free space, so it shrinks to 0.
    let v = issues(|| col().child(col().child(text("lost").id("t")).id("body").flex_1().min_h_0()).h(0).into());
    assert!(codes(&v).contains(&("COLLAPSED", Some("body".into()))), "{v:#?}");
    // An element sized to zero on purpose is fine.
    assert!(issues(|| el().w(0).into()).is_empty());
}

#[test]
fn clamped_overlays_and_missing_tokens() {
    // A 500 px menu 280 px down a 600 px window fits neither above nor below.
    fn menu(placement: Placement) -> View {
        el().child(text("open").id("t")).pt(280).overlay(|| true, placement, || el().id("menu").h(500).w(100).into()).into()
    }
    // Without `fit` it is pushed into the window, over its anchor: reported.
    let v = issues(|| menu(Placement::BELOW.fit(false)));
    assert!(codes(&v).iter().any(|c| c.0 == "OVERLAY_CLAMPED"), "{v:#?}");
    // With `fit` (the default for BELOW) it takes the roomier side, is limited to the room there
    // and scrolls: nothing to report.
    let app = Window::headless(|| menu(Placement::BELOW));
    assert!(app.layout_issues().is_empty(), "{:#?}", app.layout_issues());
    let k = app.kernel();
    let wrapper = k.parent(k.find("menu").unwrap()).unwrap();
    let (_, y, _, h) = k.rect(wrapper);
    let (_, ty, _, th) = k.rect(k.find("t").unwrap());
    assert_eq!(y, ty + th + 4.0, "below, the roomier side");
    assert_eq!(y + h, 600.0 - 8.0, "limited to the room, 8 px from the edge");
    assert_eq!(k.scroll_size(wrapper).1, 500.0, "the rest scrolls");
    let v = issues(|| text("x").id("t").text_color(Token::custom("brand")).into());
    assert_eq!(codes(&v), [("MISSING_TOKEN", Some("t".into()))]);
    assert!(v[0].message.contains("`brand`"));
}

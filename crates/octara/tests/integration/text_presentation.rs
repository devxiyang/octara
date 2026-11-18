//! Document drawing, interaction and positional accessibility use the same visible text.
use octara::accesskit::{Action, Node, NodeId as AkId, Role as AkRole};
use octara::inspect::{self, Delivery};
use octara::kernel::a11y::a11y_id;
use octara::platform::{HeadlessPlatform, WindowOptions};
use octara::prelude::*;
use serde_json::json;
use std::collections::HashMap;

fn tree(w: &Window) -> HashMap<AkId, Node> {
    w.accessibility_tree().nodes.into_iter().collect()
}

fn visible_text(w: &Window, id: &str) -> String {
    let tree = tree(w);
    let node = &tree[&a11y_id(w.kernel().find(id).unwrap())];
    node.children()
        .iter()
        .filter_map(|id| {
            let node = &tree[id];
            (node.role() == AkRole::TextRun).then(|| node.value().unwrap())
        })
        .collect()
}

fn highlights(w: &Window, id: &str) -> Vec<octara_primitives::paint::Rect> {
    let source = w.kernel().find(id).unwrap();
    let selection = Theme::light().get(Token::Accent).multiply_alpha(0.3);
    w.display_list().quads().filter(|q| q.source == source && q.fill == Some(selection)).map(|q| q.rect).collect()
}

#[test]
fn clamped_hidden_line_has_no_link_or_selection_geometry_and_copy_keeps_logical_text() {
    let app = App::new(HeadlessPlatform::default());
    let opened = app.run(|| state("opened", || 0u32));
    let mut w = app
        .open_window(WindowOptions::default(), move || {
            col()
                .id("doc")
                .size_full()
                .selectable()
                .child(
                    text(rich().text("visible\n").styled("hidden中文😀", |s| s.link("/hidden").id("hidden")))
                        .id("text")
                        .w(200)
                        .line_clamp(1),
                )
                .child(el().id("next").size(40).focusable())
                .on_link(move |_| opened.update(|n| *n += 1))
        })
        .unwrap();
    let hidden = w.kernel().find("hidden").unwrap();
    assert!(w.kernel().link_rects(hidden).is_empty(), "the historical hidden second-line regression");
    assert!(w.inspect().find("hidden").is_none());
    assert!(w.dispatch(Event::Press("hidden".into())).is_err());
    assert!(w.dispatch(Event::Focus("hidden".into())).is_err());
    assert!(!w.event(InputEvent::Accessibility { target: a11y_id(hidden), action: Action::Click, data: None }));
    w.press_key("tab").unwrap();
    assert!(w.kernel().state_of("next").unwrap().contains(State::FOCUSED));
    let text = w.kernel().find("text").unwrap();
    let (x, y, _, _) = w.kernel().shown_rect(text);
    w.click_at(x + 4.0, y + 30.0);
    assert_eq!(app.run(|| opened.get()), 0);
    assert_eq!(visible_text(&w, "text"), "visible");
    let ax = tree(&w);
    assert!(!ax.contains_key(&a11y_id(hidden)));
    assert_eq!(ax[&a11y_id(text)].value(), Some("visible\nhidden中文😀"));

    w.select(("text", "visible\n".len()), ("text", "visible\nhidden中文😀".len())).unwrap();
    assert_eq!(w.selected_text().as_deref(), Some("hidden中文😀"));
    assert!(highlights(&w, "text").is_empty(), "logical selection does not invent hidden rectangles");
    w.dispatch(Event::Blur).unwrap();
    w.select_all("doc").unwrap();
    assert_eq!(w.selected_text().as_deref(), Some("visible\nhidden中文😀"));
    assert!(highlights(&w, "text").iter().all(|r| r.y == y));
    w.press_key("mod+c").unwrap();
    assert_eq!(w.clipboard_text().as_deref(), Some("visible\nhidden中文😀"));
    assert!(app.diagnostics().is_empty());
}

#[test]
fn partial_link_excludes_synthetic_ellipsis_from_hits_style_and_selection() {
    let app = App::new(HeadlessPlatform::default());
    let opened = app.run(|| state("opened", || 0u32));
    let mut w = app
        .open_window(WindowOptions::default(), move || {
            col()
                .id("doc")
                .size_full()
                .selectable()
                .child(
                    text(rich().text("Read ").styled("中文😀 long guide and hidden suffix", |s| s.link("/guide").id("guide")))
                        .id("text")
                        .w(110)
                        .truncate(),
                )
                .on_link(move |_| opened.update(|n| *n += 1))
        })
        .unwrap();
    let (text, link) = {
        let k = w.kernel();
        (k.find("text").unwrap(), k.find("guide").unwrap())
    };
    let rects = w.kernel().link_rects(link);
    assert_eq!(rects.len(), 1);
    let r = rects[0];
    assert!(r.w > 0.0 && r.right() < 110.0);
    let (x, y, _, h) = w.kernel().shown_rect(text);
    w.click_at(x + r.right() + 2.0, y + h / 2.0);
    assert_eq!(app.run(|| opened.get()), 0, "ellipsis cannot activate the final link");
    let dl = w.display_list();
    let dots = dl.glyph_runs().filter(|g| g.source == text).last().unwrap();
    assert_eq!(dots.color, Theme::light().get(Token::Text), "ellipsis does not inherit the link brush");
    let visible = visible_text(&w, "text");
    assert!(visible.starts_with("Read ") && visible.len() < w.kernel().text(text).len());
    w.pointer_down(x + 0.1, y + h / 2.0);
    w.pointer_move(x + 109.0, y + h / 2.0);
    w.pointer_up(x + 109.0, y + h / 2.0);
    assert_eq!(w.selected_text().as_deref(), Some(visible.as_str()), "ellipsis selects only to the retained source boundary");
    assert!(highlights(&w, "text").iter().all(|q| q.right() <= x + r.right() + 0.01));
    w.press_key("tab").unwrap();
    assert!(w.kernel().state_of("guide").unwrap().contains(State::FOCUSED));
    w.press_key("enter").unwrap();
    assert_eq!(app.run(|| opened.get()), 1, "a partly visible link remains actionable");
    assert!(app.diagnostics().is_empty());
}

#[test]
fn resize_hides_and_reveals_same_target_and_updates_fixed_height_accessibility_children() {
    let app = App::new(HeadlessPlatform::default());
    let opened = app.run(|| state("opened", || 0u32));
    let mut w = app
        .open_window(WindowOptions::default(), move || {
            col()
                .id("doc")
                .size_full()
                .selectable()
                .child(
                    text(rich().text("Several prefix words ").styled("中文😀 guide", |s| s.link("/guide").id("guide")))
                        .id("text")
                        .w_full()
                        .h(20)
                        .line_clamp(1),
                )
                .on_link(move |_| opened.update(|n| *n += 1))
        })
        .unwrap();
    let link = w.kernel().find("guide").unwrap();
    let target = w.inspect().find("guide").unwrap().target.clone();
    w.accessibility_update();
    w.press_key("tab").unwrap();
    assert!(w.kernel().state(link).contains(State::FOCUSED));
    let shaped = w.kernel().text_engine_shaped();
    for _ in 0..2 {
        w.resize(55.0, 300.0);
        let update = w.accessibility_update();
        let text = w.kernel().find("text").unwrap();
        assert!(update.nodes.iter().any(|(id, n)| *id == a11y_id(text) && !n.children().contains(&a11y_id(link))));
        assert!(w.inspect().find("guide").is_none());
        assert_eq!(w.kernel().find("guide"), Some(link));
        assert!(w.kernel().link_rects(link).is_empty());
        assert!(!w.kernel().state(link).contains(State::FOCUSED), "resize retires hidden focus");
        assert!(w.dispatch(Event::Focus("guide".into())).is_err());
        assert!(!w.event(InputEvent::Accessibility { target: a11y_id(link), action: Action::Click, data: None }));
        w.press_key("enter").unwrap();
        let reply = inspect::handle_line(&mut w, &json!({"op":"tap","id":"guide","target":target}).to_string());
        assert_eq!(reply.code.as_deref(), Some("STEP_FAILED"), "identity is current but cannot activate: {reply:?}");
        assert_eq!(reply.delivery, Some(Delivery::Refused));

        w.resize(800.0, 600.0);
        let update = w.accessibility_update();
        assert!(update.nodes.iter().any(|(id, _)| *id == a11y_id(link)), "revealed bounds must be sent again");
        assert_eq!(w.inspect().find("guide").unwrap().target, target);
        let reply = inspect::handle_line(&mut w, &json!({"op":"tap","id":"guide","target":target}).to_string());
        assert!(reply.ok, "the same observed target works after reveal: {reply:?}");
    }
    assert_eq!(app.run(|| opened.get()), 2);
    assert_eq!(w.kernel().text_engine_shaped(), shaped, "resize reuses shaped paragraphs and cached ellipsis");
    assert!(app.diagnostics().is_empty());
}

#[test]
fn truncate_is_one_line_even_with_hard_newlines_and_rtl_geometry_remains_actionable() {
    let app = App::new(HeadlessPlatform::default());
    let opened = app.run(|| state("opened", || 0u32));
    let mut w = app
        .open_window(WindowOptions::default(), move || {
            col()
                .id("doc")
                .size_full()
                .selectable()
                .child(text(rich().styled("אבג", |s| s.link("/rtl").id("rtl")).text("\nhidden")).id("text").w(160).truncate())
                .on_link(move |_| opened.update(|n| *n += 1))
        })
        .unwrap();
    let link = w.kernel().find("rtl").unwrap();
    let rects = w.kernel().link_rects(link);
    assert_eq!(rects.len(), 1);
    assert!(rects[0].w > 0.0);
    assert_eq!(visible_text(&w, "text"), "אבג");
    let source = w.kernel().find("text").unwrap();
    let dl = w.display_list();
    let mut baselines: Vec<_> =
        dl.glyph_runs().filter(|r| r.source == source).flat_map(|r| r.glyphs.iter().map(|g| g.y).collect::<Vec<_>>()).collect();
    baselines.dedup();
    assert_eq!(baselines.len(), 1);
    w.tap("rtl").unwrap();
    assert_eq!(app.run(|| opened.get()), 1);
    w.select(("text", 0), ("text", "אבג".len())).unwrap();
    assert!(!highlights(&w, "text").is_empty());
    let ax = tree(&w);
    let runs: Vec<_> =
        ax[&a11y_id(source)].children().iter().filter_map(|id| (ax[id].role() == AkRole::TextRun).then_some(&ax[id])).collect();
    assert_eq!(runs[0].text_direction(), Some(octara::accesskit::TextDirection::RightToLeft));
    assert!(runs[0].character_widths().unwrap().iter().all(|w| *w > 0.0));
    assert!(app.diagnostics().is_empty());
}

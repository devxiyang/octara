//! Native views headless: the host hears of creation, props, placements (with clips and
//! scrolling) and removal; events and sizes come back from it; inspect shows the view; check
//! finds content an overlay view would hide, and views without a label.

use octara::native::{HostCall, NativeEvent, NativeId, RecordingHost, Slot};
use octara::prelude::*;

fn calls(app: &App) -> Vec<HostCall> {
    app.native_host().as_any().downcast_ref::<RecordingHost>().unwrap().calls.clone()
}

fn view() -> View {
    let shown = state("shown", || true);
    let url = state("url", || "https://example.com".to_string());
    let loaded = state("loaded", String::new);
    col()
        .size_full()
        .child(el().id("toggle").role(Role::Button).label("Toggle").size_8().on_press(move || shown.update(|s| *s = !*s)))
        .child(el().id("next").role(Role::Button).label("Next").size_8().on_press(move || url.set("https://example.org".into())))
        .child(text(move || loaded.get()).id("loaded"))
        .child(
            col().id("page").h(px(200.0)).overflow_y_scroll().child(show(
                move || shown.get(),
                move || {
                    web_view(move || url.get())
                        .id("web")
                        .label("Help page")
                        .w(px(300.0))
                        .on_native(move |e: &NativeEvent| {
                            if e.name == "load" {
                                loaded.set(e.data["url"].as_str().unwrap_or_default().to_owned())
                            }
                        })
                        .into()
                },
            )).child(el().h(px(400.0))),
        )
        .into()
}

#[test]
fn the_host_follows_the_view() {
    let mut app = App::new(view);
    let c = calls(&app);
    let HostCall::Create { id, kind, props } = &c[0] else { panic!("{c:?}") };
    assert_eq!((kind.as_str(), props["url"].as_str()), ("web", Some("https://example.com")));
    let id = *id;
    let HostCall::Place { placements } = &c[1] else { panic!("{c:?}") };
    let p = &placements[0];
    assert_eq!((p.id, p.slot, p.rect.w, p.rect.h, p.visible), (id, Slot::Overlay, 300.0, 150.0, true), "150 high until it reports a size");
    assert_eq!(p.clip.h, 200.0, "clipped by the scroll container");

    // Props are handed over whole when they change; a placement only when it changes.
    let before = calls(&app).len();
    app.tap("next").unwrap();
    let c = calls(&app);
    assert_eq!(c[before..], [HostCall::Update { id, props: serde_json::json!({ "url": "https://example.org" }) }]);

    // Scrolling moves it; scrolled out of its clip, it is not visible.
    app.dispatch(Event::ScrollTo("page".into(), 0.0, 100.0)).unwrap();
    let Some(HostCall::Place { placements }) = calls(&app).last().cloned() else { panic!() };
    assert_eq!(placements[0].rect.y + 100.0, p.rect.y);
    app.dispatch(Event::ScrollTo("page".into(), 0.0, 400.0)).unwrap();
    let Some(HostCall::Place { placements }) = calls(&app).last().cloned() else { panic!() };
    assert!(!placements[0].visible);

    // Events and sizes come back.
    assert!(app.native_event(id, NativeEvent { name: "load".into(), data: serde_json::json!({ "url": "https://example.org" }) }));
    assert_eq!(app.kernel().text_of("loaded").unwrap(), "https://example.org");
    app.native_size(id, 300.0, 90.0);
    let web = app.kernel().find("web").unwrap();
    assert_eq!(app.kernel().rect(web).3, 90.0, "its reported height");

    // Removed: destroyed.
    app.tap("toggle").unwrap();
    assert_eq!(calls(&app).last(), Some(&HostCall::Destroy { id }));
    assert!(!app.native_event(id, NativeEvent { name: "load".into(), data: serde_json::Value::Null }), "a gone view's events go nowhere");
}

#[test]
fn inspect_shows_the_view() {
    let app = App::new(view);
    let snap = app.inspect();
    let web = snap.find("web").unwrap();
    assert_eq!(web.role, "web_view");
    let n = web.native.as_ref().unwrap();
    assert_eq!((n.kind.as_str(), n.slot.as_str(), n.props["url"].as_str()), ("web", "overlay", Some("https://example.com")));
    let _ = NativeId(0);
}

fn badge_over(slot: Slot, label: bool) -> App {
    App::new(move || {
        let view = native_view("map").slot(slot).id("map").w(px(200.0)).h(px(120.0));
        let view = if label { view.label("Map of the venue") } else { view };
        el().w(px(300.0))
            .h(px(200.0))
            .child(view)
            .child(text("3 new").id("badge").absolute().top(px(10.0)).left(px(10.0)).bg(Token::Accent).text_color(Token::AccentText))
            .into()
    })
}

#[test]
fn content_over_an_overlay_view_is_reported() {
    let codes = |app: &App| -> Vec<(String, Option<String>)> {
        let mut v: Vec<_> = app.layout_issues().iter().map(|i| (i.code.id.to_owned(), i.test_id.clone())).collect();
        v.extend(app.kernel().accessibility_issues().iter().map(|i| (i.code.id.to_owned(), i.test_id.clone())));
        v.sort();
        v
    };
    let over = badge_over(Slot::Overlay, true);
    assert_eq!(codes(&over), [("NATIVE_COVERED".to_string(), Some("badge".to_string()))]);
    let under = badge_over(Slot::Underlay, true);
    assert!(codes(&under).is_empty(), "content draws over an underlay view: {:?}", codes(&under));
    let unlabeled = badge_over(Slot::Underlay, false);
    assert_eq!(codes(&unlabeled), [("UNLABELED_NATIVE_VIEW".to_string(), Some("map".to_string()))]);
}

#[test]
fn a_snapshot_draws_in_the_views_place() {
    let mut app = App::new(view);
    let HostCall::Create { id, .. } = calls(&app)[0].clone() else { panic!() };
    let images = |app: &App| app.display_list().items.iter().filter(|i| matches!(i, octara::paint::Item::Image(_))).count();
    assert_eq!(images(&app), 0, "a placeholder box");
    assert!(app.snapshot_native().is_empty(), "the recording host takes no snapshots");

    app.native_snapshot(id, Some(Image::rgba(1, 1, vec![255, 0, 0, 255])));
    let list = app.display_list();
    let Some(octara::paint::Item::Image(i)) = list.items.iter().find(|i| matches!(i, octara::paint::Item::Image(_))) else { panic!() };
    let web = app.kernel().find("web").unwrap();
    let (x, y, w, h) = app.kernel().rect(web);
    assert_eq!((i.rect.x, i.rect.y, i.rect.w, i.rect.h), (x, y, w, h), "the snapshot fills the view's box");

    app.native_snapshot(id, None);
    assert_eq!(images(&app), 0, "cleared: the placeholder again");
}

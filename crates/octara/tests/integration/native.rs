//! Native views headless: the platform window is presented creation, props, placements (with clips
//! and scrolling) and removal; events and sizes come back from it; inspect shows the view; check
//! finds content an overlay view would hide, and views without a label.

use octara::native::{NativeChange, NativeEvent, Slot};
use octara::platform::{HeadlessPlatform, HeadlessWindow};
use octara::prelude::*;

/// The native changes the window's platform was presented.
fn calls(app: &Window) -> Vec<NativeChange> {
    app.platform_window().as_any().downcast_ref::<HeadlessWindow>().unwrap().native.clone()
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
            col()
                .id("page")
                .h(px(200.0))
                .overflow_y_scroll()
                .child(show(
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
                ))
                .child(el().h(px(400.0))),
        )
        .into()
}

#[test]
fn the_host_follows_the_view() {
    let mut app = Window::headless(view);
    let c = calls(&app);
    let NativeChange::Create { id, kind, props, .. } = &c[0] else { panic!("{c:?}") };
    assert_eq!((kind.as_str(), props["url"].as_str()), ("web", Some("https://example.com")));
    let id = *id;
    let NativeChange::Place { placements } = &c[1] else { panic!("{c:?}") };
    let p = &placements[0];
    assert_eq!((p.id, p.slot, p.rect.w, p.rect.h, p.visible), (id, Slot::Overlay, 300.0, 150.0, true), "150 high until it reports a size");
    assert_eq!(p.clip.h, 200.0, "clipped by the scroll container");

    // Props are handed over whole when they change; a placement only when it changes.
    let before = calls(&app).len();
    app.tap("next").unwrap();
    let c = calls(&app);
    assert_eq!(c[before..], [NativeChange::Update { id, props: serde_json::json!({ "url": "https://example.org" }) }]);

    // Scrolling moves it; scrolled out of its clip, it is not visible.
    app.dispatch(Event::ScrollTo("page".into(), 0.0, 100.0)).unwrap();
    let Some(NativeChange::Place { placements }) = calls(&app).last().cloned() else { panic!() };
    assert_eq!(placements[0].rect.y + 100.0, p.rect.y);
    app.dispatch(Event::ScrollTo("page".into(), 0.0, 400.0)).unwrap();
    let Some(NativeChange::Place { placements }) = calls(&app).last().cloned() else { panic!() };
    assert!(!placements[0].visible);

    // Events and sizes come back.
    assert!(app.event(InputEvent::Native {
        id,
        event: NativeEvent { name: "load".into(), data: serde_json::json!({ "url": "https://example.org" }) }
    }));
    assert_eq!(app.kernel().text_of("loaded").unwrap(), "https://example.org");
    app.event(InputEvent::NativeSize { id, width: 300.0, height: 90.0 });
    let web = app.kernel().find("web").unwrap();
    assert_eq!(app.kernel().rect(web).3, 90.0, "its reported height");

    // Removed: destroyed.
    app.tap("toggle").unwrap();
    assert_eq!(calls(&app).last(), Some(&NativeChange::Destroy { id }));
    assert!(
        !app.event(InputEvent::Native { id, event: NativeEvent { name: "load".into(), data: serde_json::Value::Null } }),
        "a gone view's events go nowhere"
    );
}

#[test]
fn inspect_shows_the_view() {
    let app = Window::headless(view);
    let snap = app.inspect();
    let web = snap.find("web").unwrap();
    assert_eq!(web.role, "web_view");
    let n = web.native.as_ref().unwrap();
    assert_eq!((n.kind.as_str(), n.slot.as_str(), n.props["url"].as_str()), ("web", "overlay", Some("https://example.com")));
}

fn badge_over(slot: Slot, label: bool) -> Window {
    Window::headless(move || {
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
    let codes = |app: &Window| -> Vec<(String, Option<String>)> {
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

/// A platform that shows every native view (a shell's).
fn shows() -> HeadlessPlatform {
    HeadlessPlatform::with_window(HeadlessWindow { shows_native: true, ..HeadlessWindow::default() })
}

fn map_with_controls() -> View {
    el().size_full()
        .child(
            native_view("map")
                .slot(Slot::Underlay)
                .id("map")
                .label("Map")
                .absolute()
                .top(px(10.0))
                .left(px(10.0))
                .w(px(200.0))
                .h(px(100.0)),
        )
        .child(
            el().id("zoom")
                .role(Role::Button)
                .label("Zoom")
                .absolute()
                .top(px(20.0))
                .left(px(20.0))
                .size_8()
                .bg(Token::Accent)
                .on_press(|| {}),
        )
        .into()
}

#[test]
fn a_shown_underlay_view_is_cut_out_and_its_covered_parts_known() {
    use octara::paint::Item;
    let app = Window::headless(map_with_controls);
    let natives = |app: &Window| -> Vec<bool> {
        app.display_list().items.iter().filter_map(|i| if let Item::Native { cut, .. } = i { Some(*cut) } else { None }).collect()
    };
    assert_eq!(natives(&app), [false], "headless: a placeholder, no hole");

    let app = Window::headless_on(shows(), map_with_controls);
    let list = app.display_list();
    assert_eq!(natives(&app), [true], "shown by the platform: cut out");
    let map = app.kernel().find("map").unwrap();
    assert!(!list.items.iter().any(|i| matches!(i, Item::Quad(q) if q.source == map)), "no placeholder under a view the platform shows");
    let placements = app.kernel().native_placements();
    let covered = &placements[0].1.covered;
    assert_eq!(covered.len(), 1, "{covered:?}");
    let zoom = app.kernel().find("zoom").unwrap();
    let (x, y, w, h) = app.kernel().rect(zoom);
    assert_eq!((covered[0].x, covered[0].y, covered[0].w, covered[0].h), (x, y, w, h), "the button over the map is Octara's");
}

//! Captures: the `capture` request settles, asks the host for snapshots, paints, and reports
//! what each native view shows; snapshots draw in their views' places and are cleared after.

use octara::capture::{Painter, Picture, Pixels, Shown};
use octara::inspect::{Request, handle};
use octara::native::{NativeHost, NativeId, Placement};
use octara::paint::{DisplayList, Item};
use octara::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Paints a blank picture and counts the images (snapshots) in the list it was given.
struct Counting(Rc<RefCell<Vec<usize>>>);

impl Painter for Counting {
    fn renderer(&self) -> &'static str {
        "test"
    }
    fn paint(&mut self, list: &DisplayList, scale: f32) -> Result<Picture, String> {
        self.0.borrow_mut().push(list.items.iter().filter(|i| matches!(i, Item::Image(_))).count());
        let (w, h) = ((list.size.0 * scale) as u32, (list.size.1 * scale) as u32);
        Ok(Picture { width: w, height: h, rgba: vec![255; (w * h * 4) as usize] })
    }
}

/// A host that takes snapshots (handed back by the test) and can be busy.
#[derive(Default)]
struct Snapping {
    busy: Rc<RefCell<Option<String>>>,
}

impl NativeHost for Snapping {
    fn create(&mut self, _: NativeId, _: &str, _: octara::native::Slot, _: &serde_json::Value) -> bool {
        true
    }
    fn update(&mut self, _: NativeId, _: &serde_json::Value) {}
    fn place(&mut self, _: &[Placement]) {}
    fn destroy(&mut self, _: NativeId) {}
    fn snapshot(&mut self, _: NativeId) -> Result<Vec<String>, String> {
        Ok(vec!["video shows black".into()])
    }
    fn busy(&self, _: NativeId) -> Option<String> {
        self.busy.borrow().clone()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn view() -> View {
    col().size_full().child(web_view("https://example.com").id("web").label("Page").w(px(200.0)).h(px(100.0))).into()
}

fn path(name: &str) -> String {
    std::env::temp_dir().join(format!("octara-capture-{}-{name}.png", std::process::id())).to_string_lossy().into_owned()
}

#[test]
fn a_headless_capture_reports_placeholders() {
    let mut app = Window::headless(view);
    let shot = path("headless");
    let reply = handle(&mut app, Request::Capture { path: shot.clone(), settle_ms: None, scale: Some(1.0), pixels: Pixels::Painted });
    assert_eq!(reply.code.as_deref(), Some("CAPTURE_FAILED"), "nothing paints without a painter");

    let painted = Rc::new(RefCell::new(Vec::new()));
    app.app().set_painter(Box::new(Counting(painted.clone())));
    let reply = handle(&mut app, Request::Capture { path: shot.clone(), settle_ms: None, scale: Some(1.0), pixels: Pixels::Display });
    let c = reply.capture.expect("a report");
    assert!(std::fs::read(&shot).unwrap().starts_with(b"\x89PNG"));
    std::fs::remove_file(&shot).unwrap();
    assert_eq!((c.pixels, c.renderer.as_str(), c.settled, c.width), (Pixels::Painted, "test", true, 800));
    assert!(c.fallback.as_deref().unwrap().contains("headless"), "display was asked: {:?}", c.fallback);
    let n = &c.native[0];
    assert_eq!((n.id.as_deref(), n.kind.as_str(), n.shown, n.rect[2]), (Some("web"), "web", Shown::Placeholder, 200.0));
    assert!(n.reason.as_deref().unwrap().contains("headless"));
}

#[test]
fn snapshots_draw_in_place_and_are_cleared() {
    let host = Snapping::default();
    let busy = host.busy.clone();
    let mut app = Window::headless_on(octara::platform::HeadlessPlatform::with_host(host), view);

    // A busy view keeps the app unsettled.
    *busy.borrow_mut() = Some("loading https://example.com".into());
    assert_eq!(app.unsettled(), ["web view #web: loading https://example.com"]);
    *busy.borrow_mut() = None;
    assert!(app.unsettled().is_empty());

    let coming = app.capture_begin();
    assert_eq!(coming.len(), 1);
    assert_eq!(app.capture_awaiting(), coming);
    app.native_snapshot(coming[0], Some(Image::rgba(1, 1, vec![255, 0, 0, 255])));
    assert!(app.capture_awaiting().is_empty());

    let painted = Rc::new(RefCell::new(Vec::new()));
    let shot = path("snapshot");
    let c = app.capture_finish(&mut Counting(painted.clone()), &shot, 1.0, vec![], None).unwrap();
    std::fs::remove_file(&shot).unwrap();
    assert_eq!(*painted.borrow(), [1], "the snapshot was painted in the view's place");
    assert_eq!((c.native[0].shown, c.native[0].misses.as_slice()), (Shown::Snapshot, ["video shows black".to_string()].as_slice()));
    let images = app.display_list().items.iter().filter(|i| matches!(i, Item::Image(_))).count();
    assert_eq!(images, 0, "cleared after: the live frame never shows a stale snapshot");

    // A snapshot that does not come is reported, not waited for forever.
    app.capture_begin();
    let shot = path("failed");
    let c = app.capture_finish(&mut Counting(painted.clone()), &shot, 1.0, vec!["motion".into()], None).unwrap();
    std::fs::remove_file(&shot).unwrap();
    assert_eq!((c.native[0].shown, c.settled, c.pending.as_slice()), (Shown::Failed, false, ["motion".to_string()].as_slice()));
}

#[test]
fn a_display_capture_shows_views_live() {
    let mut app = Window::headless(view);
    let shot = path("display");
    let picture = Picture { width: 2, height: 1, rgba: vec![0, 0, 0, 255, 255, 255, 255, 255] };
    let c = app.capture_display(&picture, &shot, 2.0, vec![]).unwrap();
    std::fs::remove_file(&shot).unwrap();
    assert_eq!((c.pixels, c.renderer.as_str(), c.width, c.settled), (Pixels::Display, "display", 2, true));
    assert_eq!((c.native[0].id.as_deref(), c.native[0].shown), (Some("web"), Shown::Live));
}

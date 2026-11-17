//! Captures: the `capture` request settles, asks the platform window for snapshots, paints, and
//! reports what each native view shows. Native images belong to the capture that requested them.

use octara::capture::{Pixels, Shown};
use octara::inspect::{Request, handle};
use octara::platform::{HeadlessPlatform, HeadlessWindow, NativeSnapshotRequest};
use octara::prelude::*;
use octara::view::{native_view, web_view};
use octara_primitives::capture::{Painter, Picture};
use octara_primitives::native::Slot;
use octara_primitives::paint::{DisplayList, Item};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Paints a blank picture and counts the images (snapshots) in the list it was given.
struct Counting(Rc<RefCell<Vec<usize>>>);

impl Painter for Counting {
    fn renderer(&self) -> &'static str {
        "test"
    }
    fn paint(&mut self, list: &DisplayList, scale: f32) -> Result<Picture, String> {
        assert!(!list.items().iter().any(|item| matches!(item, Item::Native { cut: true, .. })), "capture has no live native underlay");
        self.0.borrow_mut().push(list.items().iter().filter(|i| matches!(i, Item::Image(_))).count());
        let (w, h) = ((list.size.0 * scale) as u32, (list.size.1 * scale) as u32);
        Ok(Picture { width: w, height: h, rgba: vec![255; (w * h * 4) as usize] })
    }
}

/// A platform that shows native views, takes snapshots (handed back by the test) and can be busy.
fn snapping(busy: Rc<RefCell<Option<String>>>) -> HeadlessPlatform {
    HeadlessPlatform::with_window(HeadlessWindow {
        shows_native: true,
        busy,
        snapshots: Some(vec!["video shows black".into()]),
        ..HeadlessWindow::default()
    })
}

#[ui]
fn view() -> View {
    col().size_full().child(web_view("https://example.com").id("web").label("Page").w(px(200.0)).h(px(100.0))).into()
}

fn path(name: &str) -> String {
    std::env::temp_dir().join(format!("octara-capture-{}-{name}.png", std::process::id())).to_string_lossy().into_owned()
}

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(60)
}

fn requests(window: &Window) -> Vec<NativeSnapshotRequest> {
    window.platform_window().as_any().downcast_ref::<HeadlessWindow>().unwrap().snapshot_requests.clone()
}

fn reply(window: &mut Window, request: NativeSnapshotRequest, image: Option<Image>) -> bool {
    window.event(InputEvent::NativeSnapshot { request, image })
}

fn red_pixel() -> Image {
    Image::rgba(1, 1, vec![255, 0, 0, 255])
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
fn snapshots_draw_in_place_without_touching_the_live_frame() {
    let busy = Rc::new(RefCell::new(None));
    let mut app = Window::headless_on(snapping(busy.clone()), view);

    // A busy view keeps the app unsettled.
    *busy.borrow_mut() = Some("loading https://example.com".into());
    assert_eq!(app.unsettled(), ["web view #web: loading https://example.com"]);
    *busy.borrow_mut() = None;
    assert!(app.unsettled().is_empty());

    let capture = app.capture_begin(deadline()).unwrap();
    let frame = app.frames_presented();
    let live = app.display_list();
    let request = requests(&app)[0];
    assert_eq!(app.capture_awaiting(capture), [request.id]);
    assert!(reply(&mut app, request, Some(red_pixel())));
    assert!(app.capture_awaiting(capture).is_empty());
    assert!(Rc::ptr_eq(&live, &app.display_list()), "a native image must not dirty the live display-list cache while capture is active");

    let painted = Rc::new(RefCell::new(Vec::new()));
    let shot = path("snapshot");
    let c = app.capture_finish(capture, &mut Counting(painted.clone()), &shot, 1.0, vec![], None).unwrap();
    std::fs::remove_file(&shot).unwrap();
    assert_eq!(*painted.borrow(), [1], "the snapshot was painted in the view's place");
    assert_eq!((c.native[0].shown, c.native[0].misses.as_slice()), (Shown::Snapshot, ["video shows black".to_string()].as_slice()));
    assert!(Rc::ptr_eq(&live, &app.display_list()));
    assert_eq!(app.frames_presented(), frame);

    // A snapshot that does not come is reported, not waited for forever.
    let capture = app.capture_begin(Instant::now()).unwrap();
    let shot = path("failed");
    let c = app.capture_finish(capture, &mut Counting(painted.clone()), &shot, 1.0, vec!["motion".into()], None).unwrap();
    std::fs::remove_file(&shot).unwrap();
    assert_eq!((c.native[0].shown, c.settled, c.pending.as_slice()), (Shown::Failed, false, ["motion".to_string()].as_slice()));

    // A failed PNG write still drops the capture, and its error identifies the destination.
    let capture = app.capture_begin(deadline()).unwrap();
    let request = *requests(&app).last().unwrap();
    assert!(reply(&mut app, request, Some(red_pixel())));
    let shot = format!("{}/shot.png", path("missing-parent"));
    let error = app.capture_finish(capture, &mut Counting(painted), &shot, 1.0, vec![], None).unwrap_err();
    assert!(error.starts_with(&format!("{shot}: ")), "{error}");
    assert!(app.capture_awaiting(capture).is_empty());
    assert!(!reply(&mut app, request, Some(red_pixel())));
    assert!(Rc::ptr_eq(&live, &app.display_list()));
}

#[test]
fn a_timed_out_reply_cannot_complete_the_next_capture_of_the_same_view() {
    let mut app = Window::headless_on(snapping(Rc::default()), view);
    let painted = Rc::new(RefCell::new(Vec::new()));
    let mut painter = Counting(painted.clone());

    let timed_out = app.capture_begin(Instant::now()).unwrap();
    let first = requests(&app)[0];
    assert!(!reply(&mut app, first, Some(red_pixel())), "the deadline has elapsed while the capture is still active");
    let shot = path("timed-out");
    let capture = app.capture_finish(timed_out, &mut painter, &shot, 1.0, vec![], None).unwrap();
    std::fs::remove_file(&shot).unwrap();
    assert_eq!(capture.native[0].shown, Shown::Failed);
    assert!(!reply(&mut app, first, Some(red_pixel())), "the timed-out capture has ended");

    let active = app.capture_begin(deadline()).unwrap();
    let next = requests(&app)[1];
    assert_eq!(first.id, next.id);
    assert_ne!(first, next);
    assert!(!reply(&mut app, first, Some(red_pixel())), "a stale reply to the same view does not belong to this capture");
    app.capture_cancel(timed_out);
    assert!(app.capture_finish(timed_out, &mut painter, &shot, 1.0, vec![], None).is_err());
    assert_eq!(app.capture_awaiting(active), [next.id], "the old capture cannot cancel or finish its successor");
    assert!(reply(&mut app, next, Some(red_pixel())));
    assert!(!reply(&mut app, next, None), "a duplicate cannot replace the completed result");
    let shot = path("after-timeout");
    let capture = app.capture_finish(active, &mut painter, &shot, 1.0, vec![], None).unwrap();
    std::fs::remove_file(&shot).unwrap();
    assert_eq!(capture.native[0].shown, Shown::Snapshot);
    assert_eq!(*painted.borrow(), [0, 1]);
}

#[test]
fn a_resize_retires_pending_and_received_snapshots_of_the_previous_size() {
    #[ui]
    fn two_views() -> View {
        col()
            .size_full()
            .child(native_view("map").slot(Slot::Underlay).id("map").label("Map").w(200).h(100))
            .child(web_view("https://example.com").id("web").label("Page").w(200).h(100))
            .into()
    }

    let mut app = Window::headless_on(snapping(Rc::default()), two_views);
    assert!(app.display_list().items().iter().any(|item| matches!(item, Item::Native { cut: true, .. })));
    let painted = Rc::new(RefCell::new(Vec::new()));
    let mut painter = Counting(painted.clone());
    let active = app.capture_begin(deadline()).unwrap();
    let native_ids = app.capture_awaiting(active);
    assert_eq!(native_ids.len(), 2);
    let asked = requests(&app);
    assert!(reply(&mut app, asked[0], Some(red_pixel())));
    assert_eq!(app.capture_awaiting(active), [asked[1].id]);

    let (width, height) = app.size();
    app.resize(width, height);
    assert_eq!(app.capture_awaiting(active), [asked[1].id], "an unchanged host notification preserves the active requests");

    app.resize(420.0, 400.0);
    assert!(app.capture_awaiting(active).is_empty());
    assert!(!reply(&mut app, asked[1], Some(red_pixel())));
    let shot = path("after-resize");
    let capture = app.capture_finish(active, &mut painter, &shot, 1.0, vec![], None).unwrap();
    std::fs::remove_file(&shot).unwrap();
    assert_eq!((capture.width, capture.height), (420, 400));
    assert_eq!(capture.native.len(), 2);
    assert!(capture.native.iter().all(|view| view.shown == Shown::Failed), "{:?}", capture.native);

    let active = app.capture_begin(deadline()).unwrap();
    assert_eq!(app.capture_awaiting(active), native_ids);
    let asked = requests(&app);
    assert!(reply(&mut app, asked[3], None));
    assert_eq!(app.capture_awaiting(active), [asked[2].id], "an explicit native failure completes one request");
    assert!(reply(&mut app, asked[2], Some(red_pixel())));
    let shot = path("resized-snapshots");
    let capture = app.capture_finish(active, &mut painter, &shot, 1.0, vec![], None).unwrap();
    std::fs::remove_file(&shot).unwrap();
    assert_eq!(capture.native.iter().filter(|view| view.shown == Shown::Snapshot).count(), 1);
    assert_eq!(capture.native.iter().filter(|view| view.shown == Shown::Failed).count(), 1);
    assert_eq!(*painted.borrow(), [0, 1]);
}

#[test]
fn cancel_reload_and_close_refuse_outstanding_native_images() {
    let mut window = Window::headless_on(snapping(Rc::default()), view);
    let app = window.app();

    let active = window.capture_begin(deadline()).unwrap();
    let request = requests(&window)[0];
    window.capture_cancel(active);
    assert!(window.capture_awaiting(active).is_empty());
    assert!(!reply(&mut window, request, Some(red_pixel())));

    let active = window.capture_begin(deadline()).unwrap();
    let request = requests(&window)[1];
    app.hot_reload();
    assert!(window.capture_awaiting(active).is_empty());
    assert!(!reply(&mut window, request, Some(red_pixel())));
    let active = window.capture_begin(deadline()).unwrap();
    let replacement = requests(&window)[2];
    assert_ne!(request.id, replacement.id);
    assert_eq!(window.capture_awaiting(active), [replacement.id]);

    app.close_window(window.id());
    assert!(window.capture_awaiting(active).is_empty());
    assert!(!reply(&mut window, replacement, Some(red_pixel())));
    let mut painter = Counting(Rc::default());
    let shot = path("closed");
    assert!(window.capture_finish(active, &mut painter, &shot, 1.0, vec![], None).is_err());
    assert!(!std::path::Path::new(&shot).exists());
}

#[test]
fn a_display_capture_shows_views_live() {
    let mut app = Window::headless(view);
    let shot = path("display");
    let picture = Picture { width: 2, height: 1, rgba: vec![22, 44, 88, 0, 240, 128, 32, 96] };
    let c = app.capture_display(&picture, &shot, 2.0, vec![]).unwrap();
    let frame = Image::decode(&std::fs::read(&shot).unwrap()).unwrap().frame();
    assert_eq!((frame.width, frame.height), (picture.width, picture.height));
    assert_eq!(frame.rgba.as_ref(), picture.rgba);
    std::fs::remove_file(&shot).unwrap();
    assert_eq!((c.pixels, c.renderer.as_str(), c.width, c.settled), (Pixels::Display, "display", 2, true));
    assert_eq!((c.native[0].id.as_deref(), c.native[0].shown), (Some("web"), Shown::Live));
}

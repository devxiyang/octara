//! Native views on macOS ([design](../../../docs/design/native-island.md)): AppKit views in
//! winit's content view, placed with the frame they belong to.
//!
//! Each view sits in a clip container (a flipped `NSView` whose frame is the view's clip, its
//! layer masking to a rounded rect). Overlay containers are subviews of winit's view above its
//! Metal layer. Underlay containers live in a host view below every subview, the Metal layer
//! kept above it (z position) and made non-opaque, so the view shows through the cutout Octara's
//! renderers clear (Rune's structure). The host takes the pointer only where an underlay view is
//! and Octara content does not cover it; everywhere else it falls through to winit's view.
//!
//! The app's settle hands over placements; the shell applies them between the frame's draw and
//! its present, inside one `CATransaction` with actions off, and turns the Metal layer's
//! `presentsWithTransaction` on while views move or the window resizes, so a view and the
//! content around it reach the screen in the same frame.
//!
//! Kinds:
//! - `web` (`WKWebView`, overlay): props `url`, or `html` with an optional `base_url`; events
//!   `load` `{url}`, `title` `{title}`, `fail` `{error}`. Snapshots from WebKit's own.
//! - `map` (`MKMapView`, underlay): props `center` `[lat, lon]`, `span` `[lat, lon]` degrees,
//!   `style` (`standard`, `satellite`, `hybrid`); event `region` `{center, span}` after the user
//!   moves it. Snapshots from `MKMapSnapshotter` (the region, without annotations).
//!
//! Other kinds, and kinds in the other slot, keep their placeholder.

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSBitmapImageRep, NSDeviceRGBColorSpace, NSGraphicsContext, NSImage, NSView, NSWindowOrderingMode};
use objc2_foundation::{
    NSDictionary, NSError, NSKeyValueChangeKey, NSKeyValueObservingOptions, NSObjectNSKeyValueObserverRegistration, NSPoint, NSRect,
    NSSize, NSString, NSURL, NSURLRequest, ns_string,
};
use objc2_map_kit::{
    MKCoordinateRegion, MKCoordinateSpan, MKHybridMapConfiguration, MKImageryMapConfiguration, MKMapConfiguration, MKMapSnapshot,
    MKMapSnapshotOptions, MKMapSnapshotter, MKMapView, MKMapViewDelegate, MKStandardMapConfiguration,
};
use objc2_quartz_core::{CAMetalLayer, CATransaction};
use objc2_web_kit::{WKNavigation, WKNavigationDelegate, WKSnapshotConfiguration, WKWebView, WKWebViewConfiguration};
use octara::image::Image;
use octara::native::{NativeChange, NativeEvent, NativeId, Placement, Slot};
use octara::paint::Rect;
use octara_render::wgpu;
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::rc::Rc;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

/// Events views sent, waiting for the event loop (delegates run on the main thread, inside
/// AppKit's dispatch; the shell hands them to the app when it wakes).
pub(crate) struct Inbox {
    events: RefCell<Vec<(NativeId, NativeEvent)>>,
    /// Snapshots taken (none when the view could not take one).
    snapshots: RefCell<Vec<(NativeId, Option<Image>)>>,
    wake: Box<dyn Fn()>,
}

impl Inbox {
    fn push(&self, id: NativeId, name: &str, data: Value) {
        self.events.borrow_mut().push((id, NativeEvent { name: name.to_owned(), data }));
        (self.wake)();
    }

    pub(crate) fn take(&self) -> Vec<(NativeId, NativeEvent)> {
        std::mem::take(&mut *self.events.borrow_mut())
    }

    pub(crate) fn take_snapshots(&self) -> Vec<(NativeId, Option<Image>)> {
        std::mem::take(&mut *self.snapshots.borrow_mut())
    }
}

/// An `NSImage` as straight RGBA at `scale` pixels per point (drawn into a bitmap of a known
/// format, whatever the image's own representation).
fn rgba(image: &NSImage, scale: f64) -> Option<Image> {
    let (w, h, px) = pixels(image, scale)?;
    Some(Image::rgba(w, h, px))
}

/// An `NSImage`'s pixels, straight RGBA, at `scale` pixels per point.
fn pixels(image: &NSImage, scale: f64) -> Option<(u32, u32, Vec<u8>)> {
    let size = image.size();
    let (w, h) = ((size.width * scale).round() as isize, (size.height * scale).round() as isize);
    if w <= 0 || h <= 0 {
        return None;
    }
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            w,
            h,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            w * 4,
            32,
        )
    }?;
    rep.setSize(size);
    let ctx = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&ctx));
    image.drawInRect(NSRect::new(NSPoint::new(0.0, 0.0), size));
    ctx.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();
    let (data, stride) = (rep.bitmapData(), rep.bytesPerRow() as usize);
    if data.is_null() {
        return None;
    }
    let (w, h) = (w as usize, h as usize);
    let mut px = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        // SAFETY: the rep owns `h` rows of `stride` bytes.
        let row = unsafe { std::slice::from_raw_parts(data.add(y * stride), w * 4) };
        for p in row.as_chunks::<4>().0 {
            // Premultiplied in the bitmap; straight in an `Image`.
            let a = p[3] as u32;
            let un = |c: u8| ((c as u32 * 255 + a / 2).checked_div(a).unwrap_or(0)).min(255) as u8;
            px.extend([un(p[0]), un(p[1]), un(p[2]), p[3]]);
        }
    }
    Some((w as u32, h as u32, px))
}

/// The screen is locked: the window server draws no window.
fn screen_locked() -> bool {
    let Some(session) = objc2_core_graphics::CGSessionCopyCurrentDictionary() else { return false };
    // SAFETY: CFDictionary is toll-free bridged to NSDictionary.
    let dict: &NSDictionary<NSString, AnyObject> = unsafe { &*(&*session as *const _ as *const NSDictionary<NSString, AnyObject>) };
    dict.objectForKey(ns_string!("CGSSessionScreenIsLocked"))
        .and_then(|v| v.downcast::<objc2_foundation::NSNumber>().ok())
        .is_some_and(|n| n.boolValue())
}

/// `CGWindowListCreateImage`, found at run time: obsoleted in the SDK (ScreenCaptureKit replaces
/// it, but needs the screen-recording permission even for one's own window), still the window
/// server's way to give a process its own window without that permission. Linking it would stop
/// the app from launching on a system that drops it.
fn window_list_create_image() -> Option<unsafe extern "C-unwind" fn(NSRect, u32, u32, u32) -> *mut objc2_core_graphics::CGImage> {
    unsafe extern "C" {
        fn dlsym(handle: *mut c_void, symbol: *const std::ffi::c_char) -> *mut c_void;
    }
    // RTLD_DEFAULT.
    let sym = unsafe { dlsym(-2isize as *mut c_void, c"CGWindowListCreateImage".as_ptr()) };
    // SAFETY: the function's C signature (CGRect, CGWindowListOption, CGWindowID, CGWindowImageOption).
    (!sym.is_null()).then(|| unsafe { std::mem::transmute(sym) })
}

define_class!(
    // SAFETY: NSView has no subclassing requirements; `Clip` does not implement `Drop`.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraNativeClip"]
    struct Clip;

    impl Clip {
        /// Top-left origin, as Octara's coordinates.
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        /// The container is as large as the view's clip: only its view takes the pointer, the
        /// rest of it goes to what is below (Octara's content).
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            let hit: Option<Retained<NSView>> = unsafe { msg_send![super(self), hitTest: point] };
            let me: *const NSView = (self as *const Self).cast();
            hit.filter(|v| !std::ptr::eq(&**v, me))
        }
    }
);

impl Clip {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let clip: Retained<Self> = unsafe { msg_send![Self::alloc(mtm), initWithFrame: NSRect::ZERO] };
        clip.setWantsLayer(true);
        if let Some(layer) = clip.layer() {
            layer.setMasksToBounds(true);
        }
        clip
    }
}

/// Where the underlay host takes the pointer: an underlay view's visible area, minus what
/// Octara content covers there.
#[derive(Default)]
struct Regions(RefCell<Vec<(Rect, Vec<Rect>)>>);

impl Regions {
    fn takes(&self, x: f32, y: f32) -> bool {
        let inside = |r: &Rect| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h;
        self.0.borrow().iter().any(|(area, covered)| inside(area) && !covered.iter().any(inside))
    }
}

define_class!(
    // SAFETY: NSView has no subclassing requirements; `Underlay` does not implement `Drop`.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraNativeUnderlay"]
    #[ivars = Regions]
    struct Underlay;

    impl Underlay {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        /// An underlay view takes the pointer where it shows and nothing of Octara's covers it;
        /// elsewhere the event falls through to winit's view (`point` is in the superview's
        /// coordinates, which the host shares, flipped: Octara's logical pixels).
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            let hit: Option<Retained<NSView>> =
                if self.ivars().takes(point.x as f32, point.y as f32) { unsafe { msg_send![super(self), hitTest: point] } } else { None };
            let me: *const NSView = (self as *const Self).cast();
            hit.filter(|v| !std::ptr::eq(&**v, me))
        }
    }
);

impl Underlay {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Regions::default());
        let host: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        // Width and height follow winit's view.
        host.setAutoresizingMask(
            objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        host.setWantsLayer(true);
        host
    }
}

struct DelegateIvars {
    id: NativeId,
    inbox: Rc<Inbox>,
}

struct MapIvars {
    id: NativeId,
    inbox: Rc<Inbox>,
    /// Tiles are loading.
    loading: Cell<bool>,
}

fn region_json(r: MKCoordinateRegion) -> Value {
    json!({ "center": [r.center.latitude, r.center.longitude], "span": [r.span.latitudeDelta, r.span.longitudeDelta] })
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; `MapDelegate` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraMapDelegate"]
    #[ivars = MapIvars]
    struct MapDelegate;

    unsafe impl NSObjectProtocol for MapDelegate {}

    unsafe impl MKMapViewDelegate for MapDelegate {
        #[unsafe(method(mapView:regionDidChangeAnimated:))]
        fn region_changed(&self, map: &MKMapView, _animated: bool) {
            let i = self.ivars();
            i.inbox.push(i.id, "region", region_json(unsafe { map.region() }));
        }

        #[unsafe(method(mapViewWillStartLoadingMap:))]
        fn will_load(&self, _map: &MKMapView) {
            self.ivars().loading.set(true);
        }

        #[unsafe(method(mapViewDidFinishLoadingMap:))]
        fn did_load(&self, _map: &MKMapView) {
            self.ivars().loading.set(false);
        }
    }
);

impl MapDelegate {
    fn new(mtm: MainThreadMarker, id: NativeId, inbox: Rc<Inbox>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(MapIvars { id, inbox, loading: Cell::new(false) });
        unsafe { msg_send![super(this), init] }
    }
}

/// A map's region and style from its props (`None` for what the props leave out).
fn map_region(props: &Value) -> Option<MKCoordinateRegion> {
    let pair = |k: &str| -> Option<(f64, f64)> {
        let a = props.get(k)?.as_array()?;
        Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?))
    };
    let (lat, lon) = pair("center")?;
    let (dlat, dlon) = pair("span").unwrap_or((0.05, 0.05));
    Some(MKCoordinateRegion {
        center: objc2_core_location::CLLocationCoordinate2D { latitude: lat, longitude: lon },
        span: MKCoordinateSpan { latitudeDelta: dlat, longitudeDelta: dlon },
    })
}

fn map_style(props: &Value) -> Retained<MKMapConfiguration> {
    unsafe {
        match props.get("style").and_then(Value::as_str) {
            Some("satellite") => MKImageryMapConfiguration::new().into_super(),
            Some("hybrid") => MKHybridMapConfiguration::new().into_super(),
            _ => MKStandardMapConfiguration::new().into_super(),
        }
    }
}

/// Applies a map's props (region and style) when they changed since last applied: the user's
/// own panning stays until the app sets another region.
fn apply_map(map: &MKMapView, props: &Value, applied: &mut Value) {
    let now = json!({ "center": props.get("center"), "span": props.get("span"), "style": props.get("style") });
    if now == *applied {
        return;
    }
    unsafe {
        map.setPreferredConfiguration(&map_style(props));
        if let Some(r) = map_region(props) {
            map.setRegion_animated(r, false);
        }
    }
    *applied = now;
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; `WebDelegate` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraWebDelegate"]
    #[ivars = DelegateIvars]
    struct WebDelegate;

    impl WebDelegate {
        /// The page's title changed (observed on `title`).
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe(
            &self,
            _key_path: Option<&NSString>,
            object: Option<&AnyObject>,
            _change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>,
            _context: *mut c_void,
        ) {
            if let Some(web) = object.and_then(|o| o.downcast_ref::<WKWebView>()) {
                let title = unsafe { web.title() }.map(|t| t.to_string()).unwrap_or_default();
                self.send("title", json!({ "title": title }));
            }
        }
    }

    unsafe impl NSObjectProtocol for WebDelegate {}

    unsafe impl WKNavigationDelegate for WebDelegate {
        #[unsafe(method(webView:didFinishNavigation:))]
        fn did_finish(&self, web: &WKWebView, _navigation: Option<&WKNavigation>) {
            let url = unsafe { web.URL() }.and_then(|u| u.absoluteString()).map(|s| s.to_string());
            self.send("load", json!({ "url": url }));
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn did_fail(&self, _web: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            self.send("fail", json!({ "error": error.localizedDescription().to_string() }));
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn did_fail_provisional(&self, _web: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            self.send("fail", json!({ "error": error.localizedDescription().to_string() }));
        }
    }
);

impl WebDelegate {
    fn new(mtm: MainThreadMarker, id: NativeId, inbox: Rc<Inbox>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars { id, inbox });
        unsafe { msg_send![super(this), init] }
    }

    fn send(&self, name: &str, data: Value) {
        let ivars = self.ivars();
        ivars.inbox.push(ivars.id, name, data);
    }
}

/// The platform view behind a kind.
enum Platform {
    Web {
        web: Retained<WKWebView>,
        delegate: Retained<WebDelegate>,
        /// What was last loaded (`url`, `html`, `base_url`): other props changing does not reload.
        loaded: Value,
    },
    Map {
        map: Retained<MKMapView>,
        delegate: Retained<MapDelegate>,
        /// The props last applied (region and style).
        applied: Value,
    },
}

/// One hosted view.
struct Hosted {
    clip: Retained<Clip>,
    /// The platform view (the clip's subview).
    view: Retained<NSView>,
    platform: Platform,
    slot: Slot,
    placed: Option<Placement>,
}

/// The views of one window, shared by the app's host (create, update, place, destroy) and the
/// shell (applying placements with the frame).
pub(crate) struct Views {
    mtm: MainThreadMarker,
    parent: Retained<NSView>,
    inbox: Rc<Inbox>,
    hosted: HashMap<NativeId, Hosted>,
    /// Placements handed over by the last settle, applied with the next present.
    pending: Vec<Placement>,
    /// The Metal layer presents inside the transaction (views are moving).
    presenting: Cell<bool>,
    /// The host of underlay views, below winit's view's subviews (made with the first one).
    underlay: Option<Retained<Underlay>>,
    /// The window's Metal layer, kept above the underlay host.
    metal: Option<Retained<CAMetalLayer>>,
}

fn source(props: &Value) -> Value {
    json!({ "url": props.get("url"), "html": props.get("html"), "base_url": props.get("base_url") })
}

fn load(web: &WKWebView, props: &Value) {
    let url = |s: &str| NSURL::URLWithString(&NSString::from_str(s));
    if let Some(html) = props.get("html").and_then(Value::as_str) {
        let base = props.get("base_url").and_then(Value::as_str).and_then(url);
        unsafe { web.loadHTMLString_baseURL(&NSString::from_str(html), base.as_deref()) };
    } else if let Some(u) = props.get("url").and_then(Value::as_str).and_then(url) {
        unsafe { web.loadRequest(&NSURLRequest::requestWithURL(&u)) };
    }
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> NSRect {
    NSRect::new(NSPoint::new(x as f64, y as f64), NSSize::new(w.max(0.0) as f64, h.max(0.0) as f64))
}

impl Views {
    /// Whether the platform shows it: web views in the overlay, maps in the underlay.
    fn create(&mut self, id: NativeId, kind: &str, slot: Slot, props: &Value) -> bool {
        let mtm = self.mtm;
        let (view, platform): (Retained<NSView>, Platform) = match (kind, slot) {
            ("web", Slot::Overlay) => {
                let (web, delegate) = self.make_web(id, props);
                (web.clone().into_super(), Platform::Web { web, delegate, loaded: source(props) })
            }
            ("map", Slot::Underlay) => {
                let map = unsafe { MKMapView::initWithFrame(MKMapView::alloc(mtm), NSRect::ZERO) };
                let delegate = MapDelegate::new(mtm, id, self.inbox.clone());
                unsafe { map.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
                let mut applied = Value::Null;
                apply_map(&map, props, &mut applied);
                (map.clone().into_super(), Platform::Map { map, delegate, applied })
            }
            _ => return false,
        };
        view.setWantsLayer(true);
        if let Some(layer) = view.layer() {
            layer.setMasksToBounds(true);
        }
        let clip = Clip::new(mtm);
        clip.addSubview(&view);
        // Shown once placed.
        clip.setHidden(true);
        self.hosted.insert(id, Hosted { clip, view, platform, slot, placed: None });
        true
    }

    fn make_web(&self, id: NativeId, props: &Value) -> (Retained<WKWebView>, Retained<WebDelegate>) {
        let mtm = self.mtm;
        let config = unsafe { WKWebViewConfiguration::new(mtm) };
        let web = unsafe { WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), NSRect::ZERO, &config) };
        let delegate = WebDelegate::new(mtm, id, self.inbox.clone());
        unsafe {
            web.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            web.addObserver_forKeyPath_options_context(
                &delegate,
                ns_string!("title"),
                NSKeyValueObservingOptions::New,
                std::ptr::null_mut(),
            );
        }
        load(&web, props);
        (web, delegate)
    }

    fn update(&mut self, id: NativeId, props: &Value) {
        let Some(h) = self.hosted.get_mut(&id) else { return };
        match &mut h.platform {
            Platform::Web { web, loaded, .. } => {
                let src = source(props);
                if src != *loaded {
                    load(web, props);
                    *loaded = src;
                }
            }
            Platform::Map { map, applied, .. } => apply_map(map, props, applied),
        }
    }

    fn destroy(&mut self, id: NativeId) {
        let Some(h) = self.hosted.remove(&id) else { return };
        match &h.platform {
            Platform::Web { web, delegate, .. } => unsafe {
                web.removeObserver_forKeyPath(delegate, ns_string!("title"));
                web.setNavigationDelegate(None);
                web.stopLoading();
            },
            Platform::Map { map, .. } => unsafe { map.setDelegate(None) },
        }
        h.clip.removeFromSuperview();
        self.update_regions();
    }

    /// Takes a snapshot of view `id`, handed to the inbox when the platform has it.
    fn snapshot(&self, id: NativeId) -> Result<Vec<String>, String> {
        let Some(h) = self.hosted.get(&id) else { return Err("no platform view of this kind on macOS yet".into()) };
        if h.placed.is_none() {
            return Err("not placed yet".into());
        }
        let scale = self.parent.window().map_or(2.0, |w| w.backingScaleFactor());
        let inbox = self.inbox.clone();
        let send = move |image: Option<Image>| {
            inbox.snapshots.borrow_mut().push((id, image));
            (inbox.wake)();
        };
        match &h.platform {
            Platform::Web { web, .. } => {
                let config = unsafe { WKSnapshotConfiguration::new(self.mtm) };
                // Not waiting for the next screen update: a hidden or occluded window gets none.
                unsafe { config.setAfterScreenUpdates(false) };
                let done = RcBlock::new(move |image: *mut NSImage, _error: *mut NSError| {
                    // SAFETY: WebKit passes an image or null.
                    send(unsafe { image.as_ref() }.and_then(|i| rgba(i, scale)));
                });
                unsafe { web.takeSnapshotWithConfiguration_completionHandler(Some(&config), &done) };
                Ok(vec!["hardware-decoded video and WebGL show black (WebKit's snapshot)".into()])
            }
            Platform::Map { map, .. } => {
                let options = unsafe { MKMapSnapshotOptions::new() };
                unsafe {
                    options.setRegion(map.region());
                    options.setPreferredConfiguration(&map.preferredConfiguration());
                    options.setSize(map.bounds().size);
                }
                let snapshotter = unsafe { MKMapSnapshotter::initWithOptions(MKMapSnapshotter::alloc(), &options) };
                let keep = RefCell::new(Some(snapshotter.clone()));
                let done = RcBlock::new(move |shot: *mut MKMapSnapshot, _error: *mut NSError| {
                    // SAFETY: MapKit passes a snapshot or null.
                    let image = unsafe { shot.as_ref() }.and_then(|s| rgba(&*unsafe { s.image() }, scale));
                    // The snapshotter lives until it answers.
                    keep.borrow_mut().take();
                    send(image);
                });
                unsafe { snapshotter.startWithCompletionHandler(RcBlock::as_ptr(&done)) };
                Ok(vec!["annotations and overlays are not drawn (MapKit's snapshotter)".into()])
            }
        }
    }

    /// A page or map tiles still loading.
    fn busy(&self, id: NativeId) -> Option<String> {
        let h = self.hosted.get(&id)?;
        match &h.platform {
            Platform::Web { web, .. } => unsafe { web.isLoading() }.then(|| {
                let url = unsafe { web.URL() }.and_then(|u| u.absoluteString()).map(|s| s.to_string()).unwrap_or_default();
                format!("loading {url}").trim_end().to_owned()
            }),
            Platform::Map { delegate, .. } => delegate.ivars().loading.get().then(|| "loading map tiles".to_owned()),
        }
    }

    /// The underlay host: made with the first underlay view, below every subview of winit's
    /// view, with the Metal layer above it.
    fn underlay_host(&mut self) -> Retained<Underlay> {
        if let Some(h) = &self.underlay {
            return h.clone();
        }
        let host = Underlay::new(self.mtm, self.parent.bounds());
        self.parent.addSubview_positioned_relativeTo(&host, NSWindowOrderingMode::Below, None);
        if let (Some(root), Some(host_layer), Some(metal)) = (self.parent.layer(), host.layer(), self.metal.as_ref()) {
            let inside = unsafe { root.sublayers() }.is_some_and(|ls| ls.iter().any(|l| std::ptr::eq(&*l, &***metal)));
            if !inside {
                root.insertSublayer_above(metal, Some(&host_layer));
            }
            // AppKit re-sorts sublayers to its subviews' order when it updates the layer tree;
            // siblings composite by z position first (Rune).
            metal.setZPosition(1.0);
        }
        self.underlay = Some(host.clone());
        host
    }

    /// The pointer regions of the underlay host: each placed, visible underlay view.
    fn update_regions(&self) {
        let Some(host) = &self.underlay else { return };
        let regions = self
            .hosted
            .values()
            .filter(|h| h.slot == Slot::Underlay)
            .filter_map(|h| h.placed.as_ref())
            .filter(|p| p.visible)
            .map(|p| (p.rect.intersect(&p.clip), p.covered.clone()))
            .collect();
        *host.ivars().0.borrow_mut() = regions;
    }

    /// Whether the Metal layer must be non-opaque (an underlay view shows through it).
    fn transparent(&self) -> bool {
        self.hosted.values().any(|h| h.slot == Slot::Underlay)
    }

    /// Placements waiting that move or resize a view (or show one for the first time).
    fn moving(&self) -> bool {
        self.pending.iter().any(|p| {
            self.hosted
                .get(&p.id)
                .is_some_and(|h| h.placed.as_ref().is_none_or(|q| q.rect != p.rect || q.clip != p.clip || q.visible != p.visible))
        })
    }

    /// Applies the waiting placements (inside the frame's transaction).
    fn apply(&mut self) {
        let flipped = self.parent.isFlipped();
        let height = self.parent.bounds().size.height as f32;
        let pending = std::mem::take(&mut self.pending);
        let underlay = pending.iter().any(|p| self.hosted.get(&p.id).is_some_and(|h| h.slot == Slot::Underlay && h.placed.is_none()));
        let host = underlay.then(|| self.underlay_host());
        for p in pending {
            let Some(h) = self.hosted.get_mut(&p.id) else { continue };
            if h.placed.is_none() {
                match h.slot {
                    Slot::Overlay => {
                        self.parent.addSubview(&h.clip);
                        // Above the Metal layer, which sits at z 1 once an underlay exists.
                        if let Some(layer) = h.clip.layer() {
                            layer.setZPosition(2.0);
                        }
                    }
                    Slot::Underlay => host.as_ref().expect("the underlay host").addSubview(&h.clip),
                }
            }
            let c = p.clip;
            let y = if flipped { c.y } else { height - c.y - c.h };
            h.clip.setFrame(rect(c.x, y, c.w, c.h));
            if let Some(layer) = h.clip.layer() {
                layer.setCornerRadius(p.clip_radius as f64);
            }
            h.view.setFrame(rect(p.rect.x - c.x, p.rect.y - c.y, p.rect.w, p.rect.h));
            if let Some(layer) = h.view.layer() {
                layer.setCornerRadius(p.radius as f64);
            }
            h.clip.setAlphaValue(p.opacity as f64);
            h.clip.setHidden(!p.visible);
            h.placed = Some(p);
        }
        self.update_regions();
    }
}

/// The window's native views as its platform window drives them: changes from each frame,
/// and the questions the app asks.
pub(crate) struct Host(Rc<RefCell<Views>>);

impl Host {
    /// Applies one frame's change (placements wait for the present).
    pub(crate) fn apply(&self, change: NativeChange) {
        let mut v = self.0.borrow_mut();
        match change {
            NativeChange::Create { id, kind, slot, props } => {
                v.create(id, &kind, slot, &props);
            }
            NativeChange::Update { id, props } => v.update(id, &props),
            NativeChange::Place { placements } => {
                for p in placements {
                    // A later placement of the same view replaces one not yet applied.
                    v.pending.retain(|q| q.id != p.id);
                    v.pending.push(p);
                }
            }
            NativeChange::Destroy { id } => {
                v.pending.retain(|q| q.id != id);
                v.destroy(id);
            }
        }
    }
    /// The kinds this backend shows: web views in the overlay, maps in the underlay.
    pub(crate) fn shows(kind: &str, slot: Slot) -> bool {
        matches!((kind, slot), ("web", Slot::Overlay) | ("map", Slot::Underlay))
    }
    pub(crate) fn snapshot(&self, id: NativeId) -> Result<Vec<String>, String> {
        self.0.borrow().snapshot(id)
    }
    pub(crate) fn busy(&self, id: NativeId) -> Option<String> {
        self.0.borrow().busy(id)
    }
}

/// The shell's side: the views, the inbox, and the frame's transaction.
pub(crate) struct Native {
    views: Rc<RefCell<Views>>,
    pub(crate) inbox: Rc<Inbox>,
}

impl Native {
    /// The window's native views, and the host to give the app; `None` without an AppKit view.
    pub(crate) fn new(window: &Window, wake: impl Fn() + 'static) -> Option<(Self, Host)> {
        let mtm = MainThreadMarker::new()?;
        let RawWindowHandle::AppKit(handle) = window.window_handle().ok()?.as_raw() else { return None };
        // SAFETY: winit's content view, alive as long as the window.
        let parent: Retained<NSView> = unsafe { Retained::retain(handle.ns_view.as_ptr().cast()) }?;
        let inbox = Rc::new(Inbox { events: RefCell::default(), snapshots: RefCell::default(), wake: Box::new(wake) });
        let views = Rc::new(RefCell::new(Views {
            mtm,
            parent,
            inbox: inbox.clone(),
            hosted: HashMap::new(),
            pending: Vec::new(),
            presenting: Cell::new(false),
            underlay: None,
            metal: None,
        }));
        Some((Native { views: views.clone(), inbox }, Host(views)))
    }

    /// Whether the Metal layer must be non-opaque now (an underlay view shows through it): the
    /// shell configures the surface with a non-opaque alpha mode.
    pub(crate) fn transparent(&self) -> bool {
        self.views.borrow().transparent()
    }

    /// Before the frame's surface texture is taken: opens the transaction the placements and
    /// the present go in, and presents inside it while views move or the window resizes.
    /// Returns whether a transaction is open (only while views exist).
    pub(crate) fn begin_frame(&self, surface: &wgpu::Surface<'_>, resized: bool) -> bool {
        let mut v = self.views.borrow_mut();
        if v.hosted.is_empty() {
            return false;
        }
        if v.metal.is_none() {
            // SAFETY: the Metal surface's layer, read on the main thread.
            v.metal = unsafe { surface.as_hal::<wgpu::hal::api::Metal>() }.map(|hal| hal.render_layer().lock().clone());
        }
        let presenting = resized || v.moving();
        if presenting != v.presenting.get() {
            v.presenting.set(presenting);
            if let Some(metal) = &v.metal {
                metal.setPresentsWithTransaction(presenting);
            }
        }
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        true
    }

    /// After the frame is drawn, before it is presented: places the views.
    pub(crate) fn place(&self) {
        self.views.borrow_mut().apply();
    }

    /// Where each view is in AppKit (in winit's view, logical pixels) next to its placement:
    /// a capture prints it, so the platform's frames are checked, not only the layout's.
    pub(crate) fn frames(&self) -> Vec<String> {
        let v = self.views.borrow();
        let mut out: Vec<_> = v
            .hosted
            .iter()
            .map(|(id, h)| {
                let f = h.view.convertRect_toView(h.view.bounds(), Some(&v.parent));
                let p = h.placed.as_ref().map(|p| p.rect);
                format!(
                    "view {}: in AppKit at {:.0},{:.0} {:.0}×{:.0}, placed at {}",
                    id.0,
                    f.origin.x,
                    f.origin.y,
                    f.size.width,
                    f.size.height,
                    p.map_or("nowhere".into(), |r| format!("{:.0},{:.0} {:.0}×{:.0}", r.x, r.y, r.w, r.h)),
                )
            })
            .collect();
        out.sort();
        out
    }

    /// The window server's picture of the window's content (what the user sees, native views
    /// live), straight RGBA at the window's backing scale; `Err` says why there is none.
    pub(crate) fn window_picture(&self) -> Result<octara::capture::Picture, String> {
        if screen_locked() {
            return Err("the screen is locked: the window server draws no window".into());
        }
        let create = window_list_create_image().ok_or("this system has no CGWindowListCreateImage")?;
        let v = self.views.borrow();
        let window = v.parent.window().ok_or("the view is in no window")?;
        if !window.isVisible() || !window.occlusionState().contains(objc2_app_kit::NSWindowOcclusionState::Visible) {
            return Err("the window is not visible (hidden, minimized or covered)".into());
        }
        let scale = window.backingScaleFactor();
        // The content view in global display coordinates (top-left origin of the main display).
        let local = v.parent.convertRect_toView(v.parent.bounds(), None);
        let screen = window.convertRectToScreen(local);
        let main = objc2_app_kit::NSScreen::screens(self.views.borrow().mtm).firstObject().ok_or("no screen")?;
        let top = main.frame().size.height;
        let bounds = NSRect::new(NSPoint::new(screen.origin.x, top - screen.origin.y - screen.size.height), screen.size);
        // Including the window only; framing ignored; best resolution.
        let image = unsafe { create(bounds, 1 << 3, window.windowNumber() as u32, (1 << 0) | (1 << 3)) };
        // SAFETY: a +1 CGImage or null (a Create function).
        let image = std::ptr::NonNull::new(image).map(|p| unsafe { objc2_core_foundation::CFRetained::from_raw(p) });
        let image = image.ok_or("the window server gave no picture")?;
        let (w, h) = (objc2_core_graphics::CGImage::width(Some(&image)), objc2_core_graphics::CGImage::height(Some(&image)));
        let (ew, eh) = ((screen.size.width * scale).round() as usize, (screen.size.height * scale).round() as usize);
        if w.abs_diff(ew) > 2 || h.abs_diff(eh) > 2 {
            return Err(format!("the window server's picture is not the window ({w}×{h}, the window is {ew}×{eh}): a sleeping display?"));
        }
        let ns = NSImage::initWithCGImage_size(NSImage::alloc(), &image, screen.size);
        let (width, height, rgba) = pixels(&ns, scale).ok_or("the picture could not be read")?;
        Ok(octara::capture::Picture { width, height, rgba })
    }

    /// After the present: commits the transaction.
    pub(crate) fn end_frame() {
        CATransaction::commit();
    }
}

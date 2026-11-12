//! Native views on macOS ([design](../../../docs/design/native-island.md)): AppKit views in
//! winit's content view, placed with the frame they belong to.
//!
//! Each view sits in a clip container (a flipped `NSView` whose frame is the view's clip, its
//! layer masking to a rounded rect); containers are subviews of winit's view, above its Metal
//! layer (the overlay slot). The app's settle hands over placements; the shell applies them
//! between the frame's draw and its present, inside one `CATransaction` with actions off, and
//! turns the Metal layer's `presentsWithTransaction` on while views move or the window resizes,
//! so a view and the content around it reach the screen in the same frame.
//!
//! Kinds: `web` (`WKWebView`: props `url`, or `html` with an optional `base_url`; events
//! `load` `{url}`, `title` `{title}`, `fail` `{error}`). Other kinds keep their placeholder, as do
//! views in the underlay slot (a later step).

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::NSView;
use objc2_foundation::{
    NSDictionary, NSError, NSKeyValueChangeKey, NSKeyValueObservingOptions, NSObjectNSKeyValueObserverRegistration,
    NSPoint, NSRect, NSSize, NSString, NSURL, NSURLRequest, ns_string,
};
use objc2_quartz_core::CATransaction;
use objc2_web_kit::{WKNavigation, WKNavigationDelegate, WKWebView, WKWebViewConfiguration};
use octara::native::{NativeEvent, NativeHost, NativeId, Placement, Slot};
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

struct DelegateIvars {
    id: NativeId,
    inbox: Rc<Inbox>,
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

/// One hosted view.
struct Hosted {
    clip: Retained<Clip>,
    web: Retained<WKWebView>,
    delegate: Retained<WebDelegate>,
    /// What was last loaded (`url`, `html`, `base_url`): other props changing does not reload.
    loaded: Value,
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
    fn create(&mut self, id: NativeId, kind: &str, props: &Value) {
        if kind != "web" {
            return;
        }
        let mtm = self.mtm;
        let config = unsafe { WKWebViewConfiguration::new(mtm) };
        let web = unsafe { WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), NSRect::ZERO, &config) };
        let delegate = WebDelegate::new(mtm, id, self.inbox.clone());
        unsafe {
            web.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            web.addObserver_forKeyPath_options_context(&delegate, ns_string!("title"), NSKeyValueObservingOptions::New, std::ptr::null_mut());
        }
        web.setWantsLayer(true);
        if let Some(layer) = web.layer() {
            layer.setMasksToBounds(true);
        }
        let clip = Clip::new(mtm);
        clip.addSubview(&web);
        // Shown once placed.
        clip.setHidden(true);
        load(&web, props);
        self.hosted.insert(id, Hosted { clip, web, delegate, loaded: source(props), placed: None });
    }

    fn update(&mut self, id: NativeId, props: &Value) {
        let Some(h) = self.hosted.get_mut(&id) else { return };
        let src = source(props);
        if src != h.loaded {
            load(&h.web, props);
            h.loaded = src;
        }
    }

    fn destroy(&mut self, id: NativeId) {
        let Some(h) = self.hosted.remove(&id) else { return };
        unsafe {
            h.web.removeObserver_forKeyPath(&h.delegate, ns_string!("title"));
            h.web.setNavigationDelegate(None);
            h.web.stopLoading();
        }
        h.clip.removeFromSuperview();
    }

    /// Placements waiting that move or resize a view (or show one for the first time).
    fn moving(&self) -> bool {
        self.pending.iter().any(|p| {
            self.hosted.get(&p.id).is_some_and(|h| {
                h.placed.as_ref().is_none_or(|q| q.rect != p.rect || q.clip != p.clip || q.visible != p.visible)
            })
        })
    }

    /// Applies the waiting placements (inside the frame's transaction).
    fn apply(&mut self) {
        let flipped = self.parent.isFlipped();
        let height = self.parent.bounds().size.height as f32;
        for p in std::mem::take(&mut self.pending) {
            let Some(h) = self.hosted.get_mut(&p.id) else { continue };
            if p.slot == Slot::Underlay {
                // The underlay host comes with step 3: the placeholder stays.
                continue;
            }
            if h.placed.is_none() {
                self.parent.addSubview(&h.clip);
            }
            let c = p.clip;
            let y = if flipped { c.y } else { height - c.y - c.h };
            h.clip.setFrame(rect(c.x, y, c.w, c.h));
            if let Some(layer) = h.clip.layer() {
                layer.setCornerRadius(p.clip_radius as f64);
            }
            h.web.setFrame(rect(p.rect.x - c.x, p.rect.y - c.y, p.rect.w, p.rect.h));
            if let Some(layer) = h.web.layer() {
                layer.setCornerRadius(p.radius as f64);
            }
            h.clip.setAlphaValue(p.opacity as f64);
            h.clip.setHidden(!p.visible);
            h.placed = Some(p);
        }
    }
}

/// The host the app talks to.
struct MacHost(Rc<RefCell<Views>>);

impl NativeHost for MacHost {
    fn create(&mut self, id: NativeId, kind: &str, props: &Value) {
        self.0.borrow_mut().create(id, kind, props);
    }
    fn update(&mut self, id: NativeId, props: &Value) {
        self.0.borrow_mut().update(id, props);
    }
    fn place(&mut self, placements: &[Placement]) {
        let mut v = self.0.borrow_mut();
        for p in placements {
            // A later placement of the same view replaces one not yet applied.
            v.pending.retain(|q| q.id != p.id);
            v.pending.push(p.clone());
        }
    }
    fn destroy(&mut self, id: NativeId) {
        let mut v = self.0.borrow_mut();
        v.pending.retain(|q| q.id != id);
        v.destroy(id);
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// The shell's side: the views, the inbox, and the frame's transaction.
pub(crate) struct Native {
    views: Rc<RefCell<Views>>,
    pub(crate) inbox: Rc<Inbox>,
}

impl Native {
    /// The window's native views, and the host to give the app; `None` without an AppKit view.
    pub(crate) fn new(window: &Window, wake: impl Fn() + 'static) -> Option<(Self, Box<dyn NativeHost>)> {
        let mtm = MainThreadMarker::new()?;
        let RawWindowHandle::AppKit(handle) = window.window_handle().ok()?.as_raw() else { return None };
        // SAFETY: winit's content view, alive as long as the window.
        let parent: Retained<NSView> = unsafe { Retained::retain(handle.ns_view.as_ptr().cast()) }?;
        let inbox = Rc::new(Inbox { events: RefCell::default(), wake: Box::new(wake) });
        let views = Rc::new(RefCell::new(Views {
            mtm,
            parent,
            inbox: inbox.clone(),
            hosted: HashMap::new(),
            pending: Vec::new(),
            presenting: Cell::new(false),
        }));
        Some((Native { views: views.clone(), inbox }, Box::new(MacHost(views))))
    }

    /// Before the frame's surface texture is taken: opens the transaction the placements and
    /// the present go in, and presents inside it while views move or the window resizes.
    /// Returns whether a transaction is open (only while views exist).
    pub(crate) fn begin_frame(&self, surface: &wgpu::Surface<'_>, resized: bool) -> bool {
        let v = self.views.borrow();
        if v.hosted.is_empty() {
            return false;
        }
        let presenting = resized || v.moving();
        if presenting != v.presenting.get() {
            v.presenting.set(presenting);
            // SAFETY: the Metal surface's layer, set on the main thread between frames.
            if let Some(hal) = unsafe { surface.as_hal::<wgpu::hal::api::Metal>() } {
                hal.render_layer().lock().setPresentsWithTransaction(presenting);
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

    /// After the present: commits the transaction.
    pub(crate) fn end_frame() {
        CATransaction::commit();
    }
}

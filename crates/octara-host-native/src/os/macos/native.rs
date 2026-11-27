//! AppKit mounting, event delivery and snapshots for WebKit and MapKit islands.

use crate::os::video::Player;
use crate::os::webkit::Web;
use block2::RcBlock;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AllocAnyThread, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBitmapImageRep, NSDeviceRGBColorSpace, NSGraphicsContext, NSImage, NSView, NSVisualEffectView, NSWindowOrderingMode,
};
use objc2_foundation::{NSError, NSPoint, NSRect, NSSize};
use objc2_map_kit::{
    MKCoordinateRegion, MKCoordinateSpan, MKHybridMapConfiguration, MKImageryMapConfiguration, MKMapConfiguration, MKMapSnapshot,
    MKMapSnapshotOptions, MKMapSnapshotter, MKMapView, MKMapViewDelegate, MKStandardMapConfiguration,
};
use objc2_quartz_core::CAMetalLayer;
use objc2_web_kit::WKSnapshotConfiguration;
use octara_model::geometry::{Rect, Size};
use octara_model::identity::NativeId;
use octara_model::native::{NativeChange, NativeCommand, NativeEvent, NativePlacement, NativeSlot, NativeSnapshot, NativeSnapshotRequest};
use octara_model::paint::Picture;
use octara_model::window::VibrancyMaterial;
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

/// Events views sent, waiting for the event loop (delegates run on the main thread, inside
/// AppKit's dispatch; the shell hands them to the app when it wakes).
pub(super) struct Inbox {
    events: Cell<Vec<(NativeId, NativeEvent)>>,
    snapshots: Cell<Vec<SnapshotCompletion>>,
    wake: Box<dyn Fn()>,
}

type SnapshotCompletion = (NativeSnapshotRequest, Result<NativeSnapshot, String>);

impl Inbox {
    fn push(&self, id: NativeId, name: &str, data: Value) {
        self.push_event(id, NativeEvent { name: name.to_owned(), data });
    }

    fn push_event(&self, id: NativeId, event: NativeEvent) {
        let mut events = self.events.take();
        events.push((id, event));
        self.events.set(events);
        (self.wake)();
    }

    fn complete_snapshot(&self, completion: SnapshotCompletion) {
        let mut snapshots = self.snapshots.take();
        snapshots.push(completion);
        self.snapshots.set(snapshots);
        (self.wake)();
    }

    pub(super) fn take(&self) -> Vec<(NativeId, NativeEvent)> {
        self.events.take()
    }

    pub(super) fn take_snapshots(&self) -> Vec<SnapshotCompletion> {
        self.snapshots.take()
    }
}

/// An `NSImage`'s pixels, straight RGBA, at `scale` pixels per point.
pub(super) fn pixels(image: &NSImage, scale: f64) -> Option<(u32, u32, Vec<u8>)> {
    let size = image.size();
    let (w, h) = ((size.width * scale).round() as isize, (size.height * scale).round() as isize);
    if w <= 0 || h <= 0 {
        return None;
    }
    let (width, height) = (u32::try_from(w).ok()?, u32::try_from(h).ok()?);
    let bytes_per_row = w.checked_mul(4)?;
    let capacity = usize::try_from(bytes_per_row.checked_mul(h)?).ok()?;
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
            bytes_per_row,
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
    let data = rep.bitmapData();
    let stride = usize::try_from(rep.bytesPerRow()).ok()?;
    let row_len = bytes_per_row as usize;
    if data.is_null() || stride < row_len {
        return None;
    }
    let mut px = Vec::new();
    px.try_reserve_exact(capacity).ok()?;
    for y in 0..height as usize {
        // SAFETY: the rep owns this row's storage and its stride covers the requested bytes.
        let row = unsafe { std::slice::from_raw_parts(data.add(y * stride), row_len) };
        for p in row.as_chunks::<4>().0 {
            // Premultiplied in the bitmap; straight in an `Image`.
            let a = p[3] as u32;
            let un = |c: u8| ((c as u32 * 255 + a / 2).checked_div(a).unwrap_or(0)).min(255) as u8;
            px.extend([un(p[0]), un(p[1]), un(p[2]), p[3]]);
        }
    }
    Some((width, height, px))
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
        /// rest of it goes to what is below (Octara's content). A scroll goes to the view only
        /// while focus is in it (see [`takes_scroll`]).
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            let hit: Option<Retained<NSView>> = unsafe { msg_send![super(self), hitTest: point] };
            hit.filter(|v| !std::ptr::eq(&**v, self.as_super())).filter(|_| takes_scroll(self))
        }
    }
);

/// Whether the native view in `clip` takes the event being dispatched: a scroll (wheel or
/// trackpad) only while the window's focus is in it, so a page scrolls over an embedded web view
/// or map a person has not clicked into (as a page's embedded map does); every other event (a
/// press, a pinch) always. AppKit keeps a gesture's events with the view its start went to.
fn takes_scroll(clip: &NSView) -> bool {
    let Some(mtm) = MainThreadMarker::new() else { return true };
    let scrolling = objc2_app_kit::NSApplication::sharedApplication(mtm)
        .currentEvent()
        .is_some_and(|e| e.r#type() == objc2_app_kit::NSEventType::ScrollWheel);
    if !scrolling {
        return true;
    }
    let focus = clip.window().and_then(|w| w.firstResponder());
    focus.is_some_and(|r| {
        let any: &AnyObject = &r;
        any.downcast_ref::<NSView>().is_some_and(|v| v.isDescendantOf(clip))
    })
}

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
            if self.ivars().takes(point.x as f32, point.y as f32) {
                let hit: Option<Retained<NSView>> = unsafe { msg_send![super(self), hitTest: point] };
                hit.filter(|v| !std::ptr::eq(&**v, self.as_super()))
            } else {
                None
            }
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

impl DelegateIvars {
    fn send(&self, name: &str, data: Value) {
        self.inbox.push(self.id, name, data);
    }
}

struct MapIvars {
    events: DelegateIvars,
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
            self.ivars().events.send("region", region_json(unsafe { map.region() }));
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
        let this = Self::alloc(mtm).set_ivars(MapIvars { events: DelegateIvars { id, inbox }, loading: Cell::new(false) });
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

/// The platform view behind a kind.
enum Platform {
    Web(Web),
    Video(Player),
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
    slot: NativeSlot,
    placed: Option<NativePlacement>,
}

/// The views of one window, shared by the app's host (create, update, place, destroy) and the
/// shell (applying placements with the frame).
pub(super) struct Views {
    mtm: MainThreadMarker,
    supports_underlay: bool,
    parent: Retained<NSView>,
    inbox: Rc<Inbox>,
    hosted: HashMap<NativeId, Hosted>,
    /// Placements handed over by the last settle, applied with the next present.
    pending: Vec<NativePlacement>,
    /// The host of underlay views, below winit's view's subviews (made with the first one).
    underlay: Option<Retained<Underlay>>,
    vibrancy: Option<Retained<NSVisualEffectView>>,
    /// The window's Metal layer, kept above the underlay host.
    metal: Option<Retained<CAMetalLayer>>,
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> NSRect {
    NSRect::new(NSPoint::new(x as f64, y as f64), NSSize::new(w.max(0.0) as f64, h.max(0.0) as f64))
}

impl Views {
    fn create(&mut self, id: NativeId, kind: &str, slot: NativeSlot, props: &Value) {
        self.destroy(id);
        if !self.shows(kind, slot) {
            return;
        }
        let mtm = self.mtm;
        let (view, platform): (Retained<NSView>, Platform) = match (kind, slot) {
            ("web", _) => {
                let inbox = self.inbox.clone();
                let web = Web::new(mtm, props, move |event| inbox.push_event(id, event));
                (web.view().clone().into_super(), Platform::Web(web))
            }
            ("video", NativeSlot::Overlay) => {
                let Some(class) = AnyClass::get(c"AVPlayerView") else { return };
                // SAFETY: AVKit's AVPlayerView is an NSView subclass and takes an NSRect initializer.
                let view: Retained<NSView> = unsafe {
                    let allocated: Allocated<NSView> = msg_send![class, alloc];
                    msg_send![allocated, initWithFrame: NSRect::ZERO]
                };
                let mut player = Player::default();
                player.update(&view, props);
                (view, Platform::Video(player))
            }
            ("map", NativeSlot::Underlay) => {
                let map = unsafe { MKMapView::initWithFrame(MKMapView::alloc(mtm), NSRect::ZERO) };
                let delegate = MapDelegate::new(mtm, id, self.inbox.clone());
                unsafe { map.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
                let mut applied = Value::Null;
                apply_map(&map, props, &mut applied);
                (map.clone().into_super(), Platform::Map { map, delegate, applied })
            }
            _ => return,
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
    }

    fn update(&mut self, id: NativeId, props: &Value) {
        let Some(h) = self.hosted.get_mut(&id) else { return };
        match &mut h.platform {
            Platform::Web(web) => web.update(props),
            Platform::Video(player) => player.update(&h.view, props),
            Platform::Map { map, applied, .. } => apply_map(map, props, applied),
        }
    }

    fn destroy(&mut self, id: NativeId) {
        let Some(h) = self.hosted.remove(&id) else { return };
        if let Platform::Map { map, .. } = &h.platform {
            // SAFETY: Clear the non-owning delegate before releasing it.
            unsafe { map.setDelegate(None) };
        }
        h.clip.removeFromSuperview();
        self.update_regions();
    }

    pub(super) fn command(&self, command: NativeCommand) {
        if let Some(hosted) = self.hosted.get(&command.id) {
            match &hosted.platform {
                Platform::Web(web) => web.command(&command.name, &command.data),
                Platform::Video(player) => player.command(&command.name),
                Platform::Map { .. } => {}
            }
        }
    }

    /// Requests a snapshot and echoes the request when the platform answers.
    pub(super) fn snapshot(&self, request: NativeSnapshotRequest) -> Result<(), String> {
        let view = self.hosted.get(&request.id).ok_or("native view is unavailable")?;
        let placed = view.placed.as_ref().ok_or("native view has not been placed")?;
        if self.pending.iter().any(|placement| placement.id == request.id && placement != placed) {
            return Err("native placement has not been committed".into());
        }
        let scale = self.parent.window().map_or(1.0, |window| window.backingScaleFactor());
        let inbox = self.inbox.clone();
        let misses = match view.platform {
            Platform::Web(_) => vec!["WebKit snapshots omit hardware-decoded video and WebGL".into()],
            Platform::Map { .. } => vec!["MapKit snapshots omit annotations and overlays".into()],
            Platform::Video(_) => return Err("AVKit video cannot be captured in an Octara snapshot".into()),
        };
        let send = move |image: Option<&NSImage>| {
            let snapshot = image
                .and_then(|image| pixels(image, scale))
                .and_then(|(width, height, pixels)| Picture::new(width, height, pixels))
                .map(|picture| NativeSnapshot { picture, misses: misses.clone() })
                .ok_or_else(|| "native view snapshot failed".into());
            inbox.complete_snapshot((request, snapshot));
        };
        match &view.platform {
            Platform::Web(web) => {
                let config = unsafe { WKSnapshotConfiguration::new(self.mtm) };
                unsafe { config.setAfterScreenUpdates(false) };
                let done = RcBlock::new(move |image: *mut NSImage, _error: *mut NSError| {
                    // SAFETY: WebKit passes an image or null during this callback.
                    send(unsafe { image.as_ref() });
                });
                unsafe { web.view().takeSnapshotWithConfiguration_completionHandler(Some(&config), &done) };
            }
            Platform::Video(_) => unreachable!("video snapshot already rejected"),
            Platform::Map { map, .. } => {
                let options = unsafe { MKMapSnapshotOptions::new() };
                unsafe {
                    options.setRegion(map.region());
                    options.setPreferredConfiguration(&map.preferredConfiguration());
                    options.setSize(map.bounds().size);
                }
                let snapshotter = unsafe { MKMapSnapshotter::initWithOptions(MKMapSnapshotter::alloc(), &options) };
                let keep = Cell::new(Some(snapshotter.clone()));
                let done = RcBlock::new(move |shot: *mut MKMapSnapshot, _error: *mut NSError| {
                    // SAFETY: MapKit passes a snapshot or null on the main thread.
                    let image = unsafe { shot.as_ref() }.map(|shot| unsafe { shot.image() });
                    keep.take();
                    send(image.as_deref());
                });
                unsafe { snapshotter.startWithCompletionHandler(RcBlock::as_ptr(&done)) };
            }
        }
        Ok(())
    }

    /// A page or map tiles still loading.
    pub(super) fn busy(&self, id: NativeId) -> Option<String> {
        let h = self.hosted.get(&id)?;
        match &h.platform {
            Platform::Web(web) => web.busy(),
            Platform::Video(_) => None,
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
        if let Some(effect) = &self.vibrancy {
            self.parent.addSubview_positioned_relativeTo(&host, NSWindowOrderingMode::Above, Some(effect));
        } else {
            self.parent.addSubview_positioned_relativeTo(&host, NSWindowOrderingMode::Below, None);
        }
        if let (Some(root), Some(host_layer), Some(metal)) = (self.parent.layer(), host.layer(), self.metal.as_ref()) {
            let inside = unsafe { root.sublayers() }.is_some_and(|ls| ls.iter().any(|l| std::ptr::eq(&*l, &***metal)));
            if !inside && !std::ptr::eq(&*root, &***metal) {
                root.insertSublayer_above(metal, Some(&host_layer));
            }
            // AppKit re-sorts sublayers to its subviews' order when it updates the layer tree;
            // siblings composite by z position first.
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
            .filter(|h| h.slot == NativeSlot::Underlay)
            .filter_map(|h| h.placed.as_ref())
            .filter(|p| p.visible)
            .map(|p| (p.rect.intersect(&p.clip), p.covered.clone()))
            .collect();
        *host.ivars().0.borrow_mut() = regions;
    }

    /// Whether the Metal layer must be non-opaque (an underlay view shows through it).
    pub(super) fn has_underlay(&self) -> bool {
        self.hosted.values().any(|h| h.slot == NativeSlot::Underlay)
    }

    /// Applies the waiting placements (inside the frame's transaction).
    fn apply(&mut self) {
        let flipped = self.parent.isFlipped();
        let height = self.parent.bounds().size.height as f32;
        let pending = std::mem::take(&mut self.pending);
        let underlay = pending.iter().any(|p| self.hosted.get(&p.id).is_some_and(|h| h.slot == NativeSlot::Underlay && h.placed.is_none()));
        let host = underlay.then(|| self.underlay_host());
        for p in pending {
            let Some(h) = self.hosted.get_mut(&p.id) else { continue };
            if h.placed.is_none() {
                match h.slot {
                    NativeSlot::Overlay => {
                        self.parent.addSubview(&h.clip);
                        // Above the Metal layer, which sits at z 1 once an underlay exists.
                        if let Some(layer) = h.clip.layer() {
                            layer.setZPosition(2.0);
                        }
                    }
                    NativeSlot::Underlay => host.as_ref().expect("the underlay host").addSubview(&h.clip),
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
                crate::os::corners::apply(&layer, p.radius, h.view.isFlipped());
            }
            h.clip.setAlphaValue(p.opacity as f64);
            h.clip.setHidden(!p.visible);
            if !p.visible
                && let Platform::Video(player) = &h.platform
            {
                player.pause();
            }
            h.placed = Some(p);
        }
        self.update_regions();
    }
}

impl Views {
    pub(super) fn new(parent: Retained<NSView>, mtm: MainThreadMarker, supports_underlay: bool, wake: impl Fn() + 'static) -> Self {
        let inbox = Rc::new(Inbox { events: Cell::default(), snapshots: Cell::default(), wake: Box::new(wake) });
        Self {
            mtm,
            supports_underlay,
            parent,
            inbox,
            hosted: HashMap::new(),
            pending: Vec::new(),
            underlay: None,
            vibrancy: None,
            metal: None,
        }
    }

    pub(super) fn set_vibrancy(&mut self, material: Option<VibrancyMaterial>) {
        let Some(material) = material.map(super::chrome::visual_effect_material) else {
            if let Some(effect) = self.vibrancy.take() {
                effect.removeFromSuperview();
            }
            return;
        };
        if let Some(effect) = &self.vibrancy {
            if effect.material() != material {
                effect.setMaterial(material);
            }
            return;
        }
        let effect = super::chrome::visual_effect(&self.parent, material);
        self.parent.addSubview_positioned_relativeTo(&effect, NSWindowOrderingMode::Below, None);
        self.vibrancy = Some(effect);
        self.order_metal_above_backing();
    }

    pub(super) fn has_vibrancy(&self) -> bool {
        self.vibrancy.is_some()
    }

    pub(super) fn set_layer(&mut self, metal: Option<&CAMetalLayer>) {
        if match (self.metal.as_deref(), metal) {
            (Some(current), Some(next)) => std::ptr::eq(current, next),
            (None, None) => true,
            _ => false,
        } {
            return;
        }
        self.metal = metal.map(objc2::Message::retain);
        self.order_metal_above_backing();
    }

    fn order_metal_above_backing(&self) {
        let backing = self.underlay.as_deref().map(|host| host.as_super()).or(self.vibrancy.as_deref().map(|effect| effect.as_super()));
        if let (Some(layer), Some(backing)) = (&self.metal, backing)
            && let (Some(root), Some(host_layer)) = (self.parent.layer(), backing.layer())
            && !std::ptr::eq(&*root, &***layer)
        {
            root.insertSublayer_above(layer, Some(&host_layer));
            layer.setZPosition(1.0);
        }
    }

    pub(super) fn sync(&mut self, changes: &[NativeChange]) {
        for change in changes {
            match change {
                NativeChange::Create { id, kind, slot, props } => self.create(*id, kind, *slot, props),
                NativeChange::Update { id, props } => self.update(*id, props),
                NativeChange::Place { placements } => {
                    for placement in placements {
                        self.pending.retain(|current| current.id != placement.id);
                        self.pending.push(placement.clone());
                    }
                }
                NativeChange::Destroy { id } => {
                    self.pending.retain(|placement| placement.id != *id);
                    self.destroy(*id);
                }
            }
        }
    }

    pub(super) fn commit(&mut self) {
        if !self.pending.is_empty() {
            self.apply();
        }
    }

    pub(super) fn shows(&self, kind: &str, slot: NativeSlot) -> bool {
        match (kind, slot) {
            ("web", NativeSlot::Overlay) => true,
            ("video", NativeSlot::Overlay) => AnyClass::get(c"AVPlayerView").is_some(),
            ("web", NativeSlot::Underlay) => self.supports_underlay,
            ("map", NativeSlot::Underlay) => self.supports_underlay && objc2::available!(macos = 13.0),
            _ => false,
        }
    }

    pub(super) fn measure(&self, id: NativeId) -> Option<Size> {
        let size = self.hosted.get(&id)?.view.intrinsicContentSize();
        (size.width.is_finite() && size.height.is_finite() && size.width > 0.0 && size.height > 0.0)
            .then_some(Size { width: size.width as f32, height: size.height as f32 })
    }

    pub(super) fn inbox(&self) -> &Inbox {
        &self.inbox
    }

    pub(super) fn inspect(&self) -> Value {
        let mut frames = self
            .hosted
            .iter()
            .map(|(id, view)| {
                let frame = view.view.convertRect_toView(view.view.bounds(), Some(&self.parent));
                (id.0, json!({ "id": id.0, "frame": [frame.origin.x, frame.origin.y, frame.size.width, frame.size.height] }))
            })
            .collect::<Vec<_>>();
        frames.sort_unstable_by_key(|(id, _)| *id);
        Value::Array(frames.into_iter().map(|(_, value)| value).collect())
    }
}

impl Drop for Views {
    fn drop(&mut self) {
        for id in self.hosted.keys().copied().collect::<Vec<_>>() {
            self.destroy(id);
        }
        if let Some(underlay) = &self.underlay {
            underlay.removeFromSuperview();
        }
        if let Some(effect) = &self.vibrancy {
            effect.removeFromSuperview();
        }
    }
}

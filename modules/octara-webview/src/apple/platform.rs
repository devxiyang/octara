use super::{WKWebView, Web};
use crate::protocol::RpcCall;
use crate::{WebViewCommand, WebViewProps, unavailable_snapshot};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::{ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_foundation::{NSError, NSPoint, NSRect, NSSize};
use objc2_web_kit::WKSnapshotConfiguration;
use octara::Picture;
use octara::Rect;
use octara::module::{
    CallTicket, HostSurface, ModuleDispatcher, ModuleError, NativePlacement, NativeSlot, NativeSnapshot, NativeSnapshotRequest,
    NativeViewEmitter, NativeViewInstance, RawNativeSurface,
};
use std::any::Any;
use std::cell::RefCell;

#[path = "corners.rs"]
mod corners;
#[cfg(target_os = "macos")]
use objc2::AllocAnyThread;
#[cfg(target_os = "macos")]
use objc2::runtime::AnyObject;
#[cfg(target_os = "macos")]
use objc2_app_kit::{
    NSApplication, NSBitmapImageRep, NSDeviceRGBColorSpace, NSEventType, NSGraphicsContext, NSImage as SnapshotImage, NSView,
    NSWindowOrderingMode,
};
#[cfg(target_os = "ios")]
use objc2_core_graphics::{CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo};
#[cfg(target_os = "ios")]
use objc2_ui_kit::{UIEvent, UIImage as SnapshotImage, UIView};

pub(crate) fn create(
    surface: HostSurface<'_>,
    slot: NativeSlot,
    props: WebViewProps,
    emitter: NativeViewEmitter,
) -> Result<Box<dyn NativeViewInstance>, ModuleError> {
    let mtm = MainThreadMarker::new().ok_or(ModuleError::Unavailable)?;
    #[cfg(target_os = "macos")]
    let Some(RawNativeSurface::AppKit { ns_view: parent, .. }) = surface.raw_native() else { return Err(ModuleError::Unavailable) };
    #[cfg(target_os = "ios")]
    let Some(RawNativeSurface::UiKit { ui_view: parent, .. }) = surface.raw_native() else { return Err(ModuleError::Unavailable) };

    // SAFETY: The host vends a live view of the indicated platform for this call, on the main thread.
    let parent: &PlatformView = unsafe { parent.cast().as_ref() };
    let web = Web::new(mtm, props, emitter.clone());
    let clip = Clip::new(mtm, parent, &web.view, slot);
    Ok(Box::new(Instance { web, clip, emitter, hidden: false }))
}

#[cfg(target_os = "macos")]
type PlatformView = NSView;
#[cfg(target_os = "ios")]
type PlatformView = UIView;

#[cfg(target_os = "macos")]
define_class!(
    // SAFETY: AppKit permits subclassing NSView on the main thread.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraModuleAppKitWebViewClip"]
    #[ivars = RefCell<Option<NativePlacement>>]
    struct Clip;

    impl Clip {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            if self.accepts_hit(point) {
                // SAFETY: Forward AppKit's hit test and pass hits outside the web view through.
                let hit: Option<Retained<NSView>> = unsafe { msg_send![super(self), hitTest: point] };
                hit.filter(|view| !std::ptr::eq(&**view, self.as_super()))
            } else {
                None
            }
        }
    }
);

#[cfg(target_os = "ios")]
define_class!(
    // SAFETY: UIKit permits subclassing UIView on the main thread.
    #[unsafe(super(UIView))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraModuleUIKitWebViewClip"]
    #[ivars = RefCell<Option<NativePlacement>>]
    struct Clip;

    impl Clip {
        #[unsafe(method_id(hitTest:withEvent:))]
        fn hit_test(&self, point: NSPoint, event: Option<&UIEvent>) -> Option<Retained<UIView>> {
            // SAFETY: Forward UIKit's hit test to UIView and pass hits outside the web view through.
            let hit: Option<Retained<UIView>> = unsafe { msg_send![super(self), hitTest: point, withEvent: event] };
            hit.filter(|view| !std::ptr::eq(&**view, self.as_super()))
        }
    }
);

#[cfg(target_os = "macos")]
impl Clip {
    fn new(mtm: MainThreadMarker, parent: &NSView, web: &WKWebView, slot: NativeSlot) -> Retained<Self> {
        let clip = Self::alloc(mtm).set_ivars(RefCell::default());
        // SAFETY: AppKit is initialized and this is its main thread.
        let clip: Retained<Self> = unsafe { msg_send![super(clip), initWithFrame: NSRect::ZERO] };
        clip.setHidden(true);
        clip.setWantsLayer(true);
        web.setWantsLayer(true);
        if let Some(layer) = web.layer() {
            layer.setMasksToBounds(true);
        }
        let underlay = slot == NativeSlot::Underlay;
        if let Some(layer) = clip.layer() {
            layer.setMasksToBounds(true);
            layer.setZPosition(if underlay { 0.0 } else { 2.0 });
        }
        clip.addSubview(web);
        if underlay {
            parent.addSubview_positioned_relativeTo(&clip, NSWindowOrderingMode::Below, None);
        } else {
            parent.addSubview(&clip);
        }
        clip
    }

    fn accepts_hit(&self, point: NSPoint) -> bool {
        // SAFETY: AppKit calls hit testing on the UI thread with a stable view hierarchy.
        let Some(parent) = (unsafe { self.superview() }) else { return false };
        let p = self.ivars().borrow();
        let Some(p) = p.as_ref() else { return false };
        let x = point.x as f32;
        let y = if parent.isFlipped() { point.y } else { parent.bounds().size.height - point.y } as f32;
        if p.slot == NativeSlot::Underlay && p.covered.iter().any(|r| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h) {
            return false;
        }
        !NSApplication::sharedApplication(self.mtm()).currentEvent().is_some_and(|e| e.r#type() == NSEventType::ScrollWheel)
            || self.window().and_then(|w| w.firstResponder()).is_some_and(|r| {
                let any: &AnyObject = &r;
                any.downcast_ref::<NSView>().is_some_and(|view| view.isDescendantOf(self))
            })
    }

    fn place_view(&self, web: &WKWebView, placement: &NativePlacement, hidden: bool) {
        let clip = placement.clip;
        let r = placement.rect;
        // SAFETY: Placement runs on the main thread in the host-owned view hierarchy.
        let parent = unsafe { self.superview() };
        let y = parent.filter(|parent| !parent.isFlipped()).map_or(clip.y, |p| p.bounds().size.height as f32 - clip.y - clip.h);
        self.setFrame(rect(Rect::new(clip.x, y, clip.w, clip.h)));
        if let Some(layer) = self.layer() {
            layer.setCornerRadius(f64::from(placement.clip_radius));
        }
        web.setFrame(rect(Rect::new(r.x - clip.x, r.y - clip.y, r.w, r.h)));
        if let Some(layer) = web.layer() {
            corners::apply(&layer, placement.radius, web.isFlipped());
        }
        self.setAlphaValue(f64::from(placement.opacity));
        self.setHidden(hidden || !placement.visible);
    }
}

#[cfg(target_os = "ios")]
impl Clip {
    fn new(mtm: MainThreadMarker, parent: &UIView, web: &WKWebView, _slot: NativeSlot) -> Retained<Self> {
        let clip = Self::alloc(mtm).set_ivars(RefCell::default());
        // SAFETY: UIKit is initialized and this is its main thread.
        let clip: Retained<Self> = unsafe { msg_send![super(clip), initWithFrame: NSRect::ZERO] };
        clip.setClipsToBounds(true);
        clip.setHidden(true);
        web.setClipsToBounds(true);
        clip.addSubview(web);
        parent.addSubview(&clip);
        clip
    }

    fn place_view(&self, web: &WKWebView, placement: &NativePlacement, hidden: bool) {
        let clip = placement.clip;
        let r = placement.rect;
        self.setFrame(rect(clip));
        self.layer().setCornerRadius(f64::from(placement.clip_radius));
        web.setFrame(rect(Rect::new(r.x - clip.x, r.y - clip.y, r.w, r.h)));
        corners::apply(&web.layer(), placement.radius, true);
        self.setAlpha(f64::from(placement.opacity));
        self.setHidden(hidden || !placement.visible);
    }
}

fn rect(r: Rect) -> NSRect {
    NSRect::new(NSPoint::new(f64::from(r.x), f64::from(r.y)), NSSize::new(f64::from(r.w.max(0.0)), f64::from(r.h.max(0.0))))
}

struct Instance {
    web: Web,
    clip: Retained<Clip>,
    emitter: NativeViewEmitter,
    hidden: bool,
}

impl NativeViewInstance for Instance {
    fn update(&mut self, props: Box<dyn Any>) {
        if let Ok(props) = props.downcast::<WebViewProps>() {
            self.web.update(*props);
        }
    }

    fn place(&mut self, placement: &NativePlacement) {
        self.clip.place_view(&self.web.view, placement, self.hidden);
        self.clip.ivars().replace(Some(placement.clone()));
    }

    fn call(&mut self, _ticket: CallTicket, command: Box<dyn Any>, _reply: ModuleDispatcher) {
        match command.downcast::<WebViewCommand>() {
            Ok(command) => self.web.command(*command),
            Err(command) => {
                if let Ok(call) = command.downcast::<RpcCall>() {
                    self.web.rpc(*call);
                }
            }
        }
    }

    fn snapshot(&mut self, request: NativeSnapshotRequest, _reply: ModuleDispatcher) {
        if self.clip.ivars().borrow().is_none() {
            unavailable_snapshot(&self.emitter, request, "The web view has no committed placement".into());
            return;
        }

        #[cfg(target_os = "macos")]
        let scale = self.clip.window().map_or(1.0, |window| window.backingScaleFactor());
        #[cfg(target_os = "ios")]
        let scale = 1.0;
        let emitter = self.emitter.clone();
        let done = RcBlock::new(move |image: *mut SnapshotImage, error: *mut NSError| {
            // SAFETY: WebKit passes an image or error valid for the duration of this callback.
            let (image, error) = unsafe { (image.as_ref(), error.as_ref()) };
            if let Some(picture) = image.and_then(|image| picture(image, scale)) {
                let snapshot = NativeSnapshot { picture, misses: vec!["WebKit snapshots omit hardware-decoded video and WebGL".into()] };
                let _ = emitter.finish_snapshot(request, snapshot);
            } else {
                let reason = error.map_or_else(|| "WebKit snapshot failed".into(), |error| error.localizedDescription().to_string());
                unavailable_snapshot(&emitter, request, reason);
            }
        });
        // SAFETY: All WebKit view and snapshot operations run on the main thread.
        unsafe {
            let config = WKSnapshotConfiguration::new(self.clip.mtm());
            config.setAfterScreenUpdates(false);
            self.web.view.takeSnapshotWithConfiguration_completionHandler(Some(&config), &done);
        }
    }

    fn busy_reason(&self) -> Option<String> {
        self.web.busy()
    }

    fn set_occluded_by_snapshot(&mut self, hidden: bool) {
        self.hidden = hidden;
        self.clip.setHidden(hidden || self.clip.ivars().borrow().as_ref().is_none_or(|placement| !placement.visible));
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        self.clip.removeFromSuperview();
    }
}

/// An `NSImage`'s pixels, straight RGBA, at `scale` pixels per point.
#[cfg(target_os = "macos")]
fn picture(image: &SnapshotImage, scale: f64) -> Option<Picture> {
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
    Picture::new(width, height, px)
}

#[cfg(target_os = "ios")]
fn picture(image: &SnapshotImage, _scale: f64) -> Option<Picture> {
    // SAFETY: The snapshot image is read on the main thread.
    let image = unsafe { image.CGImage() }?;
    let (width, height) = (CGImage::width(Some(&image)), CGImage::height(Some(&image)));
    let (picture_width, picture_height) = (u32::try_from(width).ok()?, u32::try_from(height).ok()?);
    let stride = width.checked_mul(4)?;
    let len = stride.checked_mul(height)?;
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(len).ok()?;
    pixels.resize(len, 0);
    let space = CGColorSpace::new_device_rgb()?;
    let bitmap_info = CGImageByteOrderInfo::Order32Big.0 | CGImageAlphaInfo::PremultipliedLast.0;
    // SAFETY: The buffer has exactly height * stride writable bytes and outlives this context.
    let context = unsafe { CGBitmapContextCreate(pixels.as_mut_ptr().cast(), width, height, 8, stride, Some(&space), bitmap_info) }?;
    CGContext::draw_image(Some(&context), NSRect::new(NSPoint::ZERO, NSSize::new(width as f64, height as f64)), Some(&image));
    drop(context);
    for pixel in pixels.as_chunks_mut::<4>().0 {
        let alpha = u32::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = ((u32::from(*channel) * 255 + alpha / 2).checked_div(alpha).unwrap_or_default()).min(255) as u8;
        }
    }
    Picture::new(picture_width, picture_height, pixels)
}

use super::{apply_corners, rect, video_frame};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{ClassType, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSApplication, NSEventType, NSView};
use objc2_foundation::{NSPoint, NSRect};
use octara::module::{HostSurface, ModuleError, NativePlacement, RawNativeSurface};

define_class!(
    // SAFETY: NSView has no subclassing requirements. This subclass lives on the main thread.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraAVKitAppKitClip"]
    struct Clip;

    impl Clip {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            // SAFETY: Forward AppKit's original arguments to NSView on the main thread.
            let hit: Option<Retained<NSView>> = unsafe { msg_send![super(self), hitTest: point] };
            hit.filter(|view| !std::ptr::eq(&**view, self.as_super()))
                .filter(|_| takes_scroll(self))
        }
    }
);

fn takes_scroll(clip: &NSView) -> bool {
    let app = NSApplication::sharedApplication(MainThreadMarker::from(clip));
    // Let wheel scrolling reach Octara until the player receives focus.
    if app.currentEvent().is_none_or(|event| event.r#type() != NSEventType::ScrollWheel) {
        return true;
    }
    clip.window().and_then(|window| window.firstResponder()).is_some_and(|focus| {
        let focus: &AnyObject = &focus;
        focus.downcast_ref::<NSView>().is_some_and(|view| view.isDescendantOf(clip))
    })
}

pub(super) struct PlatformView {
    parent: Retained<NSView>,
    clip: Retained<Clip>,
    view: Retained<NSView>,
}

impl PlatformView {
    pub(super) fn new(surface: HostSurface<'_>, mtm: MainThreadMarker) -> Result<Self, ModuleError> {
        let Some(RawNativeSurface::AppKit { ns_view, .. }) = surface.raw_native() else { return Err(ModuleError::Unavailable) };
        // SAFETY: The AppKit host lends the live parent NSView on the main thread.
        let parent = unsafe { Retained::<NSView>::retain(ns_view.cast().as_ptr()) }.ok_or(ModuleError::Unavailable)?;
        let class = AnyClass::get(c"AVPlayerView").ok_or(ModuleError::Unavailable)?;
        // SAFETY: AVPlayerView is an NSView subclass. We initialize both views on the main thread.
        let (view, clip): (Retained<NSView>, Retained<Clip>) = unsafe {
            let allocated: Allocated<NSView> = msg_send![class, alloc];
            (msg_send![allocated, initWithFrame: NSRect::ZERO], msg_send![Clip::alloc(mtm), initWithFrame: NSRect::ZERO])
        };
        view.setWantsLayer(true);
        if let Some(layer) = view.layer() {
            layer.setMasksToBounds(true);
        }
        clip.setWantsLayer(true);
        clip.addSubview(&view);
        clip.setHidden(true);
        parent.addSubview(&clip);
        if let Some(layer) = clip.layer() {
            layer.setMasksToBounds(true);
            layer.setZPosition(2.0);
        }
        Ok(Self { parent, clip, view })
    }

    pub(super) fn player_view(&self) -> &AnyObject {
        &self.view
    }

    pub(super) fn place(&self, placement: &NativePlacement) {
        let mut clip = placement.clip;
        if !self.parent.isFlipped() {
            clip.y = self.parent.bounds().size.height as f32 - clip.y - clip.h;
        }
        self.clip.setFrame(rect(clip));
        if let Some(layer) = self.clip.layer() {
            layer.setCornerRadius(placement.clip_radius.into());
        }
        self.view.setFrame(video_frame(placement));
        if let Some(layer) = self.view.layer() {
            apply_corners(&layer, placement.radius, self.view.isFlipped());
        }
        self.clip.setAlphaValue(placement.opacity.into());
        self.clip.setHidden(!placement.visible);
    }
}

impl Drop for PlatformView {
    fn drop(&mut self) {
        self.clip.removeFromSuperview();
    }
}

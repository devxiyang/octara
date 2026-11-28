use super::{apply_corners, rect, video_frame};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{ClassType, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_foundation::{NSPoint, NSRect};
use objc2_ui_kit::{UIEvent, UIView, UIViewController};
use octara::module::{HostSurface, ModuleError, NativePlacement, RawNativeSurface};

define_class!(
    // SAFETY: UIView permits subclassing on the main thread.
    #[unsafe(super(UIView))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraAVKitUIKitClip"]
    struct Clip;

    impl Clip {
        #[unsafe(method_id(hitTest:withEvent:))]
        fn hit_test(&self, point: NSPoint, event: Option<&UIEvent>) -> Option<Retained<UIView>> {
            // SAFETY: Forward UIKit's original arguments to UIView on the main thread.
            let hit: Option<Retained<UIView>> = unsafe { msg_send![super(self), hitTest: point, withEvent: event] };
            hit.filter(|view| !std::ptr::eq(&**view, self.as_super()))
        }
    }
);

pub(super) struct PlatformView {
    controller: Retained<UIViewController>,
    clip: Retained<Clip>,
    view: Retained<UIView>,
}

impl PlatformView {
    pub(super) fn new(surface: HostSurface<'_>, mtm: MainThreadMarker) -> Result<Self, ModuleError> {
        let Some(RawNativeSurface::UiKit { ui_view, ui_view_controller }) = surface.raw_native() else {
            return Err(ModuleError::Unavailable);
        };
        // SAFETY: The UIKit host lends a live UIView and UIViewController on the main thread.
        let parent = unsafe { ui_view.cast::<UIView>().as_ref() };
        let parent_controller = unsafe { ui_view_controller.cast::<UIViewController>().as_ref() };
        let class = AnyClass::get(c"AVPlayerViewController").ok_or(ModuleError::Unavailable)?;
        // SAFETY: This subclass of UIViewController is initialized on the main thread.
        let controller: Retained<UIViewController> = unsafe {
            let allocated: Allocated<UIViewController> = msg_send![class, alloc];
            msg_send![allocated, init]
        };
        let view = controller.view().ok_or(ModuleError::Unavailable)?;
        // SAFETY: UIView's designated initializer is valid for the main-thread Clip subclass.
        let clip: Retained<Clip> = unsafe { msg_send![Clip::alloc(mtm), initWithFrame: NSRect::ZERO] };
        clip.setClipsToBounds(true);
        clip.setHidden(true);
        view.setClipsToBounds(true);
        parent_controller.addChildViewController(&controller);
        clip.addSubview(&view);
        parent.addSubview(&clip);
        controller.didMoveToParentViewController(Some(parent_controller));
        Ok(Self { controller, clip, view })
    }

    pub(super) fn player_view(&self) -> &AnyObject {
        &self.controller
    }

    pub(super) fn place(&self, placement: &NativePlacement) {
        self.clip.setFrame(rect(placement.clip));
        self.clip.layer().setCornerRadius(placement.clip_radius.into());
        self.view.setFrame(video_frame(placement));
        apply_corners(&self.view.layer(), placement.radius, true);
        self.clip.setAlpha(placement.opacity.into());
    }

    pub(super) fn set_hidden(&self, hidden: bool) {
        self.clip.setHidden(hidden);
    }
}

impl Drop for PlatformView {
    fn drop(&mut self) {
        self.controller.willMoveToParentViewController(None);
        self.view.removeFromSuperview();
        self.controller.removeFromParentViewController();
        self.clip.removeFromSuperview();
    }
}

//! UIKit placement and capture for WebKit islands.

use crate::ShellEvent;
use crate::os::video::Player;
use crate::os::webkit::Web;
use crate::os::{WindowHostEvent, WindowNativeViews, reply};
use block2::RcBlock;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::AnyClass;
use objc2::{ClassType, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_core_graphics::{CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo};
use objc2_foundation::{NSError, NSPoint, NSRect, NSSize};
use objc2_ui_kit::{UIEvent, UIImage, UIView, UIViewController};
use objc2_web_kit::WKSnapshotConfiguration;
use octara::runtime::WindowId;
use octara_model::geometry::{Rect, Size};
use octara_model::identity::NativeId;
use octara_model::input::HostReply;
use octara_model::native::{
    NativeChange, NativeCommand, NativeMeasureRequest, NativePlacement, NativeSlot, NativeSnapshot, NativeSnapshotRequest,
};
use octara_model::paint::Picture;
use serde_json::{Value, json};
use std::collections::HashMap;
use winit::event_loop::EventLoopProxy;

define_class!(
    // SAFETY: UIView permits subclassing on the main thread.
    #[unsafe(super(UIView))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraUIKitNativeClip"]
    struct Clip;

    impl Clip {
        #[unsafe(method_id(hitTest:withEvent:))]
        fn hit_test(&self, point: NSPoint, event: Option<&UIEvent>) -> Option<Retained<UIView>> {
            // SAFETY: Forward UIKit's original arguments to UIView, then pass hits outside the island through.
            let hit: Option<Retained<UIView>> = unsafe { msg_send![super(self), hitTest: point, withEvent: event] };
            hit.filter(|view| !std::ptr::eq(&**view, self.as_super()))
        }
    }
);

enum Platform {
    Web(Web),
    Video { controller: Retained<UIViewController>, player: Player },
}

struct Hosted {
    clip: Retained<Clip>,
    view: Retained<UIView>,
    platform: Platform,
    placed: bool,
}

impl Hosted {
    fn place(&mut self, placement: &NativePlacement) {
        let c = placement.clip;
        let r = placement.rect;
        self.clip.setFrame(rect(c));
        self.clip.layer().setCornerRadius(f64::from(placement.clip_radius));
        self.clip.setAlpha(f64::from(placement.opacity));
        self.clip.setHidden(!placement.visible);
        self.view.setFrame(rect(Rect::new(r.x - c.x, r.y - c.y, r.w, r.h)));
        crate::os::corners::apply(&self.view.layer(), placement.radius, true);
        if !placement.visible
            && let Platform::Video { player, .. } = &self.platform
        {
            player.pause();
        }
        self.placed = true;
    }
}

impl Drop for Hosted {
    fn drop(&mut self) {
        if let Platform::Video { controller, .. } = &self.platform {
            controller.willMoveToParentViewController(None);
            self.view.removeFromSuperview();
            controller.removeFromParentViewController();
        }
        self.clip.removeFromSuperview();
    }
}

pub(super) struct Views {
    parent: Retained<UIView>,
    hosted: HashMap<NativeId, Hosted>,
    window: WindowId,
    proxy: EventLoopProxy<ShellEvent>,
}

impl Views {
    pub(super) fn new(parent: Retained<UIView>, window: WindowId, proxy: EventLoopProxy<ShellEvent>) -> Self {
        Self { parent, hosted: HashMap::new(), window, proxy }
    }

    fn create(&mut self, id: NativeId, kind: &str, slot: NativeSlot, props: &Value) {
        self.hosted.remove(&id);
        if !self.shows_native(kind, slot) {
            return;
        }
        let mtm = MainThreadMarker::from(&*self.parent);
        let parent = self.parent.window().and_then(|window| window.rootViewController());
        let (view, platform) = if kind == "video" {
            let Some(class) = AnyClass::get(c"AVPlayerViewController") else { return };
            // SAFETY: AVKit's view controller subclasses UIViewController and is created on the main thread.
            let controller: Retained<UIViewController> = unsafe {
                let allocated: Allocated<UIViewController> = msg_send![class, alloc];
                msg_send![allocated, init]
            };
            let Some(view) = controller.view() else { return };
            let mut player = Player::default();
            player.update(&controller, props);
            if let Some(parent) = parent.as_ref() {
                parent.addChildViewController(&controller);
            }
            (view, Platform::Video { controller, player })
        } else {
            let proxy = self.proxy.clone();
            let window = self.window;
            let web = Web::new(mtm, props, move |event| {
                let _ = proxy.send_event(ShellEvent::Native { window, id, event });
            });
            (web.view().clone().into_super(), Platform::Web(web))
        };
        // SAFETY: Allocate and initialize the UIView subclass on the main thread.
        let clip: Retained<Clip> = unsafe { msg_send![Clip::alloc(mtm), initWithFrame: NSRect::ZERO] };
        clip.setClipsToBounds(true);
        clip.setHidden(true);
        view.setClipsToBounds(true);
        clip.addSubview(&view);
        self.parent.addSubview(&clip);
        if let (Platform::Video { controller, .. }, Some(parent)) = (&platform, &parent) {
            controller.didMoveToParentViewController(Some(parent));
        }
        self.hosted.insert(id, Hosted { clip, view, platform, placed: false });
    }

    fn snapshot(&self, request: NativeSnapshotRequest) -> Result<(), String> {
        let view = self.hosted.get(&request.id).ok_or("native view is unavailable")?;
        if !view.placed {
            return Err("native view has not been placed".into());
        }
        let Platform::Web(web) = &view.platform else { return Err("AVKit video cannot be captured in an Octara snapshot".into()) };
        let proxy = self.proxy.clone();
        let window = self.window;
        let done = RcBlock::new(move |image: *mut UIImage, error: *mut NSError| {
            // SAFETY: WebKit passes an image or error valid for the duration of this callback.
            let result = unsafe { image.as_ref() }.and_then(picture).map_or_else(
                || {
                    Err(unsafe { error.as_ref() }
                        .map_or_else(|| "native view snapshot failed".into(), |error| error.localizedDescription().to_string()))
                },
                |picture| Ok(NativeSnapshot { picture, misses: vec!["WebKit snapshots omit hardware-decoded video and WebGL".into()] }),
            );
            reply(&proxy, window, HostReply::NativeSnapshot { request, snapshot: result });
        });
        // SAFETY: All WebKit view and snapshot operations run on the main thread.
        unsafe {
            let config = WKSnapshotConfiguration::new(MainThreadMarker::from(&*self.parent));
            config.setAfterScreenUpdates(false);
            web.view().takeSnapshotWithConfiguration_completionHandler(Some(&config), &done);
        }
        Ok(())
    }
}

impl WindowNativeViews for Views {
    fn sync_native_views(&mut self, changes: &[NativeChange]) {
        for change in changes {
            match change {
                NativeChange::Create { id, kind, slot, props } => self.create(*id, kind, *slot, props),
                NativeChange::Update { id, props } => {
                    if let Some(view) = self.hosted.get_mut(id) {
                        match &mut view.platform {
                            Platform::Web(web) => web.update(props),
                            Platform::Video { controller, player } => player.update(controller, props),
                        }
                    }
                }
                NativeChange::Place { placements } => {
                    for placement in placements {
                        if let Some(view) = self.hosted.get_mut(&placement.id) {
                            view.place(placement);
                        }
                    }
                }
                NativeChange::Destroy { id } => {
                    self.hosted.remove(id);
                }
                NativeChange::CreateTyped { .. } | NativeChange::UpdateTyped { .. } => {}
            }
        }
    }

    fn send_native_command(&mut self, command: NativeCommand) {
        if let Some(view) = self.hosted.get(&command.id) {
            match &view.platform {
                Platform::Web(web) => web.command(&command.name, &command.data),
                Platform::Video { player, .. } => player.command(&command.name),
            }
        }
    }

    fn shows_native(&self, kind: &str, slot: NativeSlot) -> bool {
        slot == NativeSlot::Overlay && (kind == "web" || (kind == "video" && AnyClass::get(c"AVPlayerViewController").is_some()))
    }

    fn measure_native(&mut self, request: NativeMeasureRequest) -> Option<Size> {
        let size = self.hosted.get(&request.ticket.id())?.view.intrinsicContentSize();
        (size.width.is_finite() && size.height.is_finite() && size.width > 0.0 && size.height > 0.0)
            .then_some(Size { width: size.width as f32, height: size.height as f32 })
    }

    fn snapshot_native(&mut self, request: NativeSnapshotRequest) {
        if let Err(error) = self.snapshot(request) {
            reply(&self.proxy, self.window, HostReply::NativeSnapshot { request, snapshot: Err(error) });
        }
    }

    fn native_busy(&self, id: NativeId) -> Option<String> {
        match &self.hosted.get(&id)?.platform {
            Platform::Web(web) => web.busy(),
            Platform::Video { .. } => None,
        }
    }

    fn has_underlay(&self) -> bool {
        false
    }

    fn drain_native_events(&mut self, _: &mut Vec<WindowHostEvent>) {}

    fn inspect_native_views(&self) -> Value {
        let mut frames = self
            .hosted
            .iter()
            .map(|(id, hosted)| {
                let view = &hosted.view;
                let frame = view.convertRect_toView(view.bounds(), Some(&self.parent));
                (id.0, json!({ "id": id.0, "frame": [frame.origin.x, frame.origin.y, frame.size.width, frame.size.height] }))
            })
            .collect::<Vec<_>>();
        frames.sort_unstable_by_key(|(id, _)| *id);
        Value::Array(frames.into_iter().map(|(_, value)| value).collect())
    }
}

fn rect(r: Rect) -> NSRect {
    NSRect::new(NSPoint::new(f64::from(r.x), f64::from(r.y)), NSSize::new(f64::from(r.w.max(0.0)), f64::from(r.h.max(0.0))))
}

fn picture(image: &UIImage) -> Option<Picture> {
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

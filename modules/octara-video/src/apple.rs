//! AVKit playback shared by the AppKit and UIKit video views.

#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "ios")]
use ios::PlatformView;
#[cfg(target_os = "macos")]
use macos::PlatformView;

use crate::{VideoCommand, VideoSource, snapshot_unavailable};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{MainThreadMarker, msg_send};
use objc2_core_graphics::CGMutablePath;
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, NSURL};
use objc2_quartz_core::{CALayer, CAShapeLayer};
use octara::Rect;
use octara::module::{CallTicket, HostSurface, ModuleDispatcher, ModuleError, NativePlacement, NativeSnapshotRequest, NativeViewInstance};
use std::any::Any;

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {}

#[link(name = "AVKit", kind = "framework")]
unsafe extern "C" {}

pub(super) struct Video {
    player: Player,
    platform: PlatformView,
    visible: bool,
    snapshot_hidden: bool,
}

impl Video {
    pub(super) fn new(surface: HostSurface<'_>, source: VideoSource) -> Result<Self, ModuleError> {
        let mtm = MainThreadMarker::new().ok_or(ModuleError::Unavailable)?;
        let platform = PlatformView::new(surface, mtm)?;
        let mut player = Player::default();
        player.update(platform.player_view(), source);
        Ok(Self { player, platform, visible: false, snapshot_hidden: false })
    }
}

impl NativeViewInstance for Video {
    fn update(&mut self, props: Box<dyn Any>) {
        if let Ok(source) = props.downcast::<VideoSource>() {
            self.player.update(self.platform.player_view(), *source);
        }
    }

    fn place(&mut self, placement: &NativePlacement) {
        self.platform.place(placement);
        self.visible = placement.visible;
        self.platform.set_hidden(self.snapshot_hidden || !self.visible);
        if !self.visible {
            self.player.command(VideoCommand::Pause);
        }
    }

    fn call(&mut self, _: CallTicket, command: Box<dyn Any>, _: ModuleDispatcher) {
        if let Ok(command) = command.downcast::<VideoCommand>()
            && (self.visible || *command == VideoCommand::Pause)
        {
            self.player.command(*command);
        }
    }

    fn snapshot(&mut self, request: NativeSnapshotRequest, reply: ModuleDispatcher) {
        snapshot_unavailable(request, reply);
    }

    fn set_occluded_by_snapshot(&mut self, hidden: bool) {
        self.snapshot_hidden = hidden;
        self.platform.set_hidden(hidden || !self.visible);
    }
}

#[derive(Default)]
struct Player {
    uri: Option<String>,
    player: Option<Retained<AnyObject>>,
}

impl Player {
    fn update(&mut self, view: &AnyObject, source: VideoSource) {
        if self.uri.as_deref() != Some(&source.uri) {
            self.command(VideoCommand::Pause);
            self.player = player(&source.uri);
            self.uri = Some(source.uri);
            // SAFETY: Called on the main thread with an AVPlayerView or AVPlayerViewController.
            unsafe {
                let _: () = msg_send![view, setPlayer: self.player.as_deref()];
            }
        }
        // SAFETY: The native host passes its corresponding AVKit view and runs this on the main thread.
        unsafe {
            #[cfg(target_os = "macos")]
            let _: () = msg_send![view, setControlsStyle: isize::from(source.controls)];
            #[cfg(target_os = "ios")]
            let _: () = msg_send![view, setShowsPlaybackControls: source.controls];
        }
    }

    fn command(&self, command: VideoCommand) {
        if let Some(player) = self.player.as_deref() {
            // SAFETY: The object was created by +[AVPlayer playerWithURL:].
            unsafe {
                match command {
                    VideoCommand::Play => {
                        let _: () = msg_send![player, play];
                    }
                    VideoCommand::Pause => {
                        let _: () = msg_send![player, pause];
                    }
                }
            }
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.command(VideoCommand::Pause);
    }
}

fn rect(Rect { x, y, w, h }: Rect) -> NSRect {
    NSRect::new(NSPoint::new(x.into(), y.into()), NSSize::new(w.max(0.0).into(), h.max(0.0).into()))
}

fn video_frame(placement: &NativePlacement) -> NSRect {
    let r = placement.rect;
    let c = placement.clip;
    rect(Rect::new(r.x - c.x, r.y - c.y, r.w, r.h))
}

fn apply_corners(layer: &CALayer, [tl, tr, br, bl]: [f32; 4], top_left: bool) {
    if tl == tr && tl == br && tl == bl {
        // SAFETY: This layer owns its mask; clearing it detaches it from the layer.
        unsafe { layer.setMask(None) };
        layer.setCornerRadius(f64::from(tl));
        return;
    }

    let radii = if top_left { [tl, tr, br, bl] } else { [bl, br, tr, tl] };
    let [tl, tr, br, bl] = radii.map(f64::from);
    let bounds = layer.bounds();
    let (w, h) = (bounds.size.width, bounds.size.height);
    let path = CGMutablePath::new();
    let transform = std::ptr::null();
    let p = Some(&*path);
    // SAFETY: The path is owned here and a null transform uses the supplied local coordinates.
    unsafe {
        CGMutablePath::move_to_point(p, transform, tl, 0.0);
        for (x, y, next_x, next_y, radius) in [(w, 0.0, w, h, tr), (w, h, 0.0, h, br), (0.0, h, 0.0, 0.0, bl), (0.0, 0.0, w, 0.0, tl)] {
            if radius > 0.0 {
                CGMutablePath::add_arc_to_point(p, transform, x, y, next_x, next_y, radius);
            } else {
                CGMutablePath::add_line_to_point(p, transform, x, y);
            }
        }
    }
    CGMutablePath::close_subpath(p);

    let mask = CAShapeLayer::layer();
    mask.setFrame(bounds);
    mask.setPath(Some(&path));
    layer.setCornerRadius(0.0);
    // SAFETY: The fresh mask has no superlayer and is only installed on this layer.
    unsafe { layer.setMask(Some(&mask)) };
}

fn player(uri: &str) -> Option<Retained<AnyObject>> {
    let url = NSURL::URLWithString(&NSString::from_str(uri))?;
    let scheme = url.scheme()?.to_string().to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https" | "file") {
        return None;
    }
    let class = AnyClass::get(c"AVPlayer")?;
    // SAFETY: AVPlayer is loaded from AVFoundation. This class factory accepts NSURL and returns AVPlayer.
    unsafe { msg_send![class, playerWithURL: &*url] }
}

#[cfg(test)]
#[test]
fn avkit_frameworks_load_the_player_classes() {
    assert!(AnyClass::get(c"AVPlayer").is_some());
    #[cfg(target_os = "macos")]
    assert!(AnyClass::get(c"AVPlayerView").is_some());
    #[cfg(target_os = "ios")]
    assert!(AnyClass::get(c"AVPlayerViewController").is_some());
}

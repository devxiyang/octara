//! AVKit playback shared by the AppKit and UIKit native slot hosts.

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2_foundation::{NSString, NSURL};
use serde_json::Value;

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {}

#[link(name = "AVKit", kind = "framework")]
unsafe extern "C" {}

#[derive(Default)]
pub(super) struct Player {
    uri: Option<String>,
    player: Option<Retained<AnyObject>>,
}

impl Player {
    pub(super) fn update(&mut self, view: &AnyObject, props: &Value) {
        let uri = props.get("uri").and_then(Value::as_str);
        if self.uri.as_deref() != uri {
            self.pause();
            self.uri = uri.map(str::to_owned);
            self.player = uri.and_then(player);
            // SAFETY: Called on the main thread with an AVPlayerView or AVPlayerViewController.
            unsafe {
                let _: () = msg_send![view, setPlayer: self.player.as_deref()];
            }
        }
        let controls = props.get("controls").and_then(Value::as_bool).unwrap_or(true);
        // SAFETY: The native host passes its corresponding AVKit view and runs this on the main thread.
        unsafe {
            #[cfg(target_os = "macos")]
            let _: () = msg_send![view, setControlsStyle: isize::from(controls)];
            #[cfg(target_os = "ios")]
            let _: () = msg_send![view, setShowsPlaybackControls: controls];
        }
    }

    pub(super) fn pause(&self) {
        self.command("pause");
    }

    pub(super) fn command(&self, name: &str) {
        if let Some(player) = self.player.as_deref() {
            // SAFETY: The object was created by +[AVPlayer playerWithURL:].
            unsafe {
                match name {
                    "play" => {
                        let _: () = msg_send![player, play];
                    }
                    "pause" => {
                        let _: () = msg_send![player, pause];
                    }
                    _ => {}
                }
            }
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.pause();
    }
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

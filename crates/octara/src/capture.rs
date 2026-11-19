//! Captures: an agent's picture of the app, native views included, with a report of what the
//! picture can be trusted for ([design](../../../docs/design/capture.md)).
//!
//! The `capture` request of the [inspector protocol](crate::inspect::Request) settles the app,
//! asks the platform window for each shown view's snapshot, and paints
//! the frame with the snapshots in place through a [`Painter`]: the window's GPU renderer in a
//! shell, the reference renderer headless (`octara-render` implements both). The reply is a
//! [`Capture`] report.

use crate::image::Image;
use crate::kernel::NodeId;
use crate::native::NativeChange;
use crate::paint::{DisplayList, ImagePaint, Item, Part, Rect};
use crate::platform::NativeSnapshotRequest;
pub(crate) use octara_primitives::capture::{Painter, Picture};
use serde::{Deserialize, Serialize};
use std::rc::Rc;
use std::time::Instant;

/// The identity of one painted capture started by the platform host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureId(u128);

impl CaptureId {
    pub(crate) fn new() -> Self {
        Self(crate::identity::new_namespace())
    }
}

/// Where a capture's pixels come from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pixels {
    /// Octara composed it: its frame with each native view's snapshot in place.
    #[default]
    Painted,
    /// The window server's picture of the window: what the user sees.
    Display,
}

/// What a native view shows in a capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shown {
    /// Its own snapshot, drawn in its place.
    Snapshot,
    /// The placeholder box: its host takes no snapshots (`reason` says why).
    Placeholder,
    /// The capture could not use a snapshot (`reason` says why).
    Failed,
    /// The view itself (the window server's picture).
    Live,
}

/// One native view in a capture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NativeShot {
    /// Its test id, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub kind: String,
    /// `[x, y, width, height]` in logical pixels.
    pub rect: [f32; 4],
    pub shown: Shown,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// What the snapshot cannot show (WebKit: hardware video and WebGL come out black).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub misses: Vec<String>,
}

/// A capture's report: the picture, and what it can be trusted for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Capture {
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub pixels: Pixels,
    /// Why `display` was asked and `painted` given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
    /// `gpu`, `reference` or `display`.
    pub renderer: String,
    /// No work was pending when the app frame was sampled.
    pub settled: bool,
    /// What was still running when the wait ended: tasks, motion, native views loading.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native: Vec<NativeShot>,
}

impl Capture {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a capture serializes")
    }
}

/// A capture in progress and its collected native responses.
pub(crate) struct Capturing {
    pub id: CaptureId,
    pub deadline: Instant,
    pub list: DisplayList,
    pub shots: Vec<NativeShotRequest>,
}

impl Capturing {
    pub(crate) fn native_changed(&mut self, changes: &[NativeChange]) {
        for shot in &mut self.shots {
            let target = shot.request.id;
            if changes.iter().any(|change| match change {
                NativeChange::Create { id, .. } | NativeChange::Update { id, .. } | NativeChange::Destroy { id } => *id == target,
                NativeChange::Place { placements } => placements.iter().any(|placement| placement.id == target),
            }) {
                shot.invalidate("the native view's committed content or placement changed while capturing");
            }
        }
    }
}

pub(crate) struct NativeShotRequest {
    pub node: NodeId,
    pub request: NativeSnapshotRequest,
    pub status: SnapshotStatus,
    pub report: NativeShot,
}

impl NativeShotRequest {
    pub(crate) fn invalidate(&mut self, reason: &'static str) {
        if matches!(&self.status, SnapshotStatus::Waiting | SnapshotStatus::Captured(_)) {
            self.status = SnapshotStatus::Failed(reason);
        }
    }
}

pub(crate) enum SnapshotStatus {
    Waiting,
    Captured(Image),
    Failed(&'static str),
    Placeholder(String),
}

/// The app frame associated with a host's display capture attempt.
///
/// The window server supplies the pixels independently of the app's submission.
pub struct DisplayCapture {
    pub(crate) identity: crate::identity::TreeIdentity,
    pub(crate) revision: crate::app::WindowRevision,
    pub(crate) outline: Option<(NodeId, Rect)>,
    pub(crate) images: Vec<(Image, u64)>,
    pub(crate) native: Vec<NativeShot>,
}

pub(crate) fn freeze_image(image: &Image) -> Image {
    let frame = image.frame();
    Image::rgba(frame.width, frame.height, frame.rgba)
}

pub(crate) fn paint_native_snapshots(list: &mut DisplayList, snapshots: &mut rustc_hash::FxHashMap<NodeId, Image>) {
    for part in &mut list.parts {
        let Part::Drawing { items, .. } = part else { continue };
        let [.., Item::Quad(placeholder), Item::Native { source, rect, radius, cut: false, .. }] = items.as_ref() else { continue };
        if placeholder.source != *source {
            continue;
        }
        if let Some(image) = snapshots.remove(source) {
            let painted = Item::Image(ImagePaint { source: *source, image, rect: *rect, bounds: *rect, radius: *radius, opacity: 1.0 });
            let index = items.len() - 2;
            Rc::make_mut(items)[index] = painted;
        }
    }
}

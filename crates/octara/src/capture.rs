//! Captures: an agent's picture of the app, native views included, with a report of what the
//! picture can be trusted for ([design](../../../docs/design/capture.md)).
//!
//! The `capture` request of the [inspector protocol](crate::inspect::Request) settles the app,
//! asks the platform window for each shown view's snapshot, and paints
//! the frame with the snapshots in place through a [`Painter`]: the window's GPU renderer in a
//! shell, the reference renderer headless (`octara-render` implements both). The reply is a
//! [`Capture`] report.

use crate::app::WindowId;
use crate::image::{Image, Sample};
use crate::inspect::ComponentInfo;
use crate::kernel::{Kernel, NodeId};
use crate::native::NativeChange;
use crate::paint::{DisplayList, ImagePaint, Item, Part, Rect};
use crate::platform::NativeSnapshotRequest;
use octara_primitives::capture::PaintedPicture;
pub use octara_primitives::capture::{PaintFallback, PaintOutcome, PaintReceipt, PaintSample};
pub(crate) use octara_primitives::capture::{Painter, Picture};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
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

/// Identifies the drawing and what produced its author paint receipts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaintFrame {
    pub window: u64,
    #[serde(flatten)]
    pub sample: PaintSample,
    #[serde(flatten)]
    pub origin: PaintOrigin,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<EffectPaint>,
}

impl PaintFrame {
    pub(crate) fn from_receipts(
        window: crate::app::WindowId,
        sample: PaintSample,
        origin: PaintOrigin,
        paint_sites: &FxHashMap<NodeId, PaintSite>,
        receipts: &[PaintReceipt],
    ) -> Self {
        let effects = receipts
            .iter()
            .filter_map(|receipt| paint_sites.get(&receipt.source).cloned().map(|site| EffectPaint { site, outcome: receipt.outcome }))
            .collect();
        Self { window: window.number(), sample, origin, effects }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "origin", rename_all = "snake_case")]
pub enum PaintOrigin {
    /// The drawing was recorded again into an offscreen target.
    Rerecorded {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device_generation: Option<u64>,
    },
    /// The receipts came from a successful surface submission.
    Submitted { frame: u32, surface_generation: u64, device_generation: u64 },
}

/// An observed author paint outcome at a mounted or outgoing drawing site.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectPaint {
    #[serde(flatten)]
    pub site: PaintSite,
    #[serde(flatten)]
    pub outcome: PaintOutcome,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaintSite {
    /// The Inspector identity at sampling time. An outgoing site's target is already retired.
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<ComponentInfo>,
    pub exiting: bool,
}

impl PaintSite {
    pub(crate) fn of(kernel: &Kernel, node: NodeId) -> Option<Self> {
        let target = crate::inspect::paint_target(kernel, node)?;
        Some(Self {
            target,
            id: kernel.test_id(node).map(str::to_owned),
            source: kernel.origin(node).map(|at| format!("{}:{}:{}", crate::diagnostics::source_file(at), at.line(), at.column())),
            component: kernel.component(node).map(ComponentInfo::of),
            exiting: !kernel.is_mounted(node),
        })
    }
}

pub(crate) fn paint_sites(kernel: &Kernel, list: &DisplayList) -> FxHashMap<NodeId, PaintSite> {
    list.parts
        .iter()
        .flat_map(|part| match part {
            Part::Drawing { items, .. } => items.as_ref(),
            Part::Item(item) => std::slice::from_ref(item),
        })
        .filter_map(|item| {
            let node = match item {
                Item::Custom(paint) => paint.fallback.source,
                Item::PushTransition(paint) => paint.source,
                Item::SkipTransition { source, .. } => *source,
                _ => return None,
            };
            PaintSite::of(kernel, node).map(|site| (node, site))
        })
        .collect()
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
    /// The paint receipts for the replay or the submission checked when sampling the display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paint: Option<PaintFrame>,
}

impl Capture {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a capture serializes")
    }
}

/// A window's frozen offscreen capture, ready to paint without access to the window.
pub struct PreparedCapture {
    pub(crate) window: WindowId,
    pub(crate) list: Arc<DisplayList>,
    pub(crate) sample: PaintSample,
    pub(crate) paint_sites: Arc<FxHashMap<NodeId, PaintSite>>,
    pub(crate) native: Vec<NativeShot>,
}

impl PreparedCapture {
    pub fn window(&self) -> WindowId {
        self.window
    }

    pub fn display_list(&self) -> Arc<DisplayList> {
        self.list.clone()
    }

    pub fn paint_sample(&self) -> PaintSample {
        self.sample
    }

    /// Writes the painted picture and attributes its receipts to this capture's sampled sites.
    pub fn finish(
        self,
        recording: PaintedPicture,
        renderer: &str,
        path: &str,
        scale: f32,
        pending: Vec<String>,
        fallback: Option<String>,
    ) -> Result<Capture, String> {
        let PaintedPicture { picture, effects } = recording;
        picture.save_png(path).map_err(|e| format!("{path}: {e}"))?;
        let origin = PaintOrigin::Rerecorded { device_generation: None };
        let paint = PaintFrame::from_receipts(self.window, self.sample, origin, &self.paint_sites, &effects);
        Ok(Capture {
            path: path.to_owned(),
            width: picture.width,
            height: picture.height,
            scale,
            pixels: Pixels::Painted,
            fallback,
            renderer: renderer.to_owned(),
            settled: pending.is_empty(),
            pending,
            native: self.native,
            paint: Some(paint),
        })
    }
}

/// A capture in progress and its collected native responses.
pub(crate) struct Capturing {
    pub id: CaptureId,
    pub deadline: Instant,
    pub sample: PaintSample,
    pub list: Arc<DisplayList>,
    pub paint_sites: Arc<FxHashMap<NodeId, PaintSite>>,
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
    Captured(Sample),
    Failed(&'static str),
    Placeholder(String),
}

/// The app frame associated with a host's display capture attempt.
///
/// The window server supplies the pixels independently of the app's submission.
pub struct DisplayCapture {
    pub(crate) identity: crate::identity::TreeIdentity,
    pub(crate) revision: crate::app::WindowRevision,
    pub(crate) drawing_revision: u64,
    pub(crate) outline: Option<(NodeId, Rect)>,
    pub(crate) images: Vec<(Image, u64)>,
    pub(crate) native: Vec<NativeShot>,
}

pub(crate) fn paint_native_snapshots(list: &mut DisplayList, snapshots: &mut rustc_hash::FxHashMap<NodeId, Sample>) {
    for part in &mut list.parts {
        let Part::Drawing { items, .. } = part else { continue };
        let [.., Item::Quad(placeholder), Item::Native { source, rect, radius, cut: false, .. }] = items.as_ref() else { continue };
        if placeholder.source != *source {
            continue;
        }
        if let Some(image) = snapshots.remove(source) {
            let painted = Item::Image(ImagePaint { source: *source, image, rect: *rect, bounds: *rect, radius: *radius, opacity: 1.0 });
            let index = items.len() - 2;
            Arc::make_mut(items)[index] = painted;
        }
    }
}

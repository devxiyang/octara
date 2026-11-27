//! Values exchanged with a host that mounts native views.

use crate::geometry::{Rect, WindowViewport};
use crate::identity::{MeasureTicket, NativeId, NodeId, TreeId};
use crate::paint::Picture;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The stacking slot of a native view relative to the framework's surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeSlot {
    Overlay,
    Underlay,
}

/// Where a native view goes in logical window coordinates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NativePlacement {
    pub id: NativeId,
    pub slot: NativeSlot,
    pub rect: Rect,
    pub radius: [f32; 4],
    pub clip: Rect,
    pub clip_radius: f32,
    pub opacity: f32,
    pub visible: bool,
    /// Covered parts of an underlay route pointer input to the framework.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub covered: Vec<Rect>,
}

/// A native view's state changes for a committed frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum NativeChange {
    Create { id: NativeId, kind: String, slot: NativeSlot, props: Value },
    Update { id: NativeId, props: Value },
    Place { placements: Vec<NativePlacement> },
    Destroy { id: NativeId },
}

/// An imperative request to a mounted native view in a window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NativeCommand {
    pub id: NativeId,
    pub name: String,
    pub data: Value,
}

/// Event data emitted by a native view, such as navigation or region changes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NativeEvent {
    pub name: String,
    pub data: Value,
}

/// Identifies one intrinsic-size query for a particular view and its input revisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NativeMeasureTicket {
    pub measure: MeasureTicket,
    pub node: NodeId,
    pub props_rev: u64,
}

impl NativeMeasureTicket {
    pub fn id(self) -> NativeId {
        self.node.into()
    }
}

/// The inputs a host needs to measure a native view, including before its first placement.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NativeMeasureRequest {
    pub ticket: NativeMeasureTicket,
    pub kind: String,
    pub slot: NativeSlot,
    pub props: Value,
    pub viewport: WindowViewport,
}

/// The request identity that a host echoes when it completes a native snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NativeSnapshotRequest {
    pub tree: TreeId,
    pub id: NativeId,
    pub request: u64,
}

/// A native view's captured pixels and any content the platform could not capture.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeSnapshot {
    pub picture: Picture,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub misses: Vec<String>,
}

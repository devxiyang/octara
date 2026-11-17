//! Native-view data exchanged between the document and platform adapters.
//! This module owns no windows and performs no OS calls.

use crate::paint::Rect;
use serde::Serialize;
use serde_json::Value;

/// Where a native view is stacked: behind Octara's surface or above it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Slot {
    /// Behind the surface, through a cutout (maps, video layers, materials).
    Underlay,
    /// Above the surface (web views, pickers, a platform text field).
    Overlay,
}

/// A native view's identity across frames (stable while its element is mounted).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, serde::Deserialize, PartialOrd, Ord)]
pub struct NativeId(pub u64);

/// Where a native view goes this frame, in logical window coordinates.
#[derive(Clone, PartialEq, Debug, Serialize, serde::Deserialize)]
pub struct Placement {
    pub id: NativeId,
    pub slot: Slot,
    /// The view's box.
    pub rect: Rect,
    /// Its corner radius.
    pub radius: f32,
    /// What clips it (scroll containers, `overflow_hidden`): a rect rounded by a radius.
    pub clip: Rect,
    pub clip_radius: f32,
    /// Its opacity (its own and its ancestors').
    pub opacity: f32,
    /// Whether any of it shows (in the window, inside its clip, displayed, not transparent).
    pub visible: bool,
    /// An underlay view's parts Octara content covers (what is painted after it and overlaps
    /// it, clipped): pointer input there goes to Octara, elsewhere in the view to the view.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub covered: Vec<Rect>,
}

/// An event a native view sends back: a name and its data (`load` with a URL, `region` with a
/// map's bounds).
#[derive(Clone, PartialEq, Debug, Serialize, serde::Deserialize)]
pub struct NativeEvent {
    pub name: String,
    pub data: Value,
}

/// What changed about a window's native views in a frame: the app presents these to its
/// platform window as part of its frame, which applies them
/// with the frame (placements in its transaction). Headless they are kept for tests.
#[derive(Clone, PartialEq, Debug, Serialize, serde::Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum NativeChange {
    /// A view of `kind` appears in `slot` with `props`.
    Create { id: NativeId, kind: String, slot: Slot, props: Value },
    /// A view's props changed (handed over whole).
    Update { id: NativeId, props: Value },
    /// Views whose placement changed.
    Place { placements: Vec<Placement> },
    /// A view goes away.
    Destroy { id: NativeId },
}

/// What `inspect` shows of a native view.
#[derive(Clone, PartialEq, Debug, Serialize, serde::Deserialize)]
pub struct NativeInfo {
    pub kind: String,
    pub slot: String,
    pub props: Value,
}

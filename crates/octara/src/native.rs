//! Native Island: platform views placed in Octara's layout ([design](../../../docs/design/native-island.md)).
//!
//! [`native_view`](crate::view::native_view)`(kind)` is a leaf box a platform view fills: a web
//! view, a map, a platform picker. Views live in one of two [`Slot`]s per window: an
//! **underlay** behind Octara's surface (shown through a cutout where the view is, so content
//! painted after it draws over it, as map controls do) and an **overlay** above it (a web view,
//! a picker): Octara content painted over an overlay view would be hidden by it, so `check`
//! reports it (`NATIVE_COVERED`).
//!
//! After each frame the app presents what changed to the window's platform window as
//! [`NativeChange`]s: views created and destroyed, props (one JSON value, handed over whole), and
//! [`Placement`]s (where each view is, its corner radius, the clip around it, its opacity,
//! whether it shows). Events and natural sizes come back as input
//! ([`InputEvent::Native`](crate::app::InputEvent::Native), `NativeSize`). A view the platform
//! does not show draws as a placeholder box; its snapshot (taken for a
//! [capture](crate::capture)) draws in its place, so captures show the page or the map.

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
/// platform window ([`Frame::native`](crate::platform::Frame::native)), which applies them
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

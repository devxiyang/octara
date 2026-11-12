//! Native Island: platform views placed in Octara's layout ([design](../../../docs/design/native-island.md)).
//!
//! [`native_view`](crate::view::native_view)`(kind)` is a leaf box a platform view fills: a web
//! view, a map, a platform picker. Views live in one of two [`Slot`]s per window: an
//! **underlay** behind Octara's surface (shown through a cutout where the view is, so content
//! painted after it draws over it, as map controls do) and an **overlay** above it (a web view,
//! a picker): Octara content painted over an overlay view would be hidden by it, so `check`
//! reports it (`NATIVE_COVERED`).
//!
//! After each frame the app tells its [`NativeHost`] what changed: views created and destroyed,
//! props (one JSON value, handed over whole), and [`Placement`]s (where each view is, its corner
//! radius, the clip around it, its opacity, whether it shows). The host sends events back with
//! [`App::native_event`](crate::app::App::native_event) and reports a view's natural size with
//! [`App::native_size`](crate::app::App::native_size). Headless, the [`RecordingHost`] keeps
//! every call, and the view draws as a placeholder box; a host's snapshot of a view (taken for a
//! [capture](crate::capture)) draws in its place, so captures show the page or the map.

use crate::paint::Rect;
use serde::Serialize;
use serde_json::Value;
use std::any::Any;

/// Where a native view is stacked: behind Octara's surface or above it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Slot {
    /// Behind the surface, through a cutout (maps, video layers, materials).
    Underlay,
    /// Above the surface (web views, pickers, a platform text field).
    Overlay,
}

/// A native view's identity across frames (stable while its element is mounted).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, PartialOrd, Ord)]
pub struct NativeId(pub u64);

/// Where a native view goes this frame, in logical window coordinates.
#[derive(Clone, PartialEq, Debug, Serialize)]
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
}

/// An event a native view sends back: a name and its data (`load` with a URL, `region` with a
/// map's bounds).
#[derive(Clone, PartialEq, Debug, Serialize)]
pub struct NativeEvent {
    pub name: String,
    pub data: Value,
}

/// The platform side: creates, updates, places and destroys native views. A shell implements it
/// with the platform's views; [`RecordingHost`] records the calls (headless and tests).
pub trait NativeHost: Any {
    /// A view of `kind` appears with `props`.
    fn create(&mut self, id: NativeId, kind: &str, props: &Value);
    /// A view's props changed (handed over whole).
    fn update(&mut self, id: NativeId, props: &Value);
    /// Views whose placement changed this frame (shells apply them with the frame's present).
    fn place(&mut self, placements: &[Placement]);
    /// A view goes away.
    fn destroy(&mut self, id: NativeId);
    /// Takes an image of what view `id` shows, handed back later with
    /// [`App::native_snapshot`](crate::app::App::native_snapshot): `Ok` with what the snapshot
    /// cannot show (WebKit: hardware video and WebGL come out black), or `Err` with why this
    /// host takes none.
    fn snapshot(&mut self, id: NativeId) -> Result<Vec<String>, String> {
        let _ = id;
        Err("this host takes no snapshots".into())
    }
    /// What view `id` is busy with (a page loading), so a capture waits for it.
    fn busy(&self, id: NativeId) -> Option<String> {
        let _ = id;
        None
    }
    fn as_any(&self) -> &dyn Any;
}

/// One call a [`RecordingHost`] received.
#[derive(Clone, PartialEq, Debug, Serialize)]
#[serde(tag = "call", rename_all = "snake_case")]
pub enum HostCall {
    Create { id: NativeId, kind: String, props: Value },
    Update { id: NativeId, props: Value },
    Place { placements: Vec<Placement> },
    Destroy { id: NativeId },
}

/// A host that records what it is told: the headless app's, and tests'.
#[derive(Default, Debug)]
pub struct RecordingHost {
    pub calls: Vec<HostCall>,
}

impl NativeHost for RecordingHost {
    fn create(&mut self, id: NativeId, kind: &str, props: &Value) {
        self.calls.push(HostCall::Create { id, kind: kind.to_owned(), props: props.clone() });
    }
    fn update(&mut self, id: NativeId, props: &Value) {
        self.calls.push(HostCall::Update { id, props: props.clone() });
    }
    fn place(&mut self, placements: &[Placement]) {
        self.calls.push(HostCall::Place { placements: placements.to_vec() });
    }
    fn destroy(&mut self, id: NativeId) {
        self.calls.push(HostCall::Destroy { id });
    }
    fn snapshot(&mut self, _: NativeId) -> Result<Vec<String>, String> {
        Err("headless: no platform view to take a snapshot of".into())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// What `inspect` shows of a native view.
#[derive(Clone, PartialEq, Debug, Serialize, serde::Deserialize)]
pub struct NativeInfo {
    pub kind: String,
    pub slot: String,
    pub props: Value,
}

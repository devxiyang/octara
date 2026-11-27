//! Values exchanged with a host that mounts native views.

use crate::geometry::{Rect, WindowViewport};
use crate::identity::{MeasureTicket, NativeId, NodeId, TreeId};
use crate::module::CallTicket;
use crate::paint::Picture;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::any::Any;
use std::fmt;
use std::sync::Arc;

trait CloneProps: Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn boxed(&self) -> Box<dyn Any>;
}

impl<P: Clone + Send + Sync + 'static> CloneProps for P {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn boxed(&self) -> Box<dyn Any> {
        Box::new(self.clone())
    }
}

/// Immutable native view props and an optional inspection summary.
/// Serialized values retain the summary without the in-process Rust payload.
#[derive(Clone, Serialize, Deserialize)]
pub struct TypedNativeProps {
    #[serde(skip)]
    payload: Option<Arc<dyn CloneProps>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspect: Option<Value>,
}

impl TypedNativeProps {
    pub fn new<P: Clone + Send + Sync + 'static>(payload: P, inspect: Option<Value>) -> Self {
        Self { payload: Some(Arc::new(payload)), inspect }
    }

    pub fn downcast_ref<P: 'static>(&self) -> Option<&P> {
        self.payload.as_deref()?.as_any().downcast_ref()
    }

    pub fn boxed_payload(&self) -> Option<Box<dyn Any>> {
        self.payload.as_deref().map(CloneProps::boxed)
    }
}

impl fmt::Debug for TypedNativeProps {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("TypedNativeProps").field("inspect", &self.inspect).finish_non_exhaustive()
    }
}

impl PartialEq for TypedNativeProps {
    fn eq(&self, other: &Self) -> bool {
        let same_payload = match (&self.payload, &other.payload) {
            (Some(left), Some(right)) => Arc::ptr_eq(left, right),
            (None, None) => true,
            _ => false,
        };
        same_payload && self.inspect == other.inspect
    }
}

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
    CreateTyped { id: NativeId, kind: String, slot: NativeSlot, props: TypedNativeProps },
    UpdateTyped { id: NativeId, props: TypedNativeProps },
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

/// A typed imperative request for a native view.
pub struct TypedNativeCommand {
    pub id: NativeId,
    pub ticket: CallTicket,
    pub name: &'static str,
    pub inspect: Option<Value>,
    pub payload: Box<dyn Any>,
}

impl fmt::Debug for TypedNativeCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TypedNativeCommand")
            .field("id", &self.id)
            .field("ticket", &self.ticket)
            .field("name", &self.name)
            .field("inspect", &self.inspect)
            .finish_non_exhaustive()
    }
}

/// Event data emitted by a native view, such as navigation or region changes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NativeEvent {
    pub name: String,
    pub data: Value,
}

/// A typed event whose payload can cross from a host callback to the runtime.
pub struct TypedNativeEvent {
    pub name: &'static str,
    pub inspect: Option<Value>,
    payload: Box<dyn Any + Send>,
}

impl TypedNativeEvent {
    pub fn new<E: Send + 'static>(name: &'static str, payload: E, inspect: Option<Value>) -> Self {
        Self { name, inspect, payload: Box::new(payload) }
    }

    pub fn downcast_ref<E: 'static>(&self) -> Option<&E> {
        self.payload.downcast_ref()
    }
}

impl fmt::Debug for TypedNativeEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("TypedNativeEvent").field("name", &self.name).field("inspect", &self.inspect).finish_non_exhaustive()
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typed_props: Option<TypedNativeProps>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn typed_props_share_payload_identity_and_serialize_only_the_summary() {
        #[derive(Clone)]
        struct Props(u32);

        let props = TypedNativeProps::new(Props(47), Some(json!({ "label": "example" })));
        assert_eq!(props, props.clone());
        assert_ne!(props, TypedNativeProps::new(Props(47), props.inspect.clone()));
        assert_eq!(props.downcast_ref::<Props>().unwrap().0, 47);
        assert_eq!(props.boxed_payload().unwrap().downcast::<Props>().unwrap().0, 47);
        let json = serde_json::to_value(&props).unwrap();
        assert_eq!(json, json!({ "inspect": { "label": "example" } }));
        let decoded: TypedNativeProps = serde_json::from_value(json).unwrap();
        assert_eq!(decoded.inspect, props.inspect);
        assert!(decoded.boxed_payload().is_none());
    }
}

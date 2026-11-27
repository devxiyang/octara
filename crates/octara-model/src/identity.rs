//! Identities and revision stamps shared by the tree, runtime, and hosts.

use crate::capture::PaintSample;
use serde::{Deserialize, Serialize};
use slotmap::Key as _;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};

slotmap::new_key_type! {
    /// A generational handle to a node allocation within a tree.
    pub struct NodeId;
    /// A generational handle to a window allocation within an application.
    pub struct WindowId;
}

/// One incarnation of a tree within a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TreeId {
    pub namespace: [u64; 2],
    pub window: u64,
    pub generation: u64,
}

impl TreeId {
    pub const fn new(namespace: [u64; 2], window: u64) -> Self {
        Self { namespace, window, generation: 0 }
    }

    /// Allocate a namespace to share among all windows of a new application.
    pub fn fresh_namespace() -> [u64; 2] {
        static SEED: LazyLock<[u64; 2]> = LazyLock::new(namespace_seed);
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let index = NEXT.try_update(Ordering::Relaxed, Ordering::Relaxed, |index| index.checked_add(1)).expect("app namespace exhausted");
        [SEED[0], SEED[1] ^ index]
    }

    /// Advance the tree incarnation after replacing its retained nodes.
    pub fn next_generation(self) -> Option<Self> {
        Some(Self { generation: self.generation.checked_add(1)?, ..self })
    }
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn namespace_seed() -> [u64; 2] {
    std::array::from_fn(|_| getrandom::u64().expect("browser entropy for Octara identity"))
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
fn namespace_seed() -> [u64; 2] {
    use std::collections::hash_map::RandomState;
    use std::hash::BuildHasher as _;

    std::array::from_fn(|index| RandomState::new().hash_one((std::process::id(), index)))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TreeRev(pub u64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ViewportRev(pub u64);

/// Authored tree and viewport revisions used to settle a window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WindowRev {
    pub tree: TreeRev,
    pub viewport: ViewportRev,
}

/// One geometry request for a specific incarnation and revision of a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MeasureTicket {
    pub tree: TreeId,
    pub window_rev: WindowRev,
    pub request: u64,
}

/// Completion of one visual exit on a particular retained allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ExitTicket {
    pub tree: TreeId,
    pub node: NodeId,
}

/// The identity of a frame from admission through presentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameStamp {
    pub tree: TreeId,
    pub frame: u64,
    pub surface_generation: u64,
    pub device_generation: u64,
    pub native_rev: u64,
    pub sample: PaintSample,
}

/// A visit to a focused editor and the incarnation of its document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EditorSessionId {
    pub tree: TreeId,
    pub node: NodeId,
    pub session_gen: u64,
}

/// A target identified in a particular tree incarnation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TargetGuard {
    pub tree: TreeId,
    pub target: GuardTarget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardTarget {
    Window,
    Node(NodeId),
}

impl TargetGuard {
    pub const fn for_window(tree: TreeId) -> Self {
        Self { tree, target: GuardTarget::Window }
    }

    pub const fn for_node(tree: TreeId, node: NodeId) -> Self {
        Self { tree, target: GuardTarget::Node(node) }
    }

    pub const fn node(self) -> Option<NodeId> {
        match self.target {
            GuardTarget::Window => None,
            GuardTarget::Node(node) => Some(node),
        }
    }
}

/// A mounted native view's identity within its tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NativeId(pub u64);

impl NativeId {
    /// The native node key. Resolve it only in the tree that supplied this identity.
    pub fn node(self) -> NodeId {
        NodeId::from(slotmap::KeyData::from_ffi(self.0))
    }
}

impl From<NodeId> for NativeId {
    fn from(node: NodeId) -> Self {
        Self(node.data().as_ffi())
    }
}

/// A row's business key within a structural region under its parent.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RowKey {
    pub region: u64,
    pub key: RowValue,
}

impl RowKey {
    pub fn new(region: u64, key: impl Into<RowValue>) -> Self {
        Self { region, key: key.into() }
    }
}

impl fmt::Display for RowKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.key {
            RowValue::Text(value) => value.fmt(f),
            RowValue::Signed(value) => value.fmt(f),
            RowValue::Unsigned(value) => value.fmt(f),
        }
    }
}

/// A business key whose type participates in equality.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RowValue {
    Text(Arc<str>),
    Signed(i64),
    Unsigned(u64),
}

impl From<Arc<str>> for RowValue {
    fn from(value: Arc<str>) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for RowValue {
    fn from(value: &str) -> Self {
        Self::Text(value.into())
    }
}

impl From<String> for RowValue {
    fn from(value: String) -> Self {
        Self::Text(value.into())
    }
}

impl From<i64> for RowValue {
    fn from(value: i64) -> Self {
        Self::Signed(value)
    }
}

impl From<u64> for RowValue {
    fn from(value: u64) -> Self {
        Self::Unsigned(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slotmap::SlotMap;

    #[test]
    fn namespace_generation_and_reused_slots_have_distinct_targets() {
        let tree = TreeId::new(TreeId::fresh_namespace(), 1);
        let another_app = TreeId::new(TreeId::fresh_namespace(), 1);
        let replacement = tree.next_generation().unwrap();
        let mut nodes = SlotMap::<NodeId, ()>::with_key();
        let node = nodes.insert(());
        let before = TargetGuard::for_node(tree, node);
        nodes.remove(node);
        let new_node = nodes.insert(());

        assert_ne!(before, TargetGuard::for_node(tree, new_node));
        assert_ne!(before, TargetGuard::for_node(another_app, node));
        assert_ne!(before, TargetGuard::for_node(replacement, node));
        assert_ne!(NativeId::from(node), NativeId::from(new_node));
        assert_eq!(serde_json::from_value::<TargetGuard>(serde_json::to_value(before).unwrap()).unwrap(), before);
    }

    #[test]
    fn row_key_uses_region_and_business_key_type() {
        let key = RowKey::new(1, "42");
        assert_ne!(key, RowKey::new(2, "42"));
        assert_ne!(key, RowKey::new(1, 42u64));
        assert_eq!(key, RowKey::new(1, String::from("42")));
        assert_eq!(serde_json::from_value::<RowKey>(serde_json::to_value(&key).unwrap()).unwrap(), key);
    }
}

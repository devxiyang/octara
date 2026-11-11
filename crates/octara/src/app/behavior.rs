//! What built-in elements do with pointer gestures besides running their handlers: the
//! recognizer (`pointer.rs`) reports gestures through this interface and knows no element kinds;
//! each kind with behavior of its own (a text field places its caret and selects) implements it.

use super::App;
use super::pointer::{PointerEvent, PointerKind};
use crate::kernel::{Kind, NodeId};

pub(crate) trait Behavior: Sync {
    /// The primary button (or a finger) went down on `n`; `clicks` counts the run on it (1, then
    /// 2 for a double click, …).
    fn down(&self, _app: &App, _n: NodeId, _ev: &PointerEvent, _clicks: u32) {}
    /// Whether a drag on the element with this kind of pointer is its own (rather than a scroll,
    /// or an `on_drag` handler's).
    fn drags(&self, _kind: PointerKind) -> bool {
        false
    }
    /// The element's own drag is at `(x, y)` (from the first move past the slop to the release).
    fn drag(&self, _app: &App, _n: NodeId, _at: (f32, f32)) {}
}

/// The behavior of an element kind, if it has any.
pub(crate) fn behavior(kind: Kind) -> Option<&'static dyn Behavior> {
    match kind {
        Kind::TextInput => Some(&super::edit::FIELD),
        Kind::Root | Kind::Box | Kind::Text => None,
    }
}

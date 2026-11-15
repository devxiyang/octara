//! Operation-local ownership checks, independent of the Inspector protocol.

use super::Window;
use crate::identity::TreeIdentity;
use crate::kernel::NodeId;

/// The allocation chosen for one operation, retained through its input phases or host waits.
/// This checks lifetime and optionally focus, not authorization or property versions.
#[derive(Clone, Debug)]
pub struct TargetGuard {
    tree: TreeIdentity,
    pub(crate) node: Option<NodeId>,
    pub(crate) focus: bool,
}

impl TargetGuard {
    pub(crate) fn new(window: &Window, node: Option<NodeId>, focus: bool) -> Self {
        Self { tree: window.kernel().identity(), node, focus }
    }

    pub fn node(&self) -> Option<NodeId> {
        self.node
    }

    /// Checks the same app/window/tree, live node and optional original focus. Do not recheck
    /// after the final handler: a successful Tab or delete may move focus or remove itself.
    pub fn is_current(&self, window: &Window) -> bool {
        if window.is_closed() {
            return false;
        }
        let k = window.kernel();
        if k.identity() != self.tree || self.node.is_some_and(|node| !k.is_alive(node) || k.is_exiting(node)) {
            return false;
        }
        !self.focus || self.node == window.focused_node()
    }

    /// The guarded node still owns focus, for the text phase after focusing it.
    pub fn is_focused(&self, window: &Window) -> bool {
        self.node.is_some() && self.is_current(window) && self.node == window.focused_node()
    }
}

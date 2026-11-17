//! Shared semantic resolution. Names never fall back to editable values or visual hints.
//! Output adapters retain their own encoding (AccessKit static text uses a value).

use super::{Kernel, Kind, NodeId};

impl Kernel {
    /// A node's own name, not an aggregate of its descendants. An explicit empty label
    /// suppresses text fallback; absent labels use nonempty document text only.
    pub(crate) fn semantic_name(&self, id: NodeId) -> Option<&str> {
        let n = self.node(id);
        n.label.as_deref().or_else(|| (n.kind() == Kind::Text && !n.text().is_empty()).then(|| n.text()))
    }

    /// The explicit description, else the tooltip unless it repeats the resolved name.
    pub fn description(&self, id: NodeId) -> Option<&str> {
        let n = self.node(id);
        n.description.as_deref().or_else(|| n.tooltip.as_deref().filter(|t| self.semantic_name(id) != Some(*t)))
    }

    /// Shown nodes whose accessible name is `name`: an explicit label, else a text's own text.
    /// In tree order.
    pub fn find_by_name(&self, name: &str) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = vec![self.root];
        while let Some(id) = stack.pop() {
            let n = self.node(id);
            if !self.shown_to_a11y(id) {
                continue;
            }
            let own = self.semantic_name(id);
            if own == Some(name) {
                out.push(id);
            }
            stack.extend(n.children().iter().rev().copied());
        }
        out
    }
    /// How to name a node in a message: its test id, else its accessible name, else its role.
    pub fn describe(&self, id: NodeId) -> String {
        let n = self.node(id);
        if let Some(t) = &n.test_id {
            format!("`{t}`")
        } else if let Some(l) = self.semantic_name(id).filter(|l| !l.is_empty()) {
            format!("{:?} \"{l}\"", self.role(id))
        } else {
            format!("a {:?}", self.role(id))
        }
    }
}

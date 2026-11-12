//! What people point at ([core](../../../../docs/design/core.md)): the element at a point,
//! where it was made, and the components it is part of.

use super::{Kernel, NodeId};
use crate::inspect::{Ancestor, Picked};

impl Kernel {
    /// Where a node was created (`file:line:column`).
    pub(crate) fn source_of(&self, id: NodeId) -> Option<String> {
        self.node(id).origin.map(|l| format!("{}:{}:{}", crate::diagnostics::source_file(l), l.line(), l.column()))
    }

    /// The topmost element at `(x, y)` (logical window pixels), with what surrounds it.
    pub fn pick(&self, x: f32, y: f32) -> Option<Picked> {
        let hit = self.hit_test(x, y)?;
        let node = self.inspect_one(hit);
        let mut ancestors = Vec::new();
        let mut p = self.node(hit).parent;
        while let Some(id) = p {
            if id == self.root {
                break;
            }
            if id != self.overlay_layer {
                let one = self.inspect_one(id);
                ancestors.push(Ancestor { role: one.role, id: one.id, source: one.source, component: one.component });
            }
            p = self.node(id).parent;
        }
        let describe = |role: &str, id: &Option<String>, source: &Option<String>| {
            let mut s = role.to_owned();
            if let Some(id) = id {
                s.push_str(&format!(" (id {id})"));
            }
            if let Some(src) = source {
                s.push_str(&format!(" at {src}"));
            }
            s
        };
        let mut text = describe(&node.role, &node.id, &node.source);
        if let Some(name) = &node.name {
            let name: String = name.chars().take(40).collect();
            text = text.replacen(&node.role, &format!("{} {name:?}", node.role), 1);
        }
        // The nearest ancestors that say where it is: an id, or another source line.
        let mut last = node.source.clone();
        let mut shown = 0;
        for a in &ancestors {
            if shown == 3 {
                break;
            }
            if a.id.is_some() || (a.source.is_some() && a.source != last) {
                text.push_str(&format!(", in {}", describe(&a.role, &a.id, &a.source)));
                last = a.source.clone();
                shown += 1;
            }
        }
        // The components it is part of, innermost first.
        let mut made: Vec<&crate::inspect::ComponentInfo> = Vec::new();
        for c in std::iter::once(&node.component).chain(ancestors.iter().map(|a| &a.component)).flatten() {
            if made.last() != Some(&c) {
                made.push(c);
            }
        }
        for (i, c) in made.iter().take(3).enumerate() {
            text.push_str(if i == 0 { "; made by " } else { " in " });
            text.push_str(&format!("{}() called at {}", c.name, c.called_at));
        }
        Some(Picked { node, ancestors, text })
    }
}

//! Shared semantic resolution. Names never fall back to editable values or visual hints.
//! Output adapters retain their own encoding (AccessKit static text uses a value).

use super::{Kernel, Kind, NodeId};

#[derive(Clone, Copy)]
pub(crate) enum Relation {
    ActiveDescendant,
    LabelledBy,
    Controls,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Relations<T> {
    pub(crate) active_descendant: Option<T>,
    pub(crate) labelled_by: Option<T>,
    pub(crate) controls: Option<T>,
}

impl<T> Default for Relations<T> {
    fn default() -> Self {
        Self { active_descendant: None, labelled_by: None, controls: None }
    }
}

impl<T> Relations<T> {
    fn is_empty(&self) -> bool {
        self.active_descendant.is_none() && self.labelled_by.is_none() && self.controls.is_none()
    }
}

impl Kernel {
    /// A node's name from its referenced label, authored label, or document text.
    pub(crate) fn semantic_name(&self, id: NodeId) -> Option<&str> {
        self.name_with_label(id, self.labelled_by(id))
    }

    pub(super) fn name_with_label(&self, id: NodeId, label: Option<NodeId>) -> Option<&str> {
        self.referenced_name(label).or_else(|| self.own_semantic_name(id))
    }

    pub(super) fn referenced_name(&self, label: Option<NodeId>) -> Option<&str> {
        self.own_semantic_name(label?)
    }

    /// Read authored content directly. A reference does not follow another label link.
    /// An explicit empty label suppresses the fallback to the node's text.
    fn own_semantic_name(&self, id: NodeId) -> Option<&str> {
        let n = self.node(id);
        n.label.as_deref().or_else(|| (n.kind() == Kind::Text && !n.text().is_empty()).then(|| n.text()))
    }

    fn relation_target(&self, owner: NodeId, name: Option<&str>) -> Option<NodeId> {
        let target = self.find(name?)?;
        (target != owner && self.a11y_reachable(target)).then_some(target)
    }

    fn labelled_by(&self, id: NodeId) -> Option<NodeId> {
        let label = self.relation_target(id, self.node(id).relations.labelled_by.as_deref())?;
        self.own_semantic_name(label).is_some_and(|name| !name.trim().is_empty()).then_some(label)
    }

    /// The unique, accessible option named by this node's active-descendant test id.
    pub fn active_descendant(&self, id: NodeId) -> Option<NodeId> {
        self.relation_target(id, self.node(id).relations.active_descendant.as_deref())
    }

    pub(crate) fn resolved_relations(&self, id: NodeId) -> Relations<NodeId> {
        Relations {
            active_descendant: self.active_descendant(id),
            labelled_by: self.labelled_by(id),
            controls: self.relation_target(id, self.node(id).relations.controls.as_deref()),
        }
    }

    pub(crate) fn set_relation(&mut self, id: NodeId, relation: Relation, target: Option<&str>) -> bool {
        let changed = match relation {
            Relation::ActiveDescendant => self.set_string(id, target, |n| &mut n.relations.active_descendant, "active_descendant"),
            Relation::LabelledBy => self.set_string(id, target, |n| &mut n.relations.labelled_by, "labelled_by"),
            Relation::Controls => self.set_string(id, target, |n| &mut n.relations.controls, "controls"),
        };
        if changed {
            let empty = self.node(id).relations.is_empty();
            let owners = self.a11y_relation_owners.get_mut();
            if empty {
                owners.remove(&id);
            } else {
                owners.entry(id).or_default();
            }
        }
        changed
    }

    /// The active option's test id. Its current instance is resolved when semantics are read.
    pub fn set_active_descendant(&mut self, id: NodeId, target: Option<&str>) -> bool {
        self.set_relation(id, Relation::ActiveDescendant, target)
    }

    /// The explicit description, else the tooltip unless it repeats the resolved name.
    pub fn description(&self, id: NodeId) -> Option<&str> {
        let n = self.node(id);
        n.description.as_deref().or_else(|| n.tooltip.as_deref().filter(|t| self.semantic_name(id) != Some(*t)))
    }

    /// Shown nodes whose resolved accessible name is `name`, in tree order.
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

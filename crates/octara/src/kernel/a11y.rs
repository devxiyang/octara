//! The accessibility tree (AccessKit) from the kernel's semantics: what assistive technology
//! (VoiceOver, Narrator, Orca, TalkBack) reads, and what agents can read too.

use super::{HandlerKind, Kernel, Kind, NodeId, Role, State};
use accesskit as ak;
use slotmap::{Key, KeyData};

/// The AccessKit id of a node.
pub fn a11y_id(id: NodeId) -> ak::NodeId {
    ak::NodeId(id.data().as_ffi())
}

impl Kernel {
    /// The node an AccessKit id names, if it still exists.
    pub fn node_of_a11y(&self, id: ak::NodeId) -> Option<NodeId> {
        let n = NodeId::from(KeyData::from_ffi(id.0));
        self.is_alive(n).then_some(n)
    }

    /// The whole tree as an AccessKit update. Bounds are in logical pixels; the window node
    /// carries the `scale` to device pixels. `focus` is the focused node (else the window).
    pub fn accessibility_tree(&self, focus: Option<NodeId>, scale: f64) -> ak::TreeUpdate {
        let mut nodes = Vec::new();
        self.a11y_node(self.root, &mut nodes);
        if let Some((_, root)) = nodes.iter_mut().find(|(id, _)| *id == a11y_id(self.root)) {
            root.set_transform(ak::Affine::scale(scale));
        }
        let focus = focus.filter(|n| self.is_alive(*n)).unwrap_or(self.root);
        ak::TreeUpdate { nodes, tree: Some(ak::TreeInfo::new(a11y_id(self.root))), tree_id: ak::TreeId::ROOT, focus: a11y_id(focus) }
    }

    fn shown_to_a11y(&self, id: NodeId) -> bool {
        let n = self.node(id);
        let empty_layer = id == self.overlay_layer && n.children.is_empty();
        !n.hidden && !empty_layer && n.layout.display != taffy::Display::None
    }

    fn a11y_node(&self, id: NodeId, out: &mut Vec<(ak::NodeId, ak::Node)>) {
        let n = self.node(id);
        let mut node = ak::Node::new(if id == self.root { Role::Window } else { self.role(id) });
        let (x, y, w, h) = self.shown_rect(id);
        node.set_bounds(ak::Rect { x0: x as f64, y0: y as f64, x1: (x + w) as f64, y1: (y + h) as f64 });
        // The name: an explicit label, else a text's own text.
        if let Some(label) = &n.label {
            node.set_label(label.as_str());
        } else if n.kind == Kind::Text && !n.text.is_empty() {
            node.set_label(n.text.as_str());
        }
        if let Some(d) = &n.description {
            node.set_description(d.as_str());
        }
        if n.kind == Kind::TextInput {
            node.set_value(n.value.as_str());
        }
        if let Some(c) = n.checked {
            node.set_toggled(if c { ak::Toggled::True } else { ak::Toggled::False });
        }
        if let Some(e) = n.expanded {
            node.set_expanded(e);
        }
        if n.state.contains(State::SELECTED) {
            node.set_selected(true);
        }
        if n.state.contains(State::DISABLED) {
            node.set_disabled();
        }
        if [HandlerKind::Press, HandlerKind::Toggle].iter().any(|k| n.handlers[*k as usize].is_some()) {
            node.add_action(ak::Action::Click);
        }
        if n.handlers[HandlerKind::SecondaryPress as usize].is_some() {
            node.add_action(ak::Action::ShowContextMenu);
        }
        if n.focusable {
            node.add_action(ak::Action::Focus);
        }
        if n.modal.is_some() {
            node.set_modal();
        }
        if let Some(t) = &n.test_id {
            node.set_author_id(t.as_str());
        }
        let children: Vec<NodeId> = n.children.iter().copied().filter(|c| self.shown_to_a11y(*c)).collect();
        node.set_children(children.iter().map(|c| a11y_id(*c)).collect::<Vec<_>>());
        out.push((a11y_id(id), node));
        for c in children {
            self.a11y_node(c, out);
        }
    }
}

impl Kernel {
    /// The semantic tree as the Inspector protocol gives it (see [`crate::inspect`]): the nodes
    /// assistive technology sees, with test ids, presented bounds, actions and source locations.
    pub fn inspect_tree(&self) -> crate::inspect::Node {
        self.inspect_node(self.root)
    }

    fn inspect_node(&self, id: NodeId) -> crate::inspect::Node {
        use crate::inspect::{snake, tenth};
        let n = self.node(id);
        let role = if id == self.root { Role::Window } else { self.role(id) };
        let name = n.label.clone().or_else(|| (n.kind == Kind::Text && !n.text.is_empty()).then(|| n.text.clone()));
        let mut states = Vec::new();
        for (flag, s) in [
            (State::HOVER, "hovered"),
            (State::FOCUSED, "focused"),
            (State::FOCUS_VISIBLE, "focus_visible"),
            (State::PRESSED, "pressed"),
            (State::SELECTED, "selected"),
            (State::DISABLED, "disabled"),
        ] {
            if n.state.contains(flag) {
                states.push(s.to_string());
            }
        }
        let mut actions = Vec::new();
        for (kind, a) in [
            (HandlerKind::Press, "press"),
            (HandlerKind::Toggle, "toggle"),
            (HandlerKind::DoublePress, "double_press"),
            (HandlerKind::SecondaryPress, "secondary_press"),
            (HandlerKind::LongPress, "long_press"),
            (HandlerKind::Drag, "drag"),
            (HandlerKind::Input, "input"),
            (HandlerKind::Submit, "submit"),
        ] {
            if n.handlers[kind as usize].is_some() {
                actions.push(a.to_string());
            }
        }
        if n.kind == Kind::TextInput && !actions.iter().any(|a| a == "input") {
            actions.push("input".into());
        }
        if n.focusable {
            actions.push("focus".into());
        }
        if self.scrollable(id) != (false, false) {
            actions.push("scroll".into());
        }
        let (x, y, w, h) = self.shown_rect(id);
        let children = n.children.iter().copied().filter(|c| self.shown_to_a11y(*c)).map(|c| self.inspect_node(c)).collect();
        crate::inspect::Node {
            id: n.test_id.clone(),
            role: snake(&format!("{role:?}")),
            name,
            value: (n.kind == Kind::TextInput).then(|| n.value.clone()),
            checked: n.checked,
            expanded: n.expanded,
            states,
            bounds: [tenth(x), tenth(y), tenth(w), tenth(h)],
            actions,
            commands: n.commands.iter().map(|(c, _)| c.clone()).collect(),
            icon: n.icon.as_ref().map(|i| i.name.clone()),
            source: n.origin.map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())),
            children,
        }
    }
}

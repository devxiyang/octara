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

    /// The whole tree as an AccessKit update (a snapshot: it does not change what
    /// [`accessibility_update`](Self::accessibility_update) sends next). `focus` is the focused
    /// node (else the window); the window node carries the `scale` to device pixels. Each node's
    /// transform places it in its parent (a scroll container's offset is its own), and its bounds
    /// are in its own space.
    pub fn accessibility_tree(&self, focus: Option<NodeId>, scale: f64) -> ak::TreeUpdate {
        let mut nodes = Vec::new();
        self.a11y_subtree(self.root, scale, &mut nodes, &mut |_, _| {});
        let focus = focus.filter(|n| self.is_alive(*n)).unwrap_or(self.root);
        ak::TreeUpdate { nodes, tree: Some(ak::TreeInfo::new(a11y_id(self.root))), tree_id: ak::TreeId::ROOT, focus: a11y_id(focus) }
    }

    /// What changed for assistive technology since the last update: the nodes whose semantics,
    /// children or place changed, and subtrees new to it. The first update (and the first after
    /// [`restart_accessibility`](Self::restart_accessibility)) is the whole tree.
    pub fn accessibility_update(&self, focus: Option<NodeId>, scale: f64) -> ak::TreeUpdate {
        let focus = a11y_id(focus.filter(|n| self.is_alive(*n)).unwrap_or(self.root));
        if !self.a11y_live.get() || self.a11y_scale.get() != scale {
            self.a11y_live.set(true);
            self.a11y_scale.set(scale);
            self.a11y_dirty.borrow_mut().clear();
            let mut nodes = Vec::new();
            self.a11y_subtree(self.root, scale, &mut nodes, &mut |id, g| self.node(id).a11y_sent.set(Some(g)));
            #[cfg(debug_assertions)]
            {
                *self.a11y_mirror.borrow_mut() = nodes.iter().cloned().collect();
            }
            return ak::TreeUpdate { nodes, tree: Some(ak::TreeInfo::new(a11y_id(self.root))), tree_id: ak::TreeId::ROOT, focus };
        }
        let mut dirty: Vec<NodeId> = self.a11y_dirty.borrow_mut().drain().collect();
        // A layout (or motion) may have moved anything: compare each sent node's place.
        if self.a11y_geometry.replace(false) || !self.at_rest() {
            self.a11y_moved(self.root, &mut dirty);
        }
        dirty.sort();
        dirty.dedup();
        let mut nodes = Vec::new();
        let mut sent = rustc_hash::FxHashSet::default();
        for id in dirty {
            if self.is_alive(id) && self.a11y_reachable(id) && !sent.contains(&id) {
                self.a11y_emit(id, scale, &mut nodes, &mut sent);
            }
        }
        #[cfg(debug_assertions)]
        self.a11y_verify(&nodes, scale);
        ak::TreeUpdate { nodes, tree: None, tree_id: ak::TreeId::ROOT, focus }
    }

    /// The next [`accessibility_update`](Self::accessibility_update) sends the whole tree (the
    /// platform asked for it again).
    pub fn restart_accessibility(&self) {
        self.a11y_live.set(false);
    }

    /// Sends `id` and the subtrees of its children assistive technology has not seen.
    fn a11y_emit(&self, id: NodeId, scale: f64, out: &mut Vec<(ak::NodeId, ak::Node)>, sent: &mut rustc_hash::FxHashSet<NodeId>) {
        let (node, g, children) = self.a11y_build(id, scale);
        self.node(id).a11y_sent.set(Some(g));
        sent.insert(id);
        out.push((a11y_id(id), node));
        for c in children {
            if self.node(c).a11y_sent.get().is_none() && !sent.contains(&c) {
                self.a11y_emit(c, scale, out, sent);
            }
        }
    }

    /// Nodes already sent whose place changed.
    fn a11y_moved(&self, id: NodeId, out: &mut Vec<NodeId>) {
        let n = self.node(id);
        let Some(was) = n.a11y_sent.get() else { return };
        if was != self.a11y_geometry_of(id) {
            out.push(id);
        }
        for &c in &n.children {
            if self.shown_to_a11y(c) {
                self.a11y_moved(c, out);
            }
        }
    }

    /// Whether `id` is in the tree assistive technology sees (shown, and its ancestors too).
    fn a11y_reachable(&self, id: NodeId) -> bool {
        let mut at = id;
        loop {
            if at != self.root && !self.shown_to_a11y(at) {
                return false;
            }
            match self.node(at).parent {
                Some(p) => at = p,
                None => return at == self.root,
            }
        }
    }

    /// A node's place in its parent's space and its scroll: what its transform and bounds come
    /// from.
    fn a11y_geometry_of(&self, id: NodeId) -> [f32; 6] {
        let (x, y, w, h) = if id == self.root { (0.0, 0.0, self.viewport.0, self.viewport.1) } else { self.shown_frame(id) };
        let (sx, sy) = self.node(id).scroll;
        [x, y, w, h, sx, sy]
    }

    /// `id` and its shown descendants, with `seen` called for each node and its geometry.
    fn a11y_subtree(&self, id: NodeId, scale: f64, out: &mut Vec<(ak::NodeId, ak::Node)>, seen: &mut dyn FnMut(NodeId, [f32; 6])) {
        let (node, g, children) = self.a11y_build(id, scale);
        seen(id, g);
        out.push((a11y_id(id), node));
        for c in children {
            self.a11y_subtree(c, scale, out, seen);
        }
    }

    /// Debug builds: what was sent so far, applied in order, is the whole tree now.
    #[cfg(debug_assertions)]
    fn a11y_verify(&self, update: &[(ak::NodeId, ak::Node)], scale: f64) {
        let mut mirror = self.a11y_mirror.borrow_mut();
        for (id, n) in update {
            mirror.insert(*id, n.clone());
        }
        let mut full = Vec::new();
        self.a11y_subtree(self.root, scale, &mut full, &mut |_, _| {});
        for (id, n) in &full {
            debug_assert_eq!(
                mirror.get(id),
                Some(n),
                "assistive technology was not told about a change to {:?}",
                self.node_of_a11y(*id).map(|n| self.describe(n))
            );
        }
        let live: rustc_hash::FxHashSet<ak::NodeId> = full.iter().map(|(id, _)| *id).collect();
        mirror.retain(|id, _| live.contains(id));
    }

    pub(crate) fn shown_to_a11y(&self, id: NodeId) -> bool {
        let n = self.node(id);
        let empty_layer = id == self.overlay_layer && n.children.is_empty();
        !n.hidden && !empty_layer && n.layout.display != taffy::Display::None
    }

    /// One node for AccessKit, its geometry, and its shown children.
    fn a11y_build(&self, id: NodeId, scale: f64) -> (ak::Node, [f32; 6], Vec<NodeId>) {
        let n = self.node(id);
        let mut node = ak::Node::new(if id == self.root { Role::Window } else { self.role(id) });
        // Placed in the parent's space; a scroll container's space moves with its offset, so its
        // children keep their transforms while it scrolls.
        let g = self.a11y_geometry_of(id);
        let [x, y, w, h, sx, sy] = g.map(f64::from);
        let place = ak::Affine::translate((x - sx, y - sy));
        node.set_transform(if id == self.root { ak::Affine::scale(scale) * place } else { place });
        node.set_bounds(ak::Rect { x0: sx, y0: sy, x1: sx + w, y1: sy + h });
        // The name: an explicit label, else a text's own text.
        if let Some(label) = &n.label {
            node.set_label(label.as_str());
        } else if n.kind() == Kind::Text && !n.text().is_empty() {
            node.set_label(n.text());
        }
        if let Some(d) = &n.description {
            node.set_description(d.as_str());
        }
        // A text input's content, or the value set for other controls (a combo box's choice).
        if n.kind() == Kind::TextInput || n.value.is_some() {
            // A password is bullets to assistive technology (AccessKit's contract), unless shown.
            node.set_value(if n.input().is_some_and(|i| i.masked()) { self.reported_value(id) } else { self.value(id).to_owned() });
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
        if [HandlerKind::Press, HandlerKind::Toggle].iter().any(|k| n.handlers[*k as usize].is_some()) || n.kind() == Kind::Link {
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
        (node, g, children)
    }
}

impl Kernel {
    /// The semantic tree as the Inspector protocol gives it (see [`crate::inspect`]): the nodes
    /// assistive technology sees, with test ids, presented bounds, actions and source locations.
    pub fn inspect_tree(&self) -> crate::inspect::Node {
        self.inspect_node(self.root)
    }

    /// What can be done to a node, as the inspector names it.
    pub(crate) fn node_actions(&self, id: NodeId) -> Vec<String> {
        let n = self.node(id);
        let mut actions = Vec::new();
        if n.kind() == Kind::Link {
            actions.push("press".to_string());
        }
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
        if n.kind() == Kind::TextInput && !actions.iter().any(|a| a == "input") {
            actions.push("input".into());
        }
        if n.focusable {
            actions.push("focus".into());
        }
        if self.scrollable(id) != (false, false) {
            actions.push("scroll".into());
        }
        actions
    }

    /// One node of the inspector's tree, without its children.
    pub(crate) fn inspect_one(&self, id: NodeId) -> crate::inspect::Node {
        use crate::inspect::{snake, tenth};
        let n = self.node(id);
        let role = if id == self.root { Role::Window } else { self.role(id) };
        let name = n.label.clone().or_else(|| (n.kind() == Kind::Text && !n.text().is_empty()).then(|| n.text().to_owned()));
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
        let (x, y, w, h) = self.shown_rect(id);
        crate::inspect::Node {
            id: n.test_id.clone(),
            role: snake(&format!("{role:?}")),
            name,
            value: (n.kind() == Kind::TextInput || n.value.is_some()).then(|| self.reported_value(id)),
            checked: n.checked,
            expanded: n.expanded,
            states,
            bounds: [tenth(x), tenth(y), tenth(w), tenth(h)],
            actions: self.node_actions(id),
            commands: n.commands.iter().map(|(c, _)| c.clone()).collect(),
            native: self.native(id).map(|v| crate::native::NativeInfo {
                kind: v.kind.clone(),
                slot: snake(&format!("{:?}", v.slot)),
                props: v.props.clone(),
            }),
            icon: match &n.content {
                super::Content::Icon(i) => Some(i.name.clone()),
                _ => None,
            },
            source: self.source_of(id),
            component: n.component.map(crate::inspect::ComponentInfo::of),
            children: Vec::new(),
        }
    }

    fn inspect_node(&self, id: NodeId) -> crate::inspect::Node {
        let mut node = self.inspect_one(id);
        if self.node(id).parent.is_some_and(|p| self.node(p).component == self.node(id).component) {
            node.component = None;
        }
        node.children = self.node(id).children.iter().copied().filter(|c| self.shown_to_a11y(*c)).map(|c| self.inspect_node(c)).collect();
        node
    }
}

//! Focus structure: Tab order (with `tab_index` and focus groups), focus groups' arrow-key
//! movement, and modals (Rune's model).

use super::{Kernel, NodeId, State};
use rustc_hash::{FxHashMap, FxHashSet};

/// How arrow keys move focus inside a focus group.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Arrow {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
}

#[derive(Clone, Copy, Debug)]
enum Axis {
    Horizontal,
    Vertical,
    Both,
}

/// Keyboard navigation for a container with a single Tab stop.
///
/// The group skips hidden and disabled members. Home and End move to the first and last
/// member. Tab enters at the preferred member, then the remembered member, then the first
/// eligible stop. An already focused member remains the group's stop.
#[derive(Clone, Copy, Debug)]
pub struct FocusGroup {
    axis: Axis,
    wraps: bool,
    activates: bool,
}

impl FocusGroup {
    const fn new(axis: Axis) -> Self {
        Self { axis, wraps: false, activates: false }
    }

    /// Move with Left and Right.
    pub const fn horizontal() -> Self {
        Self::new(Axis::Horizontal)
    }

    /// Move with Up and Down.
    pub const fn vertical() -> Self {
        Self::new(Axis::Vertical)
    }

    /// Move with either pair of arrow keys.
    pub const fn both() -> Self {
        Self::new(Axis::Both)
    }

    /// Continue at the opposite edge when an arrow reaches the end.
    pub const fn wrap(mut self) -> Self {
        self.wraps = true;
        self
    }

    /// After group navigation settles focus, press the reached item if it is still eligible.
    /// Reaching the already focused, preferred item leaves it as is. Tab only moves focus.
    pub const fn activate_on_navigation(mut self) -> Self {
        self.activates = true;
        self
    }
}

pub(crate) enum InteractionError {
    Hidden,
    BehindModal(NodeId),
    Disabled,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct LayerPolicy {
    pub modal: bool,
    pub click_outside: bool,
}

impl InteractionError {
    pub(crate) fn message(self, kernel: &Kernel) -> String {
        match self {
            Self::Hidden => "is hidden or leaving the tree".into(),
            Self::BehindModal(modal) => format!("is behind the open modal {}", kernel.describe(modal)),
            Self::Disabled => "is disabled".into(),
        }
    }
}

impl Kernel {
    pub fn set_tab_index(&mut self, id: NodeId, i: i32) {
        self.node_mut(id).tab_index = i;
    }
    pub fn set_focus_group(&mut self, id: NodeId, group: FocusGroup) {
        self.node_mut(id).focus_group = Some(group);
    }
    pub(crate) fn set_focus_preferred(&mut self, id: NodeId, preferred: bool) -> bool {
        let n = self.node_mut(id);
        if n.focus_preferred == preferred {
            return false;
        }
        n.focus_preferred = preferred;
        self.mark(super::Dirty::empty());
        true
    }
    pub fn set_autofocus(&mut self, id: NodeId, on: bool) {
        self.node_mut(id).autofocus = on;
    }
    pub(crate) fn set_interaction_layer(&mut self, id: NodeId, layer: LayerPolicy) {
        if self.node(id).layer != Some(layer) {
            self.node_mut(id).layer = Some(layer);
            self.layer_nodes.insert(id);
            self.a11y_touch(id);
            self.mark(super::Dirty::SEMANTICS);
        }
    }
    pub(crate) fn dismiss_on_outside(&self, id: NodeId) -> bool {
        self.node(id).layer.is_some_and(|layer| layer.click_outside)
    }

    /// The authored owner of an anchored nonmodal layer. The separate placement anchor
    /// may name a different element.
    pub(crate) fn anchored_layer_owner(&self, id: NodeId) -> Option<NodeId> {
        if self.node(id).layer.is_none_or(|layer| layer.modal) {
            return None;
        }
        let mut at = id;
        loop {
            let node = self.node(at);
            if at != id && node.layer.is_some() {
                return None;
            }
            if let Some(mount) = node.overlay_mount {
                return node.anchor.and(Some(mount.owner));
            }
            at = node.parent?;
        }
    }

    /// A nonmodal layer treats its own panel and the authored trigger's physical
    /// subtree as inside. Independently anchored sibling panels keep their own scope.
    pub(crate) fn is_in_dismissal_scope(&self, node: NodeId, layer: NodeId) -> bool {
        self.is_alive(node)
            && (self.is_in_interaction_scope(node, layer)
                || self.anchored_layer_owner(layer).is_some_and(|owner| self.is_shown(owner) && self.is_within(node, owner)))
    }

    /// Anchored nonmodal layers containing this focus, from the nearest panel outward.
    /// Teardown can still read these allocations before their nodes are destroyed.
    pub(crate) fn anchored_focus_layers(&self, focused: NodeId) -> Vec<(NodeId, NodeId)> {
        let mut seen = FxHashSet::default();
        let mut layers = Vec::new();
        let mut node = Some(focused);
        while let Some(id) = node.filter(|id| self.is_alive(*id) && seen.insert(*id)) {
            if let Some(owner) = self.anchored_layer_owner(id) {
                layers.push((id, owner));
            }
            node = self.interaction_parent(id);
        }
        layers
    }

    fn shown(&self, id: NodeId) -> bool {
        self.node(id).layout.display != taffy::Display::None && self.link_is_visible(id)
    }

    /// Still mounted and displayed, including every ancestor, and not leaving the tree.
    pub(crate) fn is_shown(&self, id: NodeId) -> bool {
        self.is_alive(id) && !self.is_exiting(id) && self.shown_path(id).is_some()
    }

    /// Whether a node belongs to the displayed, interactive part of the window.
    pub(crate) fn check_interaction_scope(&self, id: NodeId, modal: Option<NodeId>) -> Result<(), InteractionError> {
        if !self.is_shown(id) {
            return Err(InteractionError::Hidden);
        }
        if let Some(modal) = modal
            && !self.is_in_modal_scope(id, modal)
        {
            return Err(InteractionError::BehindModal(modal));
        }
        Ok(())
    }

    /// Eligibility for direct activation, before operation-specific geometry or focus checks.
    pub(crate) fn check_activation(&self, id: NodeId, modal: Option<NodeId>) -> Result<(), InteractionError> {
        self.check_interaction_scope(id, modal)?;
        if self.state(id).contains(State::DISABLED) {
            return Err(InteractionError::Disabled);
        }
        Ok(())
    }

    /// Whether `node` is `ancestor` or inside it.
    pub fn is_within(&self, mut node: NodeId, ancestor: NodeId) -> bool {
        loop {
            if node == ancestor {
                return true;
            }
            match self.parent(node) {
                Some(p) => node = p,
                None => return false,
            }
        }
    }

    /// Shown modals, outermost first (tree order: overlays come after the page, a modal inside
    /// another after it).
    pub fn modals(&self) -> Vec<NodeId> {
        let mut modals: Vec<_> = self.layer_paths().filter(|&(_, id)| self.is_modal_layer(id)).collect();
        modals.sort_unstable();
        modals.into_iter().map(|(_, id)| id).collect()
    }

    pub(crate) fn front_modal(&self) -> Option<NodeId> {
        self.layer_paths().filter(|&(_, id)| self.is_modal_layer(id)).max().map(|(_, id)| id)
    }

    /// The highest dismissible layer allowed by the current modal barrier.
    pub(crate) fn dismissal_layer(&self, modal: Option<NodeId>) -> Option<NodeId> {
        self.layer_paths().filter(|&(_, layer)| modal.is_none_or(|modal| self.is_in_modal_scope(layer, modal))).max().map(|(_, id)| id)
    }

    fn is_modal_layer(&self, id: NodeId) -> bool {
        self.node(id).layer.is_some_and(|layer| layer.modal)
    }

    fn layer_paths(&self) -> impl Iterator<Item = (Vec<usize>, NodeId)> + '_ {
        // Paint and hit testing walk siblings in this same physical order. Overlay ownership
        // participates in eligibility, independently of where a layer is drawn.
        self.layer_nodes.iter().filter(|&&id| !self.is_exiting(id)).filter_map(|&id| self.shown_path(id).map(|p| (p, id)))
    }

    /// The child indices from the root to `id`, if it and every ancestor are shown and it is in
    /// the window's tree.
    fn shown_path(&self, id: NodeId) -> Option<Vec<usize>> {
        let mut path = Vec::new();
        let mut at = id;
        loop {
            if !self.shown(at) {
                return None;
            }
            let Some(p) = self.node(at).parent else { break };
            path.push(self.node(p).children().iter().position(|c| *c == at)?);
            at = p;
        }
        (at == self.root).then(|| {
            path.reverse();
            path
        })
    }

    fn walk_shown(&self, id: NodeId, f: &mut impl FnMut(NodeId)) {
        if !self.shown(id) {
            return;
        }
        f(id);
        for &c in self.node(id).children() {
            self.walk_shown(c, f);
        }
    }

    fn takes_focus(&self, id: NodeId) -> bool {
        let n = self.node(id);
        n.focusable && !n.state.contains(State::DISABLED)
    }

    /// Visit eligible controls in physical tree order, including separately mounted overlays.
    fn walk_focusable(&self, scope: Option<NodeId>, f: &mut impl FnMut(NodeId)) {
        if scope.is_some_and(|id| !self.is_shown(id)) {
            return;
        }
        self.walk_shown(self.root, &mut |id| {
            if self.takes_focus(id) && scope.is_none_or(|scope| self.is_in_interaction_scope(id, scope)) {
                f(id);
            }
        });
    }

    fn walk_tab_tree(
        &self,
        id: NodeId,
        following: &FxHashMap<NodeId, Vec<NodeId>>,
        relocated: &FxHashSet<NodeId>,
        f: &mut impl FnMut(NodeId),
    ) {
        if !self.shown(id) {
            return;
        }
        f(id);
        for &child in self.node(id).children() {
            if !relocated.contains(&child) {
                self.walk_tab_tree(child, following, relocated, f);
            }
        }
        for &panel in following.get(&id).into_iter().flatten() {
            self.walk_tab_tree(panel, following, relocated, f);
        }
    }

    /// The nearest focus group around `id` (not `id` itself).
    fn group_of(&self, id: NodeId) -> Option<NodeId> {
        let mut p = self.parent(id);
        while let Some(g) = p {
            if self.node(g).focus_group.is_some() {
                return Some(g);
            }
            p = self.parent(g);
        }
        None
    }

    /// Focus moved to `id`: its group remembers it (Tab comes back to it).
    pub fn focused_in_group(&mut self, id: NodeId) {
        if let Some(group) = self.group_of(id) {
            self.node_mut(group).group_current = Some(id);
        }
    }

    /// The focusable members owned by this group, in tree order.
    fn group_members(&self, group: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        for &c in self.node(group).children() {
            self.walk_shown(c, &mut |id| {
                if self.takes_focus(id) && self.group_of(id) == Some(group) {
                    out.push(id);
                }
            });
        }
        out
    }

    /// The Tab order inside `scope` (the open modal, else the window): focusable, enabled, shown
    /// nodes; positive `tab_index` first, then tree order with nonmodal panels following their
    /// trigger; negative ones skipped.
    /// A group stops at its focused member, preferred stop, remembered stop, or first stop.
    pub fn focus_order(&self, scope: Option<NodeId>, focused: Option<NodeId>) -> Vec<NodeId> {
        if scope.is_some_and(|id| !self.is_shown(id)) {
            return Vec::new();
        }
        let mut all = Vec::new();
        let mut panels: Vec<_> = self
            .layer_paths()
            .filter_map(|(path, layer)| {
                let owner = self.anchored_layer_owner(layer)?;
                self.is_shown(owner).then_some((path, layer, owner))
            })
            .collect();
        if panels.is_empty() {
            self.walk_focusable(scope, &mut |id| {
                if self.node(id).tab_index >= 0 {
                    all.push(id);
                }
            });
        } else {
            panels.sort_unstable();
            let mut following: FxHashMap<NodeId, Vec<NodeId>> = FxHashMap::default();
            let mut relocated = FxHashSet::default();
            for (_, layer, owner) in panels {
                following.entry(owner).or_default().push(layer);
                relocated.insert(layer);
            }
            self.walk_tab_tree(self.root, &following, &relocated, &mut |id| {
                if self.takes_focus(id)
                    && self.node(id).tab_index >= 0
                    && scope.is_none_or(|within| self.is_in_interaction_scope(id, within))
                {
                    all.push(id);
                }
            });
        }
        // One stop per group.
        let mut stops: Vec<NodeId> = Vec::new();
        let mut groups_seen: Vec<NodeId> = Vec::new();
        for id in all {
            match self.group_of(id).filter(|&group| scope.is_none_or(|scope| self.is_in_interaction_scope(group, scope))) {
                None => stops.push(id),
                Some(g) if groups_seen.contains(&g) => {}
                Some(g) => {
                    groups_seen.push(g);
                    let mut members = self.group_members(g);
                    if let Some(scope) = scope {
                        members.retain(|&member| self.is_in_interaction_scope(member, scope));
                    }
                    let is_stop = |m| self.node(m).tab_index >= 0;
                    let pick = focused
                        .filter(|f| members.contains(f))
                        .or_else(|| members.iter().copied().find(|&m| is_stop(m) && self.node(m).focus_preferred))
                        .or_else(|| self.node(g).group_current.filter(|&c| members.contains(&c) && is_stop(c)))
                        .unwrap_or(id);
                    stops.push(pick);
                }
            }
        }
        // Stable: positive indices in order, then the zeros in tree order.
        stops.sort_by_key(|id| match self.node(*id).tab_index {
            0 => i32::MAX,
            i => i,
        });
        stops
    }

    /// Where an arrow key moves focus from `focused` inside its focus group, if it does.
    pub fn group_step(&self, focused: NodeId, arrow: Arrow) -> Option<NodeId> {
        let g = self.group_of(focused)?;
        let policy = self.node(g).focus_group?;
        let members = self.group_members(g);
        let at = members.iter().position(|m| *m == focused)?;
        let last = members.len() - 1;
        let step = |forward: bool| -> Option<usize> {
            match (forward, at) {
                (true, i) if i == last => policy.wraps.then_some(0),
                (true, i) => Some(i + 1),
                (false, 0) => policy.wraps.then_some(last),
                (false, i) => Some(i - 1),
            }
        };
        let to = match arrow {
            Arrow::Home => Some(0),
            Arrow::End => Some(last),
            Arrow::Down if !matches!(policy.axis, Axis::Horizontal) => step(true),
            Arrow::Up if !matches!(policy.axis, Axis::Horizontal) => step(false),
            Arrow::Right if !matches!(policy.axis, Axis::Vertical) => step(true),
            Arrow::Left if !matches!(policy.axis, Axis::Vertical) => step(false),
            Arrow::Down | Arrow::Up | Arrow::Left | Arrow::Right => None,
        }?;
        Some(members[to])
    }

    pub(crate) fn group_activation(&self, from: NodeId, to: NodeId) -> Option<NodeId> {
        if from == to && self.node(to).focus_preferred {
            return None;
        }
        let group = self.group_of(from)?;
        self.activating_focus_group(to).filter(|&owner| owner == group)
    }

    pub(crate) fn activating_focus_group(&self, id: NodeId) -> Option<NodeId> {
        self.group_of(id).filter(|&group| self.node(group).focus_group.is_some_and(|policy| policy.activates))
    }

    /// What a modal focuses when it opens: its `autofocus` element, else its first text field,
    /// else its first Tab stop.
    pub fn initial_focus(&self, modal: NodeId) -> Option<NodeId> {
        let mut auto = None;
        let mut field = None;
        self.walk_focusable(Some(modal), &mut |id| {
            if auto.is_none() && self.node(id).autofocus {
                auto = Some(id);
            }
            if field.is_none() && self.node(id).input().is_some() {
                field = Some(id);
            }
        });
        auto.or(field).or_else(|| self.focus_order(Some(modal), None).first().copied())
    }

    /// Names `id` a hover group: descendants style themselves on its hover (`group_hover`).
    pub fn set_group(&mut self, id: NodeId, name: Option<&'static str>) {
        self.node_mut(id).group = name;
    }
    /// While `id` is hovered, the elements around it are not (`hover_alone`).
    pub fn set_hover_alone(&mut self, id: NodeId, on: bool) {
        self.node_mut(id).hover_alone = on;
    }

    /// Whether an ancestor of `id` named `group` is hovered.
    pub(crate) fn group_hovered(&self, id: NodeId, group: &str) -> bool {
        let mut p = self.parent(id);
        while let Some(a) = p {
            let n = self.node(a);
            if n.group == Some(group) {
                return n.state.contains(State::HOVER);
            }
            p = n.parent;
        }
        false
    }

    /// Resolves the descendants of `group_node` styled on group `name`.
    pub(crate) fn resolve_group_members(&mut self, group_node: NodeId, name: &'static str) {
        let mut stack: Vec<NodeId> = self.node(group_node).children().to_vec();
        while let Some(id) = stack.pop() {
            let n = self.node(id);
            let member = n.overrides.iter().any(|(w, _)| *w == super::When::GroupHover(name));
            stack.extend(n.children().iter().copied());
            if member {
                self.resolve(id, "group hover");
            }
        }
    }

    /// What a pointer over `hit` hovers: it and its ancestors, up to the innermost `hover_alone`
    /// one (the elements around that show no hover, as in Rune).
    pub fn hover_chain(&self, hit: Option<NodeId>) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut n = hit;
        while let Some(id) = n {
            if id == self.root || id == self.overlay_layer {
                break;
            }
            out.push(id);
            if self.node(id).hover_alone {
                break;
            }
            n = self.parent(id);
        }
        out
    }

    pub fn set_key_context(&mut self, id: NodeId, context: Option<&str>) {
        self.node_mut(id).key_context = context.map(str::to_owned);
    }
    /// The key context `id` adds: its own, else `text_input` for a text input.
    pub fn key_context(&self, id: NodeId) -> Option<&str> {
        let n = self.node(id);
        n.key_context.as_deref().or(match n.input() {
            Some(i) if i.multiline && !i.submit_on_enter => Some("text_area"),
            Some(_) => Some("text_input"),
            None => None,
        })
    }
    pub fn add_command(&mut self, id: NodeId, command: &str, handler: super::HandlerId) {
        self.node_mut(id).commands.push((command.to_owned(), handler));
    }
    /// `id`'s handler for `command`, if it has one.
    pub fn command_handler(&self, id: NodeId, command: &str) -> Option<super::HandlerId> {
        self.node(id).commands.iter().rev().find(|(c, _)| c == command).map(|(_, h)| *h)
    }
    /// Command ids the elements on the path handle.
    pub fn commands_on(&self, path: &[NodeId]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for id in path {
            for (c, _) in &self.node(*id).commands {
                if !out.contains(c) {
                    out.push(c.clone());
                }
            }
        }
        out
    }
}

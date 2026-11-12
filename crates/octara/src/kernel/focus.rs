//! Focus structure: Tab order (with `tab_index` and focus groups), focus groups' arrow-key
//! movement, and modals (Rune's model).

use super::{Kernel, Kind, NodeId, Role, State};

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

impl Kernel {
    pub fn set_tab_index(&mut self, id: NodeId, i: i32) {
        self.node_mut(id).tab_index = i;
    }
    pub fn set_focus_group(&mut self, id: NodeId, on: bool) {
        self.node_mut(id).focus_group = on;
    }
    pub fn set_autofocus(&mut self, id: NodeId, on: bool) {
        self.node_mut(id).autofocus = on;
    }
    /// Makes `id` a modal (`Some(click_outside)`) or not; changes what assistive technology
    /// reads (the rest of the window is inert).
    pub fn set_modal(&mut self, id: NodeId, modal: Option<bool>) {
        if self.node(id).modal != modal {
            self.node_mut(id).modal = modal;
            if modal.is_some() {
                self.modal_nodes.insert(id);
            } else {
                self.modal_nodes.remove(&id);
            }
            self.a11y_touch(id);
            self.mark(super::Dirty::SEMANTICS);
        }
    }
    pub fn modal_click_outside(&self, id: NodeId) -> bool {
        self.node(id).modal == Some(true)
    }

    fn shown(&self, id: NodeId) -> bool {
        self.node(id).layout.display != taffy::Display::None
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
        // The modal nodes are known: keep those shown in the window, in tree order (each one's
        // path of child indices from the root).
        let mut out: Vec<(Vec<usize>, NodeId)> = self.modal_nodes.iter().filter_map(|&id| self.shown_path(id).map(|p| (p, id))).collect();
        out.sort();
        out.into_iter().map(|(_, id)| id).collect()
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
            path.push(self.node(p).children.iter().position(|c| *c == at)?);
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
        for &c in &self.node(id).children {
            self.walk_shown(c, f);
        }
    }

    fn takes_focus(&self, id: NodeId) -> bool {
        let n = self.node(id);
        n.focusable && !n.state.contains(State::DISABLED)
    }

    /// The nearest focus group around `id` (not `id` itself).
    fn group_of(&self, id: NodeId) -> Option<NodeId> {
        let mut p = self.parent(id);
        while let Some(g) = p {
            if self.node(g).focus_group {
                return Some(g);
            }
            p = self.parent(g);
        }
        None
    }

    /// Focus moved to `id`: the focus groups around it remember it (Tab comes back to it).
    pub fn focused_in_group(&mut self, id: NodeId) {
        let mut p = self.parent(id);
        while let Some(g) = p {
            if self.node(g).focus_group {
                self.node_mut(g).group_current = Some(id);
            }
            p = self.parent(g);
        }
    }

    /// The focusable members of a focus group, in tree order (nested groups' members included).
    fn group_members(&self, group: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        for &c in &self.node(group).children {
            self.walk_shown(c, &mut |id| {
                if self.takes_focus(id) {
                    out.push(id);
                }
            });
        }
        out
    }

    /// The Tab order inside `scope` (the open modal, else the window): focusable, enabled, shown
    /// nodes; positive `tab_index` first, then the rest in tree order; negative ones skipped.
    /// A focus group is one stop: its focused member, else its selected one, else its first.
    pub fn focus_order(&self, scope: Option<NodeId>, focused: Option<NodeId>) -> Vec<NodeId> {
        let mut all = Vec::new();
        self.walk_shown(scope.unwrap_or(self.root), &mut |id| {
            if self.takes_focus(id) && self.node(id).tab_index >= 0 {
                all.push(id);
            }
        });
        // One stop per group.
        let mut stops: Vec<NodeId> = Vec::new();
        let mut groups_seen: Vec<NodeId> = Vec::new();
        for id in all {
            match self.group_of(id).filter(|g| scope.is_none_or(|s| self.is_within(*g, s))) {
                None => stops.push(id),
                Some(g) if groups_seen.contains(&g) => {}
                Some(g) => {
                    groups_seen.push(g);
                    let members = self.group_members(g);
                    let pick = focused
                        .filter(|f| members.contains(f))
                        .or_else(|| self.node(g).group_current.filter(|c| members.contains(c)))
                        .or_else(|| members.iter().copied().find(|m| self.node(*m).state.contains(State::SELECTED)))
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
    /// Menus, lists and trees move on Up and Down only; other groups (tabs, toolbars, radio
    /// groups, segments) on Left and Right as well. Lists, trees and grids stop at their ends;
    /// the others wrap.
    pub fn group_step(&self, focused: NodeId, arrow: Arrow) -> Option<NodeId> {
        let g = self.group_of(focused)?;
        let members = self.group_members(g);
        let at = members.iter().position(|m| *m == focused)?;
        let role = self.role(g);
        let vertical = matches!(role, Role::Menu | Role::ListBox | Role::List | Role::Tree | Role::TreeGrid | Role::Grid);
        let stops = matches!(role, Role::ListBox | Role::List | Role::Tree | Role::TreeGrid | Role::Grid);
        let last = members.len() - 1;
        let step = |forward: bool| -> Option<usize> {
            match (forward, at) {
                (true, i) if i == last => (!stops).then_some(0),
                (true, i) => Some(i + 1),
                (false, 0) => (!stops).then_some(last),
                (false, i) => Some(i - 1),
            }
        };
        let to = match arrow {
            Arrow::Home => Some(0),
            Arrow::End => Some(last),
            Arrow::Down => step(true),
            Arrow::Up => step(false),
            Arrow::Right if !vertical => step(true),
            Arrow::Left if !vertical => step(false),
            Arrow::Left | Arrow::Right => None,
        }?;
        Some(members[to])
    }

    /// What a modal focuses when it opens: its `autofocus` element, else its first text field,
    /// else its first Tab stop.
    pub fn initial_focus(&self, modal: NodeId) -> Option<NodeId> {
        let mut auto = None;
        let mut field = None;
        self.walk_shown(modal, &mut |id| {
            if self.takes_focus(id) {
                if auto.is_none() && self.node(id).autofocus {
                    auto = Some(id);
                }
                if field.is_none() && self.node(id).input().is_some() {
                    field = Some(id);
                }
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
        let mut stack: Vec<NodeId> = self.node(group_node).children.clone();
        while let Some(id) = stack.pop() {
            let n = self.node(id);
            let member = n.overrides.iter().any(|(w, _)| *w == super::When::GroupHover(name));
            stack.extend(n.children.iter().copied());
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
        n.key_context.as_deref().or((n.kind() == Kind::TextInput).then_some("text_input"))
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

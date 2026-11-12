//! The accessibility tree (AccessKit) from the kernel's semantics: what assistive technology
//! (VoiceOver, Narrator, Orca, TalkBack) reads, and what agents can read too.

use super::{HandlerKind, Kernel, Kind, NodeId, Role, State};
use accesskit as ak;
use slotmap::{Key, KeyData};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

/// The AccessKit id of a node.
pub fn a11y_id(id: NodeId) -> ak::NodeId {
    ak::NodeId(id.data().as_ffi())
}

/// A text larger than this has no runs (its value only).
const RUNS_LIMIT: usize = 64 * 1024;
/// The most characters in one run (AccessKit's word starts are bytes): a longer line is split
/// into runs on the same line.
const RUN_CHARS: usize = 255;
/// The most runs a text has.
const MAX_RUNS: usize = 1 << 12;
/// A run's id: this bit, the run's number, and its text's slot (the version's low 19 bits).
const RUN_BIT: u64 = 1 << 63;

fn run_id(text: NodeId, run: usize) -> ak::NodeId {
    let ffi = text.data().as_ffi();
    ak::NodeId(RUN_BIT | ((run as u64) << 51) | (((ffi >> 32) & 0x7_FFFF) << 32) | (ffi & 0xFFFF_FFFF))
}

/// One AccessKit node with its id.
type AkNode = (ak::NodeId, ak::Node);

/// One run of a text: its bytes, the line it is on, and its characters' byte ranges.
struct Run {
    bytes: Range<usize>,
    line: usize,
    chars: Vec<Range<usize>>,
}

impl Kernel {
    /// The node an AccessKit id names, if it still exists (a text run names its text).
    pub fn node_of_a11y(&self, id: ak::NodeId) -> Option<NodeId> {
        let ffi = if id.0 & RUN_BIT != 0 { id.0 & 0x7_FFFF_FFFF_FFFF } else { id.0 };
        let n = NodeId::from(KeyData::from_ffi(ffi));
        self.is_alive(n).then_some(n)
    }

    /// Whether a text gets runs: a selectable text of a region.
    fn has_runs(&self, id: NodeId) -> bool {
        self.kind(id) == Kind::Text
            && self.text_str(id).len() <= RUNS_LIMIT
            && self.region_of(id).is_some_and(|r| self.selectable_in(r, id))
    }

    /// A text's runs: its laid-out lines, a hard break ending the line before it, split at
    /// [`RUN_CHARS`]. Empty when it has none.
    fn text_runs(&self, id: NodeId) -> Vec<Run> {
        let text = self.text_str(id);
        if text.is_empty() || !self.has_runs(id) {
            return Vec::new();
        }
        let lines = self.text_lines(id);
        let mut out = Vec::new();
        for (i, l) in lines.iter().enumerate() {
            let end = lines.get(i + 1).map_or(text.len(), |n| n.start);
            let start = if i == 0 { 0 } else { l.start };
            let mut chars = Vec::new();
            for (b, g) in text[start..end].grapheme_indices(true) {
                if g.len() > usize::from(u8::MAX) {
                    return Vec::new();
                }
                chars.push(start + b..start + b + g.len());
                if chars.len() == RUN_CHARS {
                    let c = std::mem::take(&mut chars);
                    out.push(Run { bytes: c[0].start..c[c.len() - 1].end, line: i, chars: c });
                }
            }
            if !chars.is_empty() {
                out.push(Run { bytes: chars[0].start..chars[chars.len() - 1].end, line: i, chars });
            }
            if out.len() > MAX_RUNS {
                return Vec::new();
            }
        }
        out
    }

    /// The AccessKit nodes of a text's runs, in its node's space.
    fn a11y_runs(&self, id: NodeId, runs: &[Run]) -> Vec<AkNode> {
        let text = self.text_str(id);
        let lines = self.text_lines(id);
        let ((ox, oy), _, _) = self.text_box(id);
        // Where words start: the first character of each segment that is not blank.
        let starts: rustc_hash::FxHashSet<usize> =
            text.split_word_bound_indices().filter(|(_, w)| !w.trim().is_empty()).map(|(b, _)| b).collect();
        runs.iter()
            .enumerate()
            .map(|(i, r)| {
                let mut node = ak::Node::new(Role::TextRun);
                node.set_value(&text[r.bytes.clone()]);
                node.set_character_lengths(r.chars.iter().map(|c| c.len() as u8).collect::<Vec<_>>());
                node.set_word_starts(
                    r.chars.iter().enumerate().filter(|(_, c)| starts.contains(&c.start)).map(|(n, _)| n as u8).collect::<Vec<_>>(),
                );
                let line = &lines[r.line];
                // Each character's edges: the stops at or before its bytes, in one pass.
                let mut stop = 0;
                let mut x_at = |b: usize| {
                    while line.stops.get(stop + 1).is_some_and(|s| s.0 <= b) {
                        stop += 1;
                    }
                    line.stops.get(stop).map_or(0.0, |s| s.1)
                };
                let xs: Vec<(f32, f32)> = r.chars.iter().map(|c| (x_at(c.start), x_at(c.end))).collect();
                let x0 = xs.iter().map(|(a, b)| a.min(*b)).fold(f32::INFINITY, f32::min);
                let x1 = xs.iter().map(|(a, b)| a.max(*b)).fold(f32::NEG_INFINITY, f32::max);
                let x0 = if x0.is_finite() { x0 } else { 0.0 };
                node.set_character_positions(xs.iter().map(|(a, b)| a.min(*b) - x0).collect::<Vec<_>>());
                node.set_character_widths(xs.iter().map(|(a, b)| (b - a).abs()).collect::<Vec<_>>());
                let (x, y) = (f64::from(ox + x0), f64::from(oy + line.top));
                node.set_bounds(ak::Rect { x0: x, y0: y, x1: x + f64::from((x1 - x0).max(0.0)), y1: y + f64::from(line.height) });
                // Runs split from one line are on it together.
                if i > 0 && runs[i - 1].line == r.line {
                    node.set_previous_on_line(run_id(id, i - 1));
                }
                if runs.get(i + 1).is_some_and(|n| n.line == r.line) {
                    node.set_next_on_line(run_id(id, i + 1));
                }
                (run_id(id, i), node)
            })
            .collect()
    }

    /// Where byte `at` of a text is among its runs.
    fn run_position(&self, id: NodeId, runs: &[Run], at: usize) -> Option<ak::TextPosition> {
        let (i, r) = runs.iter().enumerate().find(|(_, r)| at < r.bytes.end).or_else(|| runs.iter().enumerate().next_back())?;
        let index = r.chars.iter().position(|c| at < c.end).unwrap_or(r.chars.len());
        Some(ak::TextPosition { node: run_id(id, i), character_index: index })
    }

    /// The text and byte a run position names (assistive technology setting a selection).
    pub fn text_at_a11y(&self, pos: ak::TextPosition) -> Option<(NodeId, usize)> {
        if pos.node.0 & RUN_BIT == 0 {
            return None;
        }
        let text = self.node_of_a11y(pos.node)?;
        let runs = self.text_runs(text);
        let run = runs.get(((pos.node.0 >> 51) & 0xFFF) as usize)?;
        Some((text, run.chars.get(pos.character_index).map_or(run.bytes.end, |c| c.start)))
    }

    /// A region's selection among its texts' runs.
    fn a11y_selection(&self, region: NodeId) -> Option<ak::TextSelection> {
        let [(a, ao), (f, fo)] = self.node(region).doc_selection?;
        let at = |t: NodeId, o: usize| self.is_alive(t).then(|| self.run_position(t, &self.text_runs(t), o)).flatten();
        Some(ak::TextSelection { anchor: at(a, ao)?, focus: at(f, fo)? })
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
        self.a11y_geometry.set(false);
        let relaid: Vec<NodeId> = self.a11y_relaid.borrow_mut().drain().collect();
        if self.a11y_viewport.replace(self.viewport) != self.viewport {
            // The window resized: anything may have moved.
            self.a11y_moved(self.root, &mut dirty);
        } else {
            self.a11y_relaid_moved(&relaid, &mut dirty);
            // Nodes moving now, and those that were (to send where they came to rest).
            let t = self.now;
            let now: Vec<NodeId> = self
                .moving
                .iter()
                .copied()
                .filter(|id| self.is_alive(*id) && self.node(*id).anim.as_ref().is_some_and(|a| a.rest_at(t).is_some()))
                .collect();
            let was = std::mem::replace(&mut *self.a11y_moving.borrow_mut(), now.clone());
            for id in now.into_iter().chain(was) {
                if self.is_alive(id) {
                    self.a11y_place_check(id, &mut dirty);
                }
            }
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

    /// How many sent nodes' places accessibility updates compared so far.
    pub fn a11y_compared(&self) -> u64 {
        self.a11y_compared.get()
    }

    /// The next [`accessibility_update`](Self::accessibility_update) sends the whole tree (the
    /// platform asked for it again).
    pub fn restart_accessibility(&self) {
        self.a11y_live.set(false);
    }

    /// Sends `id` and the subtrees of its children assistive technology has not seen.
    fn a11y_emit(&self, id: NodeId, scale: f64, out: &mut Vec<AkNode>, sent: &mut rustc_hash::FxHashSet<NodeId>) {
        let (node, g, children, runs) = self.a11y_build(id, scale);
        self.node(id).a11y_sent.set(Some(g));
        sent.insert(id);
        out.push((a11y_id(id), node));
        // A text's runs place its region's selection: send that again too.
        if !runs.is_empty()
            && let Some(r) = self.region_of(id).filter(|r| self.node(*r).doc_selection.is_some_and(|s| s.iter().any(|e| e.0 == id)))
            && !sent.contains(&r)
        {
            let (node, g, ..) = self.a11y_build(r, scale);
            self.node(r).a11y_sent.set(Some(g));
            sent.insert(r);
            out.push((a11y_id(r), node));
        }
        out.extend(runs);
        for c in children {
            if self.node(c).a11y_sent.get().is_none() && !sent.contains(&c) {
                self.a11y_emit(c, scale, out, sent);
            }
        }
    }

    /// Nodes already sent whose place may have changed with the layout of `relaid`: each one's
    /// and its ancestors' children (a change moves the ones after it), and inside a node whose
    /// size changed, its children. A node's place is in its parent's space, so a subtree that
    /// only moved is sent at its root.
    fn a11y_relaid_moved(&self, relaid: &[NodeId], out: &mut Vec<NodeId>) {
        let mut parents = rustc_hash::FxHashSet::default();
        for &r in relaid {
            let mut n = self.is_alive(r).then_some(r);
            while let Some(a) = n {
                if !parents.insert(a) {
                    break;
                }
                n = self.node(a).parent;
            }
        }
        for a in parents {
            for &c in &self.node(a).children {
                if self.shown_to_a11y(c) {
                    self.a11y_place_check(c, out);
                }
            }
        }
    }

    /// `id` if its place changed since it was sent, and inside it too if its size did.
    fn a11y_place_check(&self, id: NodeId, out: &mut Vec<NodeId>) {
        self.a11y_compared.set(self.a11y_compared.get() + 1);
        let n = self.node(id);
        let Some(was) = n.a11y_sent.get() else { return };
        let now = self.a11y_geometry_of(id);
        if was == now {
            return;
        }
        out.push(id);
        if was[2..4] != now[2..4] {
            for &c in &n.children {
                if self.shown_to_a11y(c) {
                    self.a11y_place_check(c, out);
                }
            }
        }
    }

    /// Nodes already sent whose place changed.
    fn a11y_moved(&self, id: NodeId, out: &mut Vec<NodeId>) {
        self.a11y_compared.set(self.a11y_compared.get() + 1);
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
    fn a11y_subtree(&self, id: NodeId, scale: f64, out: &mut Vec<AkNode>, seen: &mut dyn FnMut(NodeId, [f32; 6])) {
        let (node, g, children, runs) = self.a11y_build(id, scale);
        seen(id, g);
        out.push((a11y_id(id), node));
        out.extend(runs);
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
    fn a11y_build(&self, id: NodeId, scale: f64) -> (ak::Node, [f32; 6], Vec<NodeId>, Vec<AkNode>) {
        let n = self.node(id);
        let role = if id == self.root { Role::Window } else { self.role(id) };
        let mut node = ak::Node::new(role);
        // Placed in the parent's space; a scroll container's space moves with its offset, so its
        // children keep their transforms while it scrolls.
        let g = self.a11y_geometry_of(id);
        let [x, y, w, h, sx, sy] = g.map(f64::from);
        let place = ak::Affine::translate((x - sx, y - sy));
        node.set_transform(if id == self.root { ak::Affine::scale(scale) * place } else { place });
        node.set_bounds(ak::Rect { x0: sx, y0: sy, x1: sx + w, y1: sy + h });
        // The name: an explicit label, else a text's own text (a label's value, AccessKit's
        // static text).
        if let Some(label) = &n.label {
            node.set_label(label.as_str());
        } else if n.kind() == Kind::Text && !n.text().is_empty() {
            if role == Role::Label {
                node.set_value(n.text());
            } else {
                node.set_label(n.text());
            }
        }
        if let Some(l) = n.level {
            node.set_level(usize::from(l));
        }
        if let Some(l) = n.live {
            node.set_live(l);
        }
        if n.region {
            node.add_action(ak::Action::SetTextSelection);
            if let Some(sel) = self.a11y_selection(id) {
                node.set_text_selection(sel);
            }
        }
        // A tooltip saying what the label says is not said twice.
        if let Some(d) = n.description.as_ref().or(n.tooltip.as_ref().filter(|t| n.label.as_ref() != Some(*t))) {
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
        // A selectable text's runs come first, its links after.
        let runs = if n.kind() == Kind::Text { self.a11y_runs(id, &self.text_runs(id)) } else { Vec::new() };
        node.set_children(runs.iter().map(|(r, _)| *r).chain(children.iter().map(|c| a11y_id(*c))).collect::<Vec<_>>());
        (node, g, children, runs)
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
            (State::DRAG_OVER, "drag_over"),
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

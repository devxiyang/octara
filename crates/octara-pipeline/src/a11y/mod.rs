//! Project retained semantics and physical text into AccessKit's incremental protocol.

use crate::layout::LayoutEngine;
use crate::text::TextEngine;
use accesskit as ak;
use octara_model::geometry::Rect;
use octara_model::identity::TreeId;
use octara_model::semantics::{Checked, Live, Relations, Role};
use octara_model::style::{Display, State, UserSelect};
use octara_model::text::TextGeometry;
use octara_tree::selection::DocSelection;
use octara_tree::semantics::SemanticScope;
use octara_tree::tables::HandlerKind;
use octara_tree::{NodeFlags, NodeId, NodeKind, Tree, TreeDelta};
use rustc_hash::{FxHashMap, FxHashSet};
use slotmap::Key;
use std::sync::Arc;

mod publication;

pub(crate) use publication::AccessPublication;
use publication::Published;

pub fn node_identity(node: NodeId) -> ak::NodeId {
    ak::NodeId(node.data().as_ffi())
}

#[derive(Debug, PartialEq)]
struct Run {
    id: ak::NodeId,
    node: ak::Node,
    bytes: Arc<[usize]>,
}

#[derive(Debug, PartialEq)]
struct Entry {
    node: ak::Node,
    runs: Vec<Run>,
    parent: Option<NodeId>,
    bounds: Rect,
    exposed: bool,
    relations: Relations<Arc<str>>,
    selection: Option<DocSelection>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ScopeInput {
    // Modal ordering uses display and attachment even for accessibility-hidden nodes.
    shown: bool,
    visible: bool,
    parent: Option<NodeId>,
    owner: Option<NodeId>,
    modal: bool,
    group: Option<octara_model::overlay::LayerGroupId>,
}

impl ScopeInput {
    fn read(tree: &Tree, node: NodeId) -> Option<Self> {
        tree.node(node)?;
        Some(Self {
            shown: tree.is_live(node) && tree.style(node).is_none_or(|style| style.layout.display != Display::None),
            visible: tree.is_locally_accessible(node),
            parent: tree.physical_parent(node),
            owner: tree.logical_parent(node),
            modal: tree.overlay(node).and_then(|overlay| overlay.layer).is_some_and(|layer| layer.inert_outside),
            group: tree.layer_group(node),
        })
    }
}

#[derive(Clone, Copy)]
struct Scope<'a> {
    tree: &'a Tree,
    modal: Option<&'a SemanticScope>,
}

impl Scope<'_> {
    fn exposes(self, node: NodeId) -> bool {
        self.modal.map_or_else(|| self.tree.is_accessible_without_modal(node), |scope| scope.exposes(node))
    }

    fn includes(self, node: NodeId) -> bool {
        node == self.tree.root() || self.modal.map_or_else(|| self.exposes(node), |scope| scope.includes(node))
    }

    fn focus(self) -> NodeId {
        self.tree
            .focus()
            .filter(|&focus| self.exposes(focus))
            .or_else(|| self.modal.and_then(|scope| scope.modal).filter(|&modal| self.exposes(modal)))
            .unwrap_or_else(|| self.tree.root())
    }
}

#[derive(Default)]
struct Staging {
    // A cycle can project several feedback rounds. Revisit their dirty inputs against the
    // committed baseline so changes that are superseded before commit never escape.
    dirty: FxHashSet<NodeId>,
    moved: FxHashSet<NodeId>,
    scope_dirty: FxHashSet<NodeId>,
    modal_order_dirty: bool,
    tree_id: Option<TreeId>,
    root: Option<NodeId>,
    focus: Option<NodeId>,
    scale: f32,
    modal: Option<Arc<SemanticScope>>,
    scopes: FxHashMap<NodeId, Option<ScopeInput>>,
    names: FxHashMap<NodeId, [Option<Arc<str>>; 2]>,
    entries: FxHashMap<NodeId, Option<Entry>>,
    #[cfg(test)]
    built: usize,
    #[cfg(test)]
    candidates: usize,
}

pub(crate) struct AccessKitProjector {
    publication: AccessPublication,
    staging: Staging,
}

impl AccessKitProjector {
    pub(crate) fn new(publication: AccessPublication) -> Self {
        Self { publication, staging: Staging::default() }
    }

    pub(crate) fn project(&mut self, tree: &Tree, delta: &TreeDelta, layout: &LayoutEngine, text: &TextEngine, scale: f32, verify: bool) {
        let committed = self.publication.0.borrow();
        let staging = &mut self.staging;
        staging.tree_id = Some(tree.id());
        staging.root = Some(tree.root());
        staging.scale = scale;
        let reset = committed.tree_id != staging.tree_id;

        staging.scope_dirty.extend(delta.created.iter().copied());
        staging.scope_dirty.extend(delta.semantics_dirty.iter());
        staging.scope_dirty.extend(delta.topology_dirty.iter());
        staging.scope_dirty.extend(delta.retired.iter().map(|retired| retired.node()));
        if !delta.semantics_dirty.is_disjoint(&delta.layout_dirty)
            || !delta.semantics_dirty.is_disjoint(&delta.overlay_dirty)
            || !delta.topology_dirty.is_empty()
            || !delta.retired.is_empty()
        {
            staging.scope_dirty.extend(
                tree.overlay_nodes().filter(|&node| tree.overlay(node).is_some_and(|slot| slot.mount.is_some() || slot.target.is_some())),
            );
        }
        staging.modal_order_dirty |=
            delta.topology_dirty.iter().any(|node| tree.node(node).is_some_and(|core| core.kind != NodeKind::Text));
        if reset {
            staging.scope_dirty.extend(tree.nodes().map(|(node, _)| node));
        }
        staging.scopes.clear();
        staging.names.clear();
        for &node in &staging.scope_dirty {
            let input = ScopeInput::read(tree, node);
            if reset || input.as_ref() != committed.scopes.get(&node) {
                staging.scopes.insert(node, input);
            }
            let names = indexed_names(tree, node);
            let previous = committed.names.get(&node).map(|names| names.each_ref().map(Option::as_ref)).unwrap_or_default();
            if reset || names != previous {
                staging.names.insert(node, names.map(|name| name.cloned()));
            }
        }
        staging.modal = modal_scope(tree, &committed, staging, reset);
        let scope = Scope { tree, modal: staging.modal.as_deref() };
        staging.focus = Some(scope.focus());

        staging.dirty.extend(delta.semantics_dirty.iter());
        staging.dirty.extend(delta.topology_dirty.iter());
        staging.dirty.extend(delta.created.iter().copied());
        staging.dirty.extend(delta.layout_dirty.iter());
        staging.dirty.extend(delta.editor_dirty.iter());
        staging.dirty.extend(text.changed().iter());
        staging.moved.extend(layout.changed().iter());
        let mut dirty = staging.dirty.clone();
        if committed.scale != scale {
            dirty.insert(tree.root());
        }
        for &node in &staging.moved {
            if !scope.includes(node) {
                continue;
            }
            let bounds = layout.bounds(node).unwrap_or_default();
            let old = committed.entries.get(&node).map_or_else(Rect::default, |entry| entry.bounds);
            if old != bounds {
                dirty.insert(node);
                if old.x != bounds.x || old.y != bounds.y {
                    // A fixed-position child may have unchanged absolute bounds when its
                    // parent moves, but AccessKit encodes relative transforms.
                    dirty.extend(tree.children(node).iter().copied());
                }
            }
        }

        let exposure = exposure_changes(tree, scope, reset, &committed, staging);
        let mut relation_names = FxHashSet::default();
        for (&node, next) in &staging.names {
            relation_names.extend(committed.names.get(&node).into_iter().flatten().flatten());
            relation_names.extend(next.iter().flatten());
        }
        for node in exposure {
            dirty.insert(node);
            dirty.extend(tree.physical_parent(node));
            dirty.extend(committed.entries.get(&node).and_then(|entry| entry.parent));
            relation_names.extend(indexed_names(tree, node).into_iter().flatten());
        }
        for name in relation_names {
            dirty.extend(committed.related.get(name).into_iter().flatten().copied());
        }
        let regions = dirty.iter().filter_map(|node| committed.selecting.get(node)).flatten().copied().collect::<Vec<_>>();
        dirty.extend(regions);

        staging.entries.clear();
        #[cfg(test)]
        {
            staging.candidates = dirty.len() + staging.moved.len() + staging.scope_dirty.len();
            staging.built = 0;
        }
        for node in dirty {
            let entry = scope.includes(node).then(|| build_entry(tree, scope, node, layout, text, scale));
            #[cfg(test)]
            {
                staging.built += usize::from(entry.is_some());
            }
            if entry.is_some() || committed.entries.contains_key(&node) {
                staging.entries.insert(node, entry);
            }
        }

        // Document selection positions refer to text runs, potentially in different leaves.
        let selections = staging.entries.iter().filter_map(|(&node, entry)| Some((node, entry.as_ref()?.selection?)));
        let selections = selections
            .map(|(node, selection)| {
                let resolve = |id| staging.entries.get(&id).map_or_else(|| committed.entries.get(&id), Option::as_ref);
                (node, text_selection(selection, resolve))
            })
            .collect::<Vec<_>>();
        for (node, selection) in selections {
            if let Some(entry) = staging.entries.get_mut(&node).and_then(Option::as_mut) {
                entry.node.clear_text_selection();
                if let Some(selection) = selection {
                    entry.node.set_text_selection(selection);
                }
            }
        }
        staging.entries.retain(|node, entry| reset || entry.as_ref() != committed.entries.get(node));

        if verify {
            verify_projection(tree, layout, text, scale, &committed, staging);
        }
    }

    pub(crate) fn commit(&mut self) -> ak::TreeUpdate {
        self.publication.0.borrow_mut().commit(std::mem::take(&mut self.staging))
    }

    pub(crate) fn reset_staging(&mut self) {
        self.staging = Staging::default();
    }
}

fn indexed_names(tree: &Tree, node: NodeId) -> [Option<&Arc<str>>; 2] {
    tree.provenance(node)
        .filter(|_| tree.is_live(node))
        .map_or([None, None], |provenance| [provenance.id.as_ref(), provenance.test_id.as_ref()])
}

fn modal_scope(tree: &Tree, committed: &Published, staging: &Staging, reset: bool) -> Option<Arc<SemanticScope>> {
    let modal = tree.front_modal()?;
    if !reset
        && !staging.modal_order_dirty
        && staging.scopes.is_empty()
        && committed.modal.as_ref().is_some_and(|scope| scope.modal == Some(modal))
    {
        return committed.modal.clone();
    }
    if !reset && let Some(previous) = committed.modal.as_ref().filter(|scope| scope.modal == Some(modal)) {
        let exposes = |node| tree.is_accessible_without_modal(node) && tree.is_in_modal_scope(node, modal);
        let changed_physical_tree = staging.scopes.iter().any(|(&node, next)| {
            if previous.exposes(node) != exposes(node) {
                return true;
            }
            let path = |input: &ScopeInput| (input.parent, input.visible);
            previous.includes(node) && next.as_ref().map(path) != committed.scopes.get(&node).map(path)
        });
        // A portal may gain or lose an accessible physical path through a branch
        // outside the modal, or change its logical membership through its owner.
        if !changed_physical_tree && (staging.scopes.is_empty() || tree.overlay_nodes().all(|node| previous.exposes(node) == exposes(node)))
        {
            return Some(previous.clone());
        }
    }
    Some(Arc::new(tree.semantic_scope_with_modal(Some(modal))))
}

fn exposure_changes(tree: &Tree, scope: Scope<'_>, reset: bool, committed: &Published, staging: &Staging) -> FxHashSet<NodeId> {
    let changed = |node| {
        let entry = committed.entries.get(&node);
        entry.is_some() != scope.includes(node) || entry.is_some_and(|entry| entry.exposed != scope.exposes(node))
    };
    match (staging.modal.as_ref(), committed.modal.as_ref()) {
        (Some(next), Some(previous)) if Arc::ptr_eq(next, previous) => FxHashSet::default(),
        (Some(next), _) => committed.entries.keys().copied().chain(next.included()).filter(|&node| reset || changed(node)).collect(),
        (None, Some(_)) => committed
            .entries
            .keys()
            .copied()
            .chain(tree.semantic_scope_with_modal(None).included())
            .filter(|&node| reset || changed(node))
            .collect(),
        (None, None) if reset => committed.entries.keys().copied().chain(tree.semantic_scope_with_modal(None).included()).collect(),
        (None, None) => {
            let mut affected = FxHashSet::default();
            for &node in staging.scopes.keys() {
                if affected.contains(&node) || !changed(node) {
                    continue;
                }
                let mut pending = vec![node];
                while let Some(node) = pending.pop() {
                    if affected.insert(node) {
                        pending.extend_from_slice(tree.children(node));
                    }
                }
            }
            affected
        }
    }
}

fn text_selection<'a>([anchor, focus]: DocSelection, entry: impl Fn(NodeId) -> Option<&'a Entry>) -> Option<ak::TextSelection> {
    Some(ak::TextSelection { anchor: position(entry(anchor.0)?, anchor.1)?, focus: position(entry(focus.0)?, focus.1)? })
}

#[cold]
#[inline(never)]
fn verify_projection(tree: &Tree, layout: &LayoutEngine, text: &TextEngine, scale: f32, committed: &Published, staged: &Staging) {
    let full = tree.semantic_scope();
    let scope = Scope { tree, modal: Some(&full) };
    let mut reference: FxHashMap<_, _> = full.included().map(|id| (id, build_entry(tree, scope, id, layout, text, scale))).collect();
    let selections = reference
        .iter()
        .filter_map(|(&node, entry)| Some((node, text_selection(entry.selection?, |id| reference.get(&id)))))
        .collect::<Vec<_>>();
    for (node, selection) in selections {
        let entry = reference.get_mut(&node).expect("included selection region");
        entry.node.clear_text_selection();
        if let Some(selection) = selection {
            entry.node.set_text_selection(selection);
        }
    }
    for &node in reference.keys().chain(committed.entries.keys()).chain(staged.entries.keys()) {
        let current = staged.entries.get(&node).map_or_else(|| committed.entries.get(&node), Option::as_ref);
        assert_eq!(current, reference.get(&node), "incremental AccessKit projection diverged for {node:?}");
    }
    assert_eq!(staged.focus, Some(scope.focus()), "incremental AccessKit focus diverged");
}

fn build_entry(tree: &Tree, scope: Scope<'_>, id: NodeId, layout: &LayoutEngine, text: &TextEngine, scale: f32) -> Entry {
    let core = tree.node(id).expect("scope contains allocated node");
    let semantics = tree.semantics(id);
    let exposed = scope.exposes(id);
    let selected_role = if id == tree.root() {
        Role::Window
    } else if !exposed {
        Role::GenericContainer
    } else {
        semantics.and_then(|slot| slot.role).unwrap_or_else(|| match core.kind {
            NodeKind::Text => Role::Label,
            NodeKind::TextInput => match tree.editor(id) {
                Some(editor) if editor.is_secure() => Role::PasswordInput,
                Some(editor) if editor.multiline => Role::MultilineTextInput,
                _ => Role::TextInput,
            },
            NodeKind::Image | NodeKind::Media | NodeKind::Icon => Role::Image,
            NodeKind::Native => Role::WebView,
            NodeKind::Link => Role::Link,
            _ if tree.handler(id, HandlerKind::Press).is_some() => Role::Button,
            _ if tree.scroll(id).is_some() => Role::ScrollView,
            _ => Role::GenericContainer,
        })
    };
    let mut node = ak::Node::new(role(selected_role));
    let bounds = layout.bounds(id).unwrap_or_default();
    let parent = tree.physical_parent(id);
    let parent_bounds = parent.and_then(|parent| layout.bounds(parent)).unwrap_or_default();
    let transform = ak::Affine::translate((f64::from(bounds.x - parent_bounds.x), f64::from(bounds.y - parent_bounds.y)));
    node.set_transform(if id == tree.root() { ak::Affine::scale(f64::from(scale)) * transform } else { transform });
    node.set_bounds(ak_rect(Rect::new(0.0, 0.0, bounds.w, bounds.h)));
    let mut children: Vec<_> = tree.children(id).iter().copied().filter(|&child| scope.includes(child)).map(node_identity).collect();
    if !exposed {
        node.set_children(children);
        return Entry { node, runs: Vec::new(), parent, bounds, exposed, relations: Relations::default(), selection: None };
    }

    let label = semantics.and_then(|slot| slot.label.as_deref());
    if let Some(label) = label {
        node.set_label(label);
    } else if let Some(label) = tree.text(id).map(|text| text.text.as_ref()) {
        if selected_role == Role::Label {
            node.set_value(label);
        } else {
            node.set_label(label);
        }
    } else if selected_role == Role::Button
        && !semantics.is_some_and(|slot| slot.relations.labelled_by.is_some())
        && let [caption] = tree.children(id)
        && scope.exposes(*caption)
        && tree.text(*caption).is_some()
    {
        node.push_labelled_by(node_identity(*caption));
    } else if let Some(link) = tree.link(id)
        && let Some(label) =
            tree.physical_parent(id).and_then(|parent| tree.text(parent)).and_then(|text| text.text.get(link.range.clone()))
    {
        node.set_label(label);
    }
    if let Some(editor) = tree.editor(id) {
        node.set_placeholder(editor.placeholder.as_ref());
        if editor.is_secure() {
            node.set_value("•".repeat(unicode_segmentation::UnicodeSegmentation::graphemes(editor.value(), true).count()));
        } else if let Some(composition) = editor.composition() {
            node.set_value(composition.text());
        } else {
            node.set_value(editor.value());
        }
        if tree.handler(id, HandlerKind::Input).is_some() {
            node.add_action(ak::Action::SetValue);
        } else {
            node.set_read_only();
        }
        if !editor.is_secure() {
            node.add_action(ak::Action::SetTextSelection);
        }
    }
    if let Some(slot) = semantics {
        if selected_role == Role::TabList
            && let Some(group) = slot.focus_group
        {
            node.set_orientation(if group.is_vertical() { ak::Orientation::Vertical } else { ak::Orientation::Horizontal });
        }
        let target = |name: &str| {
            let [id] = tree.named_nodes(name) else { return None };
            scope.exposes(*id).then(|| node_identity(*id))
        };
        if let Some(value) = &slot.a11y_value
            && !tree.editor(id).is_some_and(|editor| editor.is_secure())
        {
            node.set_value(value.as_ref());
        }
        if let Some(description) = &slot.description {
            node.set_description(description.as_ref());
        }
        if let Some(description) = &slot.tooltip {
            node.set_tooltip(description.as_ref());
        }
        if let Some(level) = slot.level {
            node.set_level(usize::from(level));
        }
        if let Some(live) = slot.live {
            node.set_live(match live {
                Live::Off => ak::Live::Off,
                Live::Polite => ak::Live::Polite,
                Live::Assertive => ak::Live::Assertive,
            });
        }
        if let Some(checked) = slot.checked {
            node.set_toggled(match checked {
                Checked::Unchecked => ak::Toggled::False,
                Checked::Checked => ak::Toggled::True,
                Checked::Mixed => ak::Toggled::Mixed,
            });
        }
        if slot.multiselectable == Some(true) {
            node.set_multiselectable();
        }
        if let Some(selected) = slot.selected {
            node.set_selected(selected);
        }
        if slot.required == Some(true) {
            node.set_required();
        }
        if slot.invalid == Some(true) {
            node.set_invalid(ak::Invalid::True);
        }
        if let Some(expanded) = slot.expanded {
            node.set_expanded(expanded);
        }
        if let Some(range) = slot.numeric_range {
            node.set_min_numeric_value(range.min);
            node.set_max_numeric_value(range.max);
            if let Some(value) = range.value {
                node.set_numeric_value(value);
            }
            if let Some(step) = range.step {
                node.set_numeric_value_step(step);
            }
            if tree.handler(id, HandlerKind::Accessibility).is_some() {
                node.add_action(ak::Action::SetValue);
                if range.step.is_some() {
                    node.add_action(ak::Action::Increment);
                    node.add_action(ak::Action::Decrement);
                }
            }
        }
        if let Some(target) = slot.relations.labelled_by.as_deref().and_then(target) {
            node.push_labelled_by(target);
        }
        if let Some(target) = slot.relations.described_by.as_deref().and_then(target) {
            node.set_described_by(vec![target]);
        }
        if let Some(target) = slot.relations.controls.as_deref().and_then(target) {
            node.push_controlled(target);
        }
        if let Some(target) = slot.relations.active_descendant.as_deref().and_then(target) {
            node.set_active_descendant(target);
        }
    }
    if tree.is_disabled(id) {
        node.set_disabled();
    }
    if core.state.contains(State::SELECTED) {
        node.set_selected(true);
    }
    if core.flags.contains(NodeFlags::FOCUSABLE) {
        node.add_action(ak::Action::Focus);
    }
    let selectable = tree.user_select(id) == Some(UserSelect::Text) && core.kind != NodeKind::TextInput;
    if selectable {
        node.add_action(ak::Action::SetTextSelection);
    }
    if tree.overlay(id).and_then(|overlay| overlay.layer).is_some_and(|layer| layer.inert_outside) {
        node.set_modal();
    }
    if let Some(test_id) = tree.provenance(id).and_then(|source| source.test_id.as_deref()) {
        node.set_author_id(test_id);
    }
    if tree.handler(id, HandlerKind::Press).is_some() || tree.handler(id, HandlerKind::Toggle).is_some() || core.kind == NodeKind::Link {
        node.add_action(ak::Action::Click);
    }
    if tree.handler(id, HandlerKind::SecondaryPress).is_some()
        || std::iter::successors(Some(id), |&node| tree.logical_parent(node))
            .any(|node| tree.handler(node, HandlerKind::ContextMenu).is_some())
    {
        node.add_action(ak::Action::ShowContextMenu);
    }

    let runs = text
        .geometry(id)
        .and_then(|geometry| {
            let value = tree
                .editor(id)
                .and_then(|editor| {
                    editor.reportable_value().map(|value| editor.composition().map_or(value, |composition| composition.text()))
                })
                .or_else(|| tree.text(id).map(|slot| slot.text.as_ref()))?;
            Some(text_runs(id, value, bounds, &geometry))
        })
        .unwrap_or_default();
    children.splice(0..0, runs.iter().map(|run| run.id));
    node.set_children(children);
    let relations = semantics.map_or_else(Relations::default, |slot| slot.relations.clone());
    let selection = selectable.then(|| tree.doc_selection(id)).flatten();
    let mut entry = Entry { node, runs, parent, bounds, exposed, relations, selection };
    if !selectable && let Some(editor) = tree.editor(id) {
        let selected = editor.composition().map_or(Some(editor.selection()), |composition| composition.selection());
        if let Some((anchor, focus)) = selected
            && let Some((anchor, focus)) = position(&entry, anchor).zip(position(&entry, focus))
        {
            entry.node.set_text_selection(ak::TextSelection { anchor, focus });
        }
    }
    entry
}

fn text_runs(id: NodeId, value: &str, bounds: Rect, geometry: &TextGeometry) -> Vec<Run> {
    // Keep the protocol's one-byte character lengths representable, even for long graphemes.
    let mut runs = Vec::new();
    if value.len() > 64 * 1024 {
        return runs;
    }
    for line in geometry.lines.iter() {
        let mut fragments = Vec::new();
        for part in line.fragments.iter().filter(|part| !part.range.is_empty() && value.get(part.range.clone()).is_some()) {
            let mut start = part.range.start;
            while start < part.range.end {
                let end = value.floor_char_boundary((start + usize::from(u8::MAX)).min(part.range.end));
                fragments.push(octara_model::text::TextFragment { range: start..end, rect: part.rect, rtl: part.rtl });
                start = end;
            }
        }
        fragments.sort_by_key(|part| part.range.start);
        if fragments.is_empty() {
            continue;
        }
        let mut cursor = 0;
        while cursor < fragments.len() && runs.len() < 4096 {
            let first = cursor;
            let rtl = fragments[first].rtl;
            cursor += 1;
            while cursor < fragments.len()
                && cursor - first < 255
                && fragments[cursor].rtl == rtl
                && fragments[cursor - 1].range.end == fragments[cursor].range.start
            {
                cursor += 1;
            }
            let spans = &fragments[first..cursor];
            let start = spans[0].range.start;
            let end = spans[spans.len() - 1].range.end;
            let left = spans.iter().map(|part| part.rect.x).fold(f32::INFINITY, f32::min);
            let right = spans.iter().map(|part| part.rect.right()).fold(f32::NEG_INFINITY, f32::max);
            let mut run = ak::Node::new(ak::Role::TextRun);
            run.set_value(&value[start..end]);
            run.set_character_lengths(spans.iter().map(|part| part.range.len() as u8).collect::<Vec<_>>());
            run.set_character_positions(
                spans.iter().map(|part| if rtl { right - part.rect.right() } else { part.rect.x - left }).collect::<Vec<_>>(),
            );
            run.set_character_widths(spans.iter().map(|part| part.rect.w).collect::<Vec<_>>());
            run.set_text_direction(if rtl { ak::TextDirection::RightToLeft } else { ak::TextDirection::LeftToRight });
            run.set_bounds(ak_rect(Rect::new(0.0, 0.0, right - left, line.rect.h)));
            run.set_transform(ak::Affine::translate((
                f64::from(left + geometry.origin.x - bounds.x),
                f64::from(line.rect.y + geometry.origin.y - bounds.y),
            )));
            let node = node_identity(id).0;
            let synthetic = (1_u64 << 63) | ((runs.len() as u64) << 51) | (node & ((1_u64 << 51) - 1));
            let bytes = spans.iter().map(|part| part.range.start).chain(std::iter::once(end)).collect();
            runs.push(Run { id: ak::NodeId(synthetic), node: run, bytes });
        }
    }
    runs
}

fn position(entry: &Entry, byte: usize) -> Option<ak::TextPosition> {
    let run = entry.runs.iter().find(|run| run.bytes.last().is_some_and(|&last| byte <= last)).or_else(|| entry.runs.last())?;
    let index = run.bytes.partition_point(|&offset| offset <= byte).saturating_sub(1);
    Some(ak::TextPosition { node: run.id, character_index: index })
}

fn ak_rect(bounds: Rect) -> ak::Rect {
    ak::Rect::new(f64::from(bounds.x), f64::from(bounds.y), f64::from(bounds.right()), f64::from(bounds.bottom()))
}

fn role(role: Role) -> ak::Role {
    match role {
        Role::Window => ak::Role::Window,
        Role::GenericContainer => ak::Role::GenericContainer,
        Role::Label => ak::Role::Label,
        Role::Heading => ak::Role::Heading,
        Role::Paragraph => ak::Role::Paragraph,
        Role::Blockquote => ak::Role::Blockquote,
        Role::Code => ak::Role::Code,
        Role::Button => ak::Role::Button,
        Role::CheckBox => ak::Role::CheckBox,
        Role::RadioButton => ak::Role::RadioButton,
        Role::RadioGroup => ak::Role::RadioGroup,
        Role::Switch => ak::Role::Switch,
        Role::TextInput => ak::Role::TextInput,
        Role::MultilineTextInput => ak::Role::MultilineTextInput,
        Role::PasswordInput => ak::Role::PasswordInput,
        Role::ComboBox => ak::Role::ComboBox,
        Role::EditableComboBox => ak::Role::EditableComboBox,
        Role::List => ak::Role::List,
        Role::ListItem => ak::Role::ListItem,
        Role::ListBox => ak::Role::ListBox,
        Role::ListBoxOption => ak::Role::ListBoxOption,
        Role::Table => ak::Role::Table,
        Role::Row => ak::Role::Row,
        Role::Cell => ak::Role::Cell,
        Role::ColumnHeader => ak::Role::ColumnHeader,
        Role::Tree => ak::Role::Tree,
        Role::TreeItem => ak::Role::TreeItem,
        Role::Navigation => ak::Role::Navigation,
        Role::Tooltip => ak::Role::Tooltip,
        Role::MenuItemCheckbox => ak::Role::MenuItemCheckBox,
        Role::MenuItemRadio => ak::Role::MenuItemRadio,
        Role::Toolbar => ak::Role::Toolbar,
        Role::Menu => ak::Role::Menu,
        Role::MenuItem => ak::Role::MenuItem,
        Role::Tab => ak::Role::Tab,
        Role::TabList => ak::Role::TabList,
        Role::TabPanel => ak::Role::TabPanel,
        Role::Dialog => ak::Role::Dialog,
        Role::AlertDialog => ak::Role::AlertDialog,
        Role::Status => ak::Role::Status,
        Role::Alert => ak::Role::Alert,
        Role::Main => ak::Role::Main,
        Role::Group => ak::Role::Group,
        Role::Document => ak::Role::Document,
        Role::Link => ak::Role::Link,
        Role::Image => ak::Role::Image,
        Role::Slider => ak::Role::Slider,
        Role::SpinButton => ak::Role::SpinButton,
        Role::ProgressIndicator => ak::Role::ProgressIndicator,
        Role::WebView => ak::Role::WebView,
        Role::ScrollView => ak::Role::ScrollView,
        Role::Splitter | Role::Separator => ak::Role::Splitter,
    }
}

#[cfg(test)]
mod tests;

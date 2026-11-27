//! Committed accessibility state for the current window.

use super::{AccessibilityWork, Entry, ScopeInput, Staging, node_identity};
use accesskit as ak;
use octara_model::identity::{TargetGuard, TreeId};
use octara_model::input::HostReply;
use octara_model::semantics::{A11yAction, TextPosition};
use octara_runtime::engine::AccessibilityConsumer;
use octara_tree::semantics::SemanticScope;
use octara_tree::{NodeId, Tree};
use rustc_hash::{FxHashMap, FxHashSet};
use std::cell::RefCell;
use std::hash::Hash;
use std::rc::Rc;
use std::sync::Arc;

/// The metadata of a committed native accessibility tree.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AccessibilityContext {
    pub tree: TreeId,
    pub modal: Option<ak::NodeId>,
    pub scale: f32,
}

/// A platform delta with the scope and scale that were current at its commit.
pub struct AccessibilityCommit {
    pub version: u64,
    pub context: AccessibilityContext,
    pub update: Arc<ak::TreeUpdate>,
}

#[derive(Clone, Default)]
pub(crate) struct AccessPublication(pub(super) Rc<RefCell<Published>>);

impl AccessPublication {
    pub(crate) fn for_window_host() -> Self {
        Self(Rc::new(RefCell::new(Published { platform_updates: Some(Vec::new()), ..Published::default() })))
    }

    pub(crate) fn active(&self) -> bool {
        self.0.borrow().active()
    }

    pub(crate) fn set_active(&self, consumer: AccessibilityConsumer, active: bool) {
        let mut published = self.0.borrow_mut();
        match consumer {
            AccessibilityConsumer::Platform => {
                published.platform_active = active;
                if !active && let Some(updates) = &mut published.platform_updates {
                    updates.clear();
                }
            }
            AccessibilityConsumer::Inspector => published.inspector_active = active,
        }
        if !published.active() {
            published.clear_tree();
        }
    }

    pub(crate) fn has_tree(&self) -> bool {
        self.0.borrow().root.is_some()
    }

    pub(crate) fn work(&self) -> AccessibilityWork {
        self.0.borrow().work
    }

    pub(crate) fn take_platform_updates(&self) -> Vec<(u64, Arc<ak::TreeUpdate>)> {
        self.take_platform_commits().into_iter().map(|commit| (commit.version, commit.update)).collect()
    }

    pub(crate) fn take_platform_commits(&self) -> Vec<AccessibilityCommit> {
        self.0.borrow_mut().platform_updates.as_mut().map(std::mem::take).unwrap_or_default()
    }

    pub(crate) fn context(&self) -> Option<AccessibilityContext> {
        self.0.borrow().context()
    }

    pub(crate) fn target(&self, native: ak::NodeId) -> Option<TargetGuard> {
        let published = self.0.borrow();
        let node = *published.targets.get(&native)?;
        let tree = published.tree_id?;
        published.entries.get(&node)?.exposed.then(|| TargetGuard::for_node(tree, node))
    }

    pub(crate) fn supports_action(&self, target: TargetGuard, action: ak::Action) -> bool {
        let published = self.0.borrow();
        published.tree_id == Some(target.tree)
            && target
                .node()
                .and_then(|node| published.entries.get(&node))
                .is_some_and(|entry| entry.exposed && !entry.node.is_disabled() && entry.node.supports_action(action))
    }

    pub(crate) fn snapshot_full(&self) -> Option<ak::TreeUpdate> {
        let published = self.0.borrow();
        let root = node_identity(published.root?);
        let mut nodes = Vec::new();
        for (&id, entry) in &published.entries {
            nodes.push((node_identity(id), entry.node.clone()));
            nodes.extend(entry.runs.iter().map(|run| (run.id, run.node.clone())));
        }
        nodes.sort_unstable_by_key(|(id, _)| *id);
        #[cfg(test)]
        published.snapshotted.set(nodes.len());
        Some(ak::TreeUpdate {
            nodes,
            tree: Some(ak::TreeInfo::new(root)),
            tree_id: ak::TreeId::ROOT,
            focus: node_identity(published.focus?),
        })
    }

    pub(crate) fn node(&self, native: ak::NodeId) -> Option<NodeId> {
        self.0.borrow().node(native)
    }

    pub(crate) fn host_action(&self, tree: &Tree, request: ak::ActionRequest) -> Option<HostReply> {
        let published = self.0.borrow();
        if request.target_tree != ak::TreeId::ROOT || published.tree_id != Some(tree.id()) {
            return None;
        }
        let id = published.node(request.target_node)?;
        let target = tree.guard(id);
        if !tree.contains_target(target) {
            return None;
        }
        let action = match (request.action, request.data) {
            (ak::Action::Click, _) => A11yAction::Press,
            (ak::Action::Focus, _) => A11yAction::Focus,
            (ak::Action::ShowContextMenu, _) => A11yAction::ShowContextMenu,
            (ak::Action::SetValue | ak::Action::ReplaceSelectedText, Some(ak::ActionData::Value(text))) => {
                A11yAction::SetValue(text.into())
            }
            (ak::Action::SetValue, Some(ak::ActionData::NumericValue(value))) => A11yAction::SetNumericValue(value),
            (ak::Action::Increment, _) => A11yAction::Increment,
            (ak::Action::Decrement, _) => A11yAction::Decrement,
            (ak::Action::SetTextSelection, Some(ak::ActionData::SetTextSelection(selection))) => A11yAction::SetTextSelection {
                anchor: published.text_position(selection.anchor)?,
                focus: published.text_position(selection.focus)?,
            },
            _ => return None,
        };
        Some(HostReply::AccessibilityAction { target, action })
    }
}

#[derive(Default)]
pub(super) struct Published {
    platform_active: bool,
    inspector_active: bool,
    platform_updates: Option<Vec<AccessibilityCommit>>,
    pub work: AccessibilityWork,
    pub tree_id: Option<TreeId>,
    pub root: Option<NodeId>,
    pub focus: Option<NodeId>,
    pub scale: f32,
    pub modal: Option<Arc<SemanticScope>>,
    pub entries: FxHashMap<NodeId, Entry>,
    pub scopes: FxHashMap<NodeId, ScopeInput>,
    pub names: FxHashMap<NodeId, [Option<Arc<str>>; 2]>,
    pub related: FxHashMap<Arc<str>, FxHashSet<NodeId>>,
    pub selecting: FxHashMap<NodeId, FxHashSet<NodeId>>,
    targets: FxHashMap<ak::NodeId, NodeId>,
    runs: FxHashMap<ak::NodeId, (NodeId, Arc<[usize]>)>,
    #[cfg(test)]
    pub committed: usize,
    #[cfg(test)]
    pub snapshotted: std::cell::Cell<usize>,
}

impl Published {
    fn active(&self) -> bool {
        self.platform_active || self.inspector_active
    }

    fn clear_tree(&mut self) {
        let platform_updates = self.platform_updates.as_ref().map(|_| Vec::new());
        *self = Self {
            platform_active: self.platform_active,
            inspector_active: self.inspector_active,
            platform_updates,
            work: self.work,
            ..Self::default()
        };
    }

    pub(super) fn record_platform_update(&mut self, version: u64, update: &Arc<ak::TreeUpdate>) {
        if self.platform_active
            && let Some(context) = self.context()
            && let Some(updates) = &mut self.platform_updates
        {
            updates.push(AccessibilityCommit { version, context, update: update.clone() });
        }
    }

    fn context(&self) -> Option<AccessibilityContext> {
        Some(AccessibilityContext {
            tree: self.tree_id?,
            modal: self.modal.as_ref().and_then(|scope| scope.modal).map(node_identity),
            scale: self.scale,
        })
    }

    fn node(&self, native: ak::NodeId) -> Option<NodeId> {
        self.targets.get(&native).copied().or_else(|| self.runs.get(&native).map(|entry| entry.0))
    }

    fn text_position(&self, position: ak::TextPosition) -> Option<TextPosition> {
        let (node, bytes) = self.runs.get(&position.node)?;
        Some(TextPosition { node: *node, offset: *bytes.get(position.character_index)? })
    }

    pub(super) fn commit(&mut self, staged: Staging, emit: bool) -> Option<ak::TreeUpdate> {
        let root = staged.root.expect("project before publish");
        let focus = staged.focus.expect("project focus before publish");
        let focus_changed = self.focus != Some(focus);
        let modal_changed = self.modal.as_ref().and_then(|scope| scope.modal) != staged.modal.as_ref().and_then(|scope| scope.modal);
        let new_tree = self.tree_id != staged.tree_id;
        let tree = (new_tree || self.root != Some(root)).then(|| ak::TreeInfo::new(node_identity(root)));
        if new_tree {
            self.clear_tree();
        }
        self.root = Some(root);
        self.tree_id = staged.tree_id;
        self.focus = Some(focus);
        self.scale = staged.scale;
        self.modal = staged.modal;

        for (id, input) in staged.scopes {
            set(&mut self.scopes, id, input);
        }
        for (id, names) in staged.names {
            let retained = names.iter().any(Option::is_some);
            set(&mut self.names, id, retained.then_some(names));
        }

        #[cfg(test)]
        {
            self.committed = staged.entries.len() + staged.geometry.len();
        }
        let mut nodes = Vec::new();
        for (id, entry) in staged.entries {
            let old = self.entries.remove(&id);
            let key = node_identity(id);
            if let Some(old) = old.as_ref() {
                self.index(id, old, false);
                self.targets.remove(&key);
                for run in &old.runs {
                    self.runs.remove(&run.id);
                }
            }
            if let Some(entry) = entry {
                if emit && old.as_ref().is_none_or(|old| entry.node != old.node) {
                    nodes.push((key, entry.node.clone()));
                }
                self.targets.insert(key, id);
                for (index, run) in entry.runs.iter().enumerate() {
                    if emit
                        && old.as_ref().and_then(|entry| entry.runs.get(index)).is_none_or(|old| old.id != run.id || old.node != run.node)
                    {
                        nodes.push((run.id, run.node.clone()));
                    }
                    self.runs.insert(run.id, (id, run.bytes.clone()));
                }
                self.index(id, &entry, true);
                self.entries.insert(id, entry);
            }
        }
        for (id, geometry) in staged.geometry {
            let entry = self.entries.get_mut(&id).expect("geometry patch has a published node");
            geometry.apply(&mut entry.node);
            entry.geometry = geometry;
            if emit {
                nodes.push((node_identity(id), entry.node.clone()));
            }
        }
        nodes.sort_unstable_by_key(|(id, _)| *id);
        (emit && (!nodes.is_empty() || tree.is_some() || focus_changed || modal_changed)).then(|| ak::TreeUpdate {
            nodes,
            tree,
            tree_id: ak::TreeId::ROOT,
            focus: node_identity(focus),
        })
    }

    fn index(&mut self, id: NodeId, entry: &Entry, insert: bool) {
        let relations = &entry.relations;
        for name in
            [&relations.labelled_by, &relations.described_by, &relations.controls, &relations.active_descendant].into_iter().flatten()
        {
            index(&mut self.related, name, id, insert);
        }
        for (endpoint, _) in entry.selection.iter().flatten() {
            index(&mut self.selecting, endpoint, id, insert);
        }
    }
}

fn set<T>(entries: &mut FxHashMap<NodeId, T>, id: NodeId, value: Option<T>) {
    if let Some(value) = value {
        entries.insert(id, value);
    } else {
        entries.remove(&id);
    }
}

fn index<K: Clone + Eq + Hash>(index: &mut FxHashMap<K, FxHashSet<NodeId>>, key: &K, id: NodeId, insert: bool) {
    if insert {
        index.entry(key.clone()).or_default().insert(id);
    } else if let Some(entries) = index.get_mut(key) {
        entries.remove(&id);
        if entries.is_empty() {
            index.remove(key);
        }
    }
}

//! Transaction construction and read-only validation of authored writes.

use crate::delta::TreeDelta;
use crate::editor::EditorRichText;
use crate::overlay::{LayerGroupId, LayerPolicy};
use crate::tables::{
    AuthoredStyle, CommandSlot, Component, DragDrop, EditorSlot, HandlerId, HandlerKind, LinkSlot, PaintSlot, SemanticsSlot,
};
use crate::{NodeFlags, NodeId, NodeKind, RowKey, Tree, TreeId, TreeRev};
use octara_model::CacheLine;
use octara_model::geometry::Placement;
use octara_model::image::ImageSource;
use octara_model::motion::TextReveal;
use octara_model::native::NativeSlot;
use octara_model::overlay::AnchorTarget;
use octara_model::paint::{CanvasOp, Image, NodeIcon, ObjectFit};
use octara_model::text::{InlineAttachment, RichSpan};
use octara_model::theme::ThemePatch;
use rustc_hash::{FxHashMap, FxHashSet};
use std::borrow::Cow;
use std::ops::Range;
use std::panic::Location;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PendingNode {
    batch_ns: u128,
    index: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NodeRef {
    Existing { tree: TreeId, id: NodeId },
    Pending(PendingNode),
}

impl From<PendingNode> for NodeRef {
    fn from(node: PendingNode) -> Self {
        Self::Pending(node)
    }
}

pub enum NodeContent {
    Box,
    Text(Arc<str>),
    TextInput(Box<EditorSlot>),
    Image { image: Image, fit: ObjectFit },
    ImageSource { source: ImageSource, fit: ObjectFit },
    Icon(Arc<NodeIcon>),
    Canvas(Arc<[CanvasOp]>),
    Native { kind: &'static str, slot: NativeSlot },
}

impl NodeContent {
    pub(crate) fn kind(&self) -> NodeKind {
        match self {
            Self::Box => NodeKind::Box,
            Self::Text(_) => NodeKind::Text,
            Self::TextInput(_) => NodeKind::TextInput,
            Self::Image { .. } => NodeKind::Image,
            Self::ImageSource { .. } => NodeKind::Media,
            Self::Icon(_) => NodeKind::Icon,
            Self::Canvas(_) => NodeKind::Canvas,
            Self::Native { .. } => NodeKind::Native,
        }
    }
}

pub struct NodeSpec {
    pub content: NodeContent,
    pub origin: &'static Location<'static>,
    pub component: Option<Component>,
    pub row_key: Option<RowKey>,
}

impl NodeSpec {
    #[track_caller]
    pub fn new(content: NodeContent) -> Self {
        Self { content, origin: Location::caller(), component: None, row_key: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Retirement {
    Exit,
    VisibleExit,
    Destroy,
}

#[derive(Debug, Default)]
pub struct RichText {
    pub text: Arc<str>,
    pub runs: Vec<(Range<usize>, RichSpan)>,
    pub links: Vec<LinkSlot>,
    pub attachments: Vec<InlineAttachment>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct OverlaySpec {
    pub target: Option<(AnchorTarget<NodeRef>, Placement)>,
    pub layer: Option<LayerPolicy>,
    pub group: Option<LayerGroupId>,
    pub mount: Option<(NodeRef, u64)>,
}

pub enum NodeChange {
    Style(Box<AuthoredStyle>),
    /// Replaces a subtree theme patch, or clears it with `None`.
    ThemePatch(Option<Box<ThemePatch>>),
    Flags(NodeFlags),
    Disabled(bool),
    Text(Arc<str>),
    RichText(Box<RichText>),
    SpliceText {
        range: Range<usize>,
        insert: Arc<str>,
    },
    EditorValue(String),
    EditorRichText(Box<EditorRichText>),
    EditorPlaceholder(Arc<str>),
    EditorSecure(bool),
    Image {
        image: Image,
        fit: ObjectFit,
    },
    ImageSource {
        source: ImageSource,
        fit: ObjectFit,
    },
    TextReveal(Option<TextReveal>),
    Icon(Arc<NodeIcon>),
    Canvas(Arc<[CanvasOp]>),
    NativeProps(serde_json::Value),
    Paint(Box<PaintSlot>),
    Semantics(Box<SemanticsSlot>),
    Handler {
        kind: HandlerKind,
        id: Option<HandlerId>,
    },
    DragDrop(Option<Box<DragDrop>>),
    Commands(Box<CommandSlot>),
    Overlay(OverlaySpec),
    TestId(Option<Arc<str>>),
    Id(Option<Arc<str>>),
    DemandBounds,
}

impl NodeChange {
    fn content_kind(&self) -> Option<NodeKind> {
        match self {
            Self::Text(_) | Self::RichText(_) | Self::SpliceText { .. } | Self::TextReveal(_) => Some(NodeKind::Text),
            Self::EditorValue(_) | Self::EditorRichText(_) | Self::EditorPlaceholder(_) | Self::EditorSecure(_) => {
                Some(NodeKind::TextInput)
            }
            Self::Image { .. } => Some(NodeKind::Image),
            Self::ImageSource { .. } => Some(NodeKind::Media),
            Self::Icon(_) => Some(NodeKind::Icon),
            Self::Canvas(_) => Some(NodeKind::Canvas),
            Self::NativeProps(_) => Some(NodeKind::Native),
            _ => None,
        }
    }
}

pub enum TxOp {
    Change(NodeRef, NodeChange),
    /// The destination index after removing the moved children.
    Place {
        parent: NodeRef,
        index: usize,
        child: NodeRef,
    },
    Retire {
        node: NodeRef,
        mode: Retirement,
    },
}

pub struct Transaction {
    pub tree: TreeId,
    pub base_rev: TreeRev,
    pub batch_ns: u128,
    pub specs: Vec<NodeSpec>,
    pub ops: Vec<TxOp>,
}

pub struct TxBuilder(Transaction);

impl TxBuilder {
    pub fn new(tree: TreeId, base_rev: TreeRev) -> Self {
        use std::sync::atomic::AtomicU64;
        static NEXT: CacheLine<AtomicU64> = CacheLine(AtomicU64::new(1));
        use std::sync::atomic::Ordering::Relaxed;
        let ns = NEXT.try_update(Relaxed, Relaxed, |next| next.checked_add(1)).expect("transaction namespace exhausted");
        let batch_ns = (u128::from(tree.namespace[0]) << 64) | u128::from(ns);
        Self(Transaction { tree, base_rev, batch_ns, specs: Vec::new(), ops: Vec::new() })
    }

    pub fn create(&mut self, spec: NodeSpec) -> PendingNode {
        let index = u32::try_from(self.0.specs.len()).expect("transaction node budget exhausted");
        self.0.specs.push(spec);
        PendingNode { batch_ns: self.0.batch_ns, index }
    }

    pub fn change(&mut self, node: impl Into<NodeRef>, change: NodeChange) {
        self.0.ops.push(TxOp::Change(node.into(), change));
    }

    pub fn place(&mut self, parent: impl Into<NodeRef>, index: usize, child: impl Into<NodeRef>) {
        self.0.ops.push(TxOp::Place { parent: parent.into(), index, child: child.into() });
    }

    pub fn retire(&mut self, node: impl Into<NodeRef>, mode: Retirement) {
        self.0.ops.push(TxOp::Retire { node: node.into(), mode });
    }

    /// Marks a pending allocation as destroyed before it acquires authored properties.
    pub fn discard(&mut self, pending: PendingNode) {
        let node = NodeRef::Pending(pending);
        self.0.ops.retain(|op| !matches!(op, TxOp::Change(target, _) | TxOp::Retire { node: target, .. } if *target == node));
        self.retire(node, Retirement::Destroy);
    }

    pub fn is_empty(&self) -> bool {
        self.0.specs.is_empty() && self.0.ops.is_empty()
    }

    pub fn finish(self) -> Transaction {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeError {
    WrongTree,
    StaleRevision,
    RevisionExhausted,
    UnknownNode(NodeRef),
    InternalNode(NodeRef),
    RetiredNode(NodeRef),
    WrongContent(NodeRef),
    DuplicatePlacement(NodeRef),
    DuplicateRow(NodeRef),
    Cycle(NodeRef),
    InvalidTextEdit(NodeRef),
}

pub struct RejectedTransaction {
    pub reason: TreeError,
    pub tx: Box<Transaction>,
}

impl std::fmt::Debug for RejectedTransaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RejectedTransaction").field("reason", &self.reason).finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct TreeReceipt {
    pub revision: TreeRev,
    pub created: Vec<NodeId>,
    batch_ns: u128,
}

impl TreeReceipt {
    pub fn node(&self, pending: PendingNode) -> Option<NodeId> {
        if self.batch_ns != pending.batch_ns {
            return None;
        }
        self.created.get(pending.index as usize).copied()
    }
}

#[derive(Default)]
struct StagedTopology {
    parents: FxHashMap<NodeRef, NodeRef>,
    mount_owners: FxHashMap<NodeRef, Option<NodeRef>>,
    changed_mounts: FxHashSet<NodeRef>,
    placements: Vec<(NodeRef, usize, NodeRef)>,
    retirements: FxHashMap<NodeRef, Retirement>,
    retire_order: Vec<NodeRef>,
}

struct Preflight<'a> {
    tree: &'a Tree,
    tx: &'a Transaction,
    topology: StagedTopology,
}

impl Preflight<'_> {
    fn kind(&self, node: NodeRef) -> Result<NodeKind, TreeError> {
        self.resolve_kind(node, false)
    }

    fn resolve_kind(&self, node: NodeRef, allow_exiting: bool) -> Result<NodeKind, TreeError> {
        match node {
            NodeRef::Existing { tree, id } => {
                if tree != self.tree.id() {
                    return Err(TreeError::WrongTree);
                }
                let core = self.tree.node(id).ok_or(TreeError::UnknownNode(node))?;
                if !allow_exiting && core.flags.contains(NodeFlags::EXITING) {
                    return Err(TreeError::RetiredNode(node));
                }
                Ok(core.kind)
            }
            NodeRef::Pending(pending) => self
                .tx
                .specs
                .get(pending.index as usize)
                .filter(|_| pending.batch_ns == self.tx.batch_ns)
                .map(|spec| spec.content.kind())
                .ok_or(TreeError::UnknownNode(node)),
        }
    }

    fn application_node(&self, node: NodeRef, allow_exiting: bool) -> Result<(), TreeError> {
        let kind = self.resolve_kind(node, allow_exiting)?;
        if matches!(kind, NodeKind::Root | NodeKind::Link) || matches!(node, NodeRef::Existing { id, .. } if id == self.tree.overlay_root())
        {
            return Err(TreeError::InternalNode(node));
        }
        Ok(())
    }

    fn parent(&self, node: NodeRef) -> Option<NodeRef> {
        self.topology.parents.get(&node).copied().or_else(|| match node {
            NodeRef::Existing { id, .. } => self.tree.physical_parent(id).map(|parent| self.tree.node_ref(parent)),
            NodeRef::Pending(_) => None,
        })
    }

    fn row_key(&self, node: NodeRef) -> Option<&RowKey> {
        match node {
            NodeRef::Existing { id, .. } => self.tree.provenance(id)?.row_key.as_ref(),
            NodeRef::Pending(pending) => self.tx.specs[pending.index as usize].row_key.as_ref(),
        }
    }

    fn retirement_ancestor(&self, node: NodeRef) -> Option<NodeRef> {
        if self.topology.retirements.is_empty() {
            return None;
        }
        let mut seen = FxHashSet::default();
        let mut pending = smallvec::SmallVec::<[NodeRef; 8]>::new();
        pending.push(node);
        while let Some(node) = pending.pop() {
            if !seen.insert(node) {
                continue;
            }
            if self.topology.retirements.contains_key(&node) {
                return Some(node);
            }
            pending.extend(self.parent(node));
            pending.extend(self.portal_owner(node));
        }
        None
    }

    fn portal_owner(&self, node: NodeRef) -> Option<NodeRef> {
        self.topology.mount_owners.get(&node).copied().unwrap_or_else(|| match node {
            NodeRef::Existing { id, .. } => self.tree.portal_owner(id).map(|owner| self.tree.node_ref(owner)),
            NodeRef::Pending(_) => None,
        })
    }

    fn validate(mut self) -> Result<StagedTopology, TreeError> {
        for op in &self.tx.ops {
            match *op {
                TxOp::Place { parent, child, index } => {
                    self.application_node(child, false)?;
                    if !matches!(self.kind(parent)?, NodeKind::Box | NodeKind::Root) {
                        return Err(TreeError::WrongContent(parent));
                    }
                    if self.topology.parents.insert(child, parent).is_some() {
                        return Err(TreeError::DuplicatePlacement(child));
                    }
                    self.topology.placements.push((parent, index, child));
                }
                TxOp::Retire { node, mode } => {
                    self.application_node(node, mode == Retirement::Destroy)?;
                    use std::collections::hash_map::Entry;
                    match self.topology.retirements.entry(node) {
                        Entry::Vacant(entry) => {
                            entry.insert(mode);
                            self.topology.retire_order.push(node);
                        }
                        Entry::Occupied(mut entry) if mode == Retirement::Destroy => {
                            entry.insert(mode);
                        }
                        Entry::Occupied(_) => {}
                    }
                }
                TxOp::Change(node, NodeChange::Overlay(overlay)) => {
                    let owner = overlay.mount.map(|(owner, _)| owner);
                    if owner != self.portal_owner(node) {
                        self.topology.changed_mounts.insert(node);
                    }
                    self.topology.mount_owners.insert(node, owner);
                }
                TxOp::Change(..) => {}
            }
        }

        let mut resolved = FxHashSet::default();
        let mut visited = FxHashSet::default();
        for &(parent, _, child) in &self.topology.placements {
            visited.clear();
            visited.insert(child);
            let mut cursor = Some(parent);
            while let Some(ancestor) = cursor {
                if !visited.insert(ancestor) {
                    return Err(TreeError::Cycle(child));
                }
                if resolved.contains(&ancestor) {
                    break;
                }
                cursor = self.parent(ancestor);
            }
            resolved.extend(visited.iter().copied());
        }

        let mut rows = FxHashSet::default();
        for &(parent, _, child) in &self.topology.placements {
            if let Some(retired) = self.retirement_ancestor(child) {
                return Err(TreeError::RetiredNode(retired));
            }
            let Some(key) = self.row_key(child) else { continue };
            if !rows.insert((parent, key)) {
                return Err(TreeError::DuplicateRow(child));
            }
            if let NodeRef::Existing { id, .. } = parent
                && let Some(old) = self.tree.row_child(id, key)
            {
                let old = self.tree.node_ref(old);
                if old != child && self.parent(old) == Some(parent) && self.retirement_ancestor(old).is_none() {
                    return Err(TreeError::DuplicateRow(child));
                }
            }
        }

        self.validate_changes()?;
        self.validate_overlay_paths()?;
        self.topology.placements.sort_by_key(|&(_, index, _)| index);
        Ok(self.topology)
    }

    fn validate_overlay_paths(&self) -> Result<(), TreeError> {
        let overlays: FxHashMap<_, _> = self
            .tx
            .ops
            .iter()
            .filter_map(|op| match op {
                TxOp::Change(node, NodeChange::Overlay(spec)) => Some((*node, *spec)),
                _ => None,
            })
            .collect();
        if overlays.is_empty() && self.tree.overlay_index.is_empty() {
            return Ok(());
        }
        let anchor = |node| {
            if let Some(spec) = overlays.get(&node) {
                spec.target.and_then(|(target, _)| target.node())
            } else if let NodeRef::Existing { id, .. } = node {
                self.tree.overlay(id)?.target?.0.node().map(|node| self.tree.node_ref(node))
            } else {
                None
            }
        };
        let seeds = || overlays.keys().copied().chain(self.topology.parents.keys().copied());
        validate_parent_paths(seeds(), |node| [anchor(node).or_else(|| self.parent(node))])?;
        validate_parent_paths(seeds(), |node| [self.portal_owner(node), self.parent(node)])
    }

    fn validate_changes(&self) -> Result<(), TreeError> {
        let mut text = FxHashMap::default();
        for op in &self.tx.ops {
            let TxOp::Change(node, NodeChange::SpliceText { .. }) = op else { continue };
            let previous = match *node {
                NodeRef::Existing { id, .. } => self.tree.text(id).map(|slot| slot.text.as_ref()),
                NodeRef::Pending(pending) => match self.tx.specs.get(pending.index as usize).map(|spec| &spec.content) {
                    Some(NodeContent::Text(text)) => Some(text.as_ref()),
                    _ => None,
                },
            };
            text.insert(*node, Cow::Borrowed(previous.unwrap_or_default()));
        }
        for op in &self.tx.ops {
            let TxOp::Change(node, change) = op else { continue };
            let node = *node;
            let kind = self.kind(node)?;
            if let Some(retired) = self.retirement_ancestor(node) {
                return Err(TreeError::RetiredNode(retired));
            }
            if change.content_kind().is_some_and(|required| kind != required) {
                return Err(TreeError::WrongContent(node));
            }
            if let NodeChange::Overlay(overlay) = change {
                if let Some((owner, _)) = overlay.mount {
                    self.application_node(node, false)?;
                    // Text edits rebuild inline link allocations without authoring placements.
                    if self.kind(owner)? == NodeKind::Link {
                        return Err(TreeError::InternalNode(owner));
                    }
                }
                for target in
                    overlay.target.and_then(|(target, _)| target.node()).into_iter().chain(overlay.mount.map(|(target, _)| target))
                {
                    self.kind(target)?;
                    if let Some(retired) = self.retirement_ancestor(target) {
                        return Err(TreeError::RetiredNode(retired));
                    }
                    if node == target {
                        return Err(TreeError::Cycle(node));
                    }
                }
            }
            let replacement = match change {
                NodeChange::Text(value) => Some(value),
                NodeChange::RichText(rich) => {
                    let ranges = rich.runs.iter().map(|run| &run.0).chain(rich.links.iter().map(|link| &link.range));
                    if ranges.into_iter().any(|range| rich.text.get(range.clone()).is_none())
                        || !InlineAttachment::valid_ranges(&rich.attachments, &rich.text)
                    {
                        return Err(TreeError::InvalidTextEdit(node));
                    }
                    Some(&rich.text)
                }
                NodeChange::SpliceText { range, insert } => {
                    let current = text.get_mut(&node).expect("staged splice");
                    if current.get(range.clone()).is_none() {
                        return Err(TreeError::InvalidTextEdit(node));
                    }
                    current.to_mut().replace_range(range.clone(), insert);
                    None
                }
                _ => None,
            };
            if let Some(replacement) = replacement
                && let Some(current) = text.get_mut(&node)
            {
                *current = Cow::Borrowed(replacement);
            }
        }
        Ok(())
    }
}

fn validate_parent_paths<const N: usize>(
    seeds: impl Iterator<Item = NodeRef>,
    parents: impl Fn(NodeRef) -> [Option<NodeRef>; N],
) -> Result<(), TreeError> {
    let mut resolved = FxHashSet::default();
    let mut path = FxHashSet::default();
    let mut pending = smallvec::SmallVec::<[(NodeRef, bool); 8]>::new();
    for seed in seeds {
        pending.push((seed, false));
        while let Some((node, leaving)) = pending.pop() {
            if leaving {
                path.remove(&node);
                resolved.insert(node);
            } else if !resolved.contains(&node) {
                if !path.insert(node) {
                    return Err(TreeError::Cycle(seed));
                }
                pending.push((node, true));
                pending.extend(parents(node).into_iter().flatten().map(|parent| (parent, false)));
            }
        }
    }
    Ok(())
}

impl Tree {
    pub fn transaction(&self) -> TxBuilder {
        TxBuilder::new(self.id(), self.revision())
    }

    pub fn apply(&mut self, tx: Transaction) -> Result<(TreeReceipt, TreeDelta), RejectedTransaction> {
        let preflight = || {
            if tx.tree != self.id() {
                return Err(TreeError::WrongTree);
            }
            if tx.base_rev != self.revision() {
                return Err(TreeError::StaleRevision);
            }
            let revision = if tx.ops.is_empty() && tx.specs.is_empty() {
                self.revision()
            } else {
                TreeRev(self.revision().0.checked_add(1).ok_or(TreeError::RevisionExhausted)?)
            };
            let topology = Preflight { tree: self, tx: &tx, topology: StagedTopology::default() }.validate()?;
            Ok((revision, topology))
        };
        let (revision, topology) = match preflight() {
            Ok(accepted) => accepted,
            Err(reason) => return Err(RejectedTransaction { reason, tx: Box::new(tx) }),
        };

        let focus_ancestors = self.focus_ancestors();
        let selections = self.selection_projections().collect();
        let mut delta = TreeDelta::default();
        let receipt = TreeReceipt {
            revision,
            batch_ns: tx.batch_ns,
            created: tx.specs.into_iter().map(|spec| self.allocate(spec, &mut delta)).collect(),
        };
        let resolve = |node| match node {
            NodeRef::Existing { id, .. } => id,
            NodeRef::Pending(pending) => receipt.node(pending).expect("preflight validated pending node"),
        };

        let mut style_roots = FxHashSet::default();
        let mut theme_roots = FxHashSet::default();
        let mut portals: Vec<_> =
            topology.changed_mounts.iter().copied().map(resolve).map(|portal| (portal, self.portal_owner(portal))).collect();
        portals.sort_unstable();
        // Remove old edges before adding new ones. Each intermediate Bloom update
        // then sees a subset of either the old or the preflighted final graph.
        for &(portal, _) in &portals {
            self.unindex_portal(portal);
        }
        for op in tx.ops {
            if let TxOp::Change(node, change) = op {
                self.commit_change(resolve(node), change, &resolve, &mut style_roots, &mut theme_roots, &mut delta);
            }
        }
        let mut changed_parent = FxHashSet::default();
        for &(parent, _, child) in &topology.placements {
            let child = resolve(child);
            if self.physical_parent(child).is_some_and(|previous| previous != resolve(parent)) {
                changed_parent.insert(child);
            }
            self.detach(child, &mut delta);
        }
        for (portal, old_owner) in portals {
            self.index_portal(portal);
            changed_parent.insert(portal);
            if self.portal_owner(portal) != old_owner {
                style_roots.insert(portal);
            }
        }
        let retirements = topology.retirements.into_iter().map(|(node, mode)| (resolve(node), mode)).collect();
        self.retire_subtrees(topology.retire_order.into_iter().map(resolve).collect(), &retirements, &mut delta);
        for (parent, index, child) in topology.placements {
            let child = resolve(child);
            self.attach(resolve(parent), index, child, &mut delta);
            style_roots.insert(child);
        }
        for node in changed_parent {
            self.logical_semantics_changed(node, &mut delta);
        }
        self.cancel_replaced_overlay_exits(&mut delta);
        if !focus_ancestors.is_empty() && !(delta.topology_dirty.is_empty() && delta.overlay_dirty.is_empty()) {
            self.reconcile_focus_states(focus_ancestors, self.focus_visible(), &mut style_roots, &mut delta);
        }
        self.resolve_dirty_styles(style_roots, theme_roots, &mut delta);
        self.reconcile_focus(&mut delta);
        self.revalidate_selection(selections, &mut delta);
        self.rev = revision;
        Ok((receipt, delta))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RuntimeOp;
    use crate::delta::RetiredNode;
    use crate::tables::CustomPaintBinding;
    use octara_model::paint::PaintProgram;

    fn tree() -> Tree {
        Tree::new(TreeId::new([3, 7], 1), Default::default())
    }

    #[test]
    fn preflight_rejects_cycle_and_preserves_revision_and_indices() {
        let mut tree = tree();
        let root = tree.node_ref(tree.root());
        let mut mount = tree.transaction();
        let parent = mount.create(NodeSpec::new(NodeContent::Box));
        let child = mount.create(NodeSpec::new(NodeContent::Box));
        mount.change(child, NodeChange::TestId(Some("child".into())));
        mount.place(root, 0, parent);
        mount.place(parent, 0, child);
        let (receipt, _) = tree.apply(mount.finish()).unwrap();
        let parent = receipt.node(parent).unwrap();
        let child = receipt.node(child).unwrap();

        let mut invalid = tree.transaction();
        invalid.change(tree.node_ref(child), NodeChange::TestId(Some("modified".into())));
        invalid.place(tree.node_ref(child), 0, tree.node_ref(parent));
        let error = tree.apply(invalid.finish()).unwrap_err();
        assert_eq!(error.reason, TreeError::Cycle(tree.node_ref(parent)));
        assert_eq!(tree.revision(), receipt.revision);
        assert_eq!(tree.test_ids("child"), &[child]);
        assert!(tree.test_ids("modified").is_empty());

        let mut rotate = tree.transaction();
        rotate.place(tree.node_ref(child), 0, tree.node_ref(parent));
        rotate.place(root, 0, tree.node_ref(child));
        tree.apply(rotate.finish()).unwrap();
        assert_eq!(tree.physical_parent(child), Some(tree.root()));
        assert_eq!(tree.physical_parent(parent), Some(child));
        assert_eq!(tree.test_ids("child"), &[child]);
    }

    #[test]
    fn preflight_rejects_cycles_crossing_physical_and_portal_edges() {
        let mut tree = tree();
        let root = tree.node_ref(tree.root());
        let mut tx = tree.transaction();
        let parent = tx.create(NodeSpec::new(NodeContent::Box));
        let child = tx.create(NodeSpec::new(NodeContent::Box));
        tx.place(root, 0, parent);
        tx.place(parent, 0, child);
        let (receipt, _) = tree.apply(tx.finish()).unwrap();
        let parent = tree.node_ref(receipt.node(parent).unwrap());
        let child = tree.node_ref(receipt.node(child).unwrap());
        let mount = |owner| NodeChange::Overlay(OverlaySpec { mount: Some((owner, 0)), ..Default::default() });

        let mut tx = tree.transaction();
        tx.change(parent, mount(child));
        tx.change(child, mount(root));
        assert!(matches!(tree.apply(tx.finish()).unwrap_err().reason, TreeError::Cycle(_)));
        assert_eq!(tree.revision(), receipt.revision);
        assert!(tree.validate_integrity().is_ok());

        // Moving the child makes these owner changes acyclic as a single transaction.
        let mut tx = tree.transaction();
        tx.change(parent, mount(child));
        tx.change(child, mount(root));
        tx.place(root, 1, child);
        tree.apply(tx.finish()).unwrap();
        assert!(tree.validate_integrity().is_ok());
    }

    #[test]
    fn preflight_validates_edits_against_staged_utf8() {
        let mut tree = tree();
        let mut tx = tree.transaction();
        let node = tx.create(NodeSpec::new(NodeContent::Text("x".into())));
        tx.place(tree.node_ref(tree.root()), 0, node);
        tx.change(node, NodeChange::Text("中ab".into()));
        tx.change(node, NodeChange::SpliceText { range: 3..4, insert: "界".into() });
        tx.change(node, NodeChange::SpliceText { range: 4..5, insert: "".into() });
        let rejected = tree.apply(tx.finish()).unwrap_err();
        assert_eq!(rejected.reason, TreeError::InvalidTextEdit(node.into()));
        assert_eq!(rejected.tx.specs.len(), 1);
        assert_eq!(tree.revision(), TreeRev(0));
        assert_eq!(tree.children(tree.root()), &[tree.overlay_root()]);
    }

    #[test]
    fn finishing_exit_keeps_replacement_row_index_and_returns_owned_assets() {
        let mut tree = tree();
        let root = tree.node_ref(tree.root());
        let row = RowKey::new(1, "file");
        let row_spec = || NodeSpec { row_key: Some(row.clone()), ..NodeSpec::new(NodeContent::Box) };
        let mut mount = tree.transaction();
        let old = mount.create(row_spec());
        let program = PaintProgram::new(17);
        let weak = program.downgrade();
        mount.change(
            old,
            NodeChange::Paint(Box::new(PaintSlot {
                custom: Some(CustomPaintBinding { program, input: Default::default(), mount: Default::default() }),
                transition: None,
            })),
        );
        mount.change(old, NodeChange::TestId(Some("old".into())));
        mount.place(root, 0, old);
        let (receipt, _) = tree.apply(mount.finish()).unwrap();
        let old = receipt.node(old).unwrap();

        let mut duplicate = tree.transaction();
        let second = duplicate.create(row_spec());
        duplicate.place(root, 0, second);
        assert_eq!(tree.apply(duplicate.finish()).unwrap_err().reason, TreeError::DuplicateRow(second.into()));

        let mut replace = tree.transaction();
        replace.retire(tree.node_ref(old), Retirement::Exit);
        let new = replace.create(row_spec());
        replace.place(root, 0, new);
        let (receipt, delta) = tree.apply(replace.finish()).unwrap();
        let new = receipt.node(new).unwrap();
        let ticket = tree.pending_exit(old).unwrap();
        assert_eq!(delta.retired, [RetiredNode::Exiting { ticket, visible: false }]);
        assert_eq!(tree.row_child(tree.root(), &row), Some(new));
        assert!(tree.test_ids("old").is_empty());
        assert!(tree.node(old).is_some());

        let mut retired = tree.apply_runtime(RuntimeOp::FinishExit(ticket));
        assert_eq!(retired.retired, [RetiredNode::Destroyed(old)]);
        assert_eq!(tree.row_child(tree.root(), &row), Some(new));
        assert!(weak.upgrade().is_some());
        drop(retired.take_retired_assets());
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn preflight_rejects_anchor_cycles_and_orders_dependent_overlays() {
        let mut tree = tree();
        let root = tree.node_ref(tree.root());
        let overlays = tree.node_ref(tree.overlay_root());
        let mut tx = tree.transaction();
        let trigger = tx.create(NodeSpec::new(NodeContent::Box));
        let first = tx.create(NodeSpec::new(NodeContent::Box));
        let second = tx.create(NodeSpec::new(NodeContent::Box));
        tx.place(root, 0, trigger);
        tx.place(overlays, 0, second);
        tx.place(overlays, 1, first);
        tx.change(
            first,
            NodeChange::Overlay(OverlaySpec {
                target: Some((octara_model::overlay::AnchorTarget::Node(trigger.into()), Placement::BELOW)),
                ..Default::default()
            }),
        );
        tx.change(
            second,
            NodeChange::Overlay(OverlaySpec {
                target: Some((octara_model::overlay::AnchorTarget::Node(first.into()), Placement::BELOW)),
                ..Default::default()
            }),
        );
        let (receipt, _) = tree.apply(tx.finish()).unwrap();
        let first = receipt.node(first).unwrap();
        let second = receipt.node(second).unwrap();
        assert_eq!(tree.anchored_nodes().collect::<Vec<_>>(), [first, second]);

        let mut cycle = tree.transaction();
        cycle.change(
            tree.node_ref(first),
            NodeChange::Overlay(OverlaySpec {
                target: Some((octara_model::overlay::AnchorTarget::Node(tree.node_ref(second)), Placement::BELOW)),
                ..Default::default()
            }),
        );
        assert!(matches!(tree.apply(cycle.finish()).unwrap_err().reason, TreeError::Cycle(_)));
        assert_eq!(tree.anchored_nodes().collect::<Vec<_>>(), [first, second]);
    }
}

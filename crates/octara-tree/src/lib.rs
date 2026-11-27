//! Retained topology, logical text and interaction state.
//!
//! Authored changes pass through [Tree::apply]. Transient changes pass through
//! [Tree::apply_runtime]. Projection engines only receive shared tree references.

pub mod arbiter;
mod cursor;
pub mod delta;
mod dirty_bitmap;
pub mod editor;
mod exit;
pub mod focus;
mod integrity;
pub mod overlay;
#[doc(hidden)]
pub mod reconcile;
pub mod runtime_op;
pub mod selection;
pub mod semantics;
pub mod style;
pub mod tables;
pub mod transaction;

pub use cursor::resolve_cursor;
pub use delta::{GeometryDemand, RetiredNode, TreeDelta};
pub use dirty_bitmap::DirtyBitmap;
pub use octara_model::identity::{ExitTicket, GuardTarget, NodeId, RowKey, TargetGuard, TreeId, TreeRev};
pub use runtime_op::RuntimeOp;
pub use tables::TableMask;
pub use transaction::{NodeChange, NodeContent, NodeRef, NodeSpec, Retirement, Transaction, TreeReceipt, TxBuilder};

use bitflags::bitflags;
use focus::FocusRestore;
use octara_model::style::{Inherited, LayoutStyle, State, UserSelect, Visual};
use octara_model::theme::{Breakpoint, ResponsiveBreakpoint, Theme};
use overlay::OverlayMount;
use rustc_hash::{FxHashMap, FxHashSet};
use selection::DocSelection;
use slotmap::{SecondaryMap, SlotMap};
use smallvec::SmallVec;
use std::collections::BTreeMap;
use std::sync::Arc;
use style::{ComputedStyle, StyleInterner, StyleResolver, sensitivity};
use tables::*;
use transaction::RichText;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Root,
    Box,
    Text,
    TextInput,
    Image,
    Media,
    Icon,
    Canvas,
    Native,
    Link,
}

bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct NodeFlags: u32 {
        const ATTACHED           = 1 << 0;
        const EXITING            = 1 << 1;
        const HIDDEN             = 1 << 2;
        const FOCUSABLE          = 1 << 3;
        const FOCUS_PREFERRED    = 1 << 4;
        const PRESERVE_FOCUS     = 1 << 5;
        const AUTOFOCUS          = 1 << 6;
        const HOVER_ALONE        = 1 << 7;
        const SELECT_REGION      = 1 << 8;
        const SELECT_NONE        = 1 << 9;
        const FOLLOW_TAIL        = 1 << 10;
        const FOLLOW_TAIL_ACTIVE = 1 << 11;
        const HAS_OVERLAY_MOUNT  = 1 << 12;
        const HAS_ANCHOR         = 1 << 13;
        const HAS_NATIVE         = 1 << 14;
        const HAS_CUSTOM_PAINT   = 1 << 15;
        const INERT              = 1 << 16;
        /// The authoring element has checkbox semantics and no child slots.
        const NATIVE_CHECKBOX    = 1 << 17;
        const INTERACTION_STATE  = 1 << 18;
        const WINDOW_DRAG_REGION = 1 << 19;
        /// Excludes only this node from direct pointer hit testing.
        const POINTER_EVENTS_NONE = 1 << 20;
        const MAINTAIN_SCROLL_ANCHOR = 1 << 21;
        /// A virtual list's leading spacer requests anchoring from its parent.
        const VIRTUAL_SCROLL_ANCHOR = 1 << 22;
        const OVERSCROLL = 1 << 23;
        const DISMISS_KEYBOARD_IMMEDIATELY = 1 << 24;
        const DISMISS_KEYBOARD_INTERACTIVELY = 1 << 25;
    }
}

impl NodeFlags {
    const AUTHORED: Self = Self::HIDDEN
        .union(Self::FOCUSABLE)
        .union(Self::FOCUS_PREFERRED)
        .union(Self::PRESERVE_FOCUS)
        .union(Self::AUTOFOCUS)
        .union(Self::HOVER_ALONE)
        .union(Self::SELECT_REGION)
        .union(Self::SELECT_NONE)
        .union(Self::FOLLOW_TAIL)
        .union(Self::MAINTAIN_SCROLL_ANCHOR)
        .union(Self::VIRTUAL_SCROLL_ANCHOR)
        .union(Self::INERT)
        .union(Self::NATIVE_CHECKBOX)
        .union(Self::INTERACTION_STATE)
        .union(Self::WINDOW_DRAG_REGION)
        .union(Self::OVERSCROLL)
        .union(Self::DISMISS_KEYBOARD_IMMEDIATELY)
        .union(Self::DISMISS_KEYBOARD_INTERACTIVELY);
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NodeCore {
    style_bloom: u64,
    pub parent: Option<NodeId>,
    pub kind: NodeKind,
    // The transaction's flag value before applying authored style.
    authored_pointer_events_none: bool,
    pub state: State,
    pub flags: NodeFlags,
    pub present: TableMask,
    pub style_rev: u32,
    pub layout_rev: u32,
    pub paint_rev: u32,
    pub semantics_rev: u32,
}

const _: () = assert!(std::mem::size_of::<NodeCore>() <= 48);

impl NodeCore {
    fn new(kind: NodeKind) -> Self {
        Self {
            style_bloom: sensitivity::DEFAULT,
            parent: None,
            kind,
            authored_pointer_events_none: false,
            state: State::empty(),
            flags: NodeFlags::empty(),
            present: TableMask::empty(),
            style_rev: 0,
            layout_rev: 0,
            paint_rev: 0,
            semantics_rev: 0,
        }
    }
}

type SelectionProjection = Vec<(NodeId, std::ops::Range<usize>)>;

pub struct Tree {
    id: TreeId,
    rev: TreeRev,
    root: NodeId,
    overlay_root: NodeId,
    nodes: SlotMap<NodeId, NodeCore>,
    tables: Tables,
    id_index: FxHashMap<Arc<str>, SmallVec<[NodeId; 1]>>,
    test_id_index: FxHashMap<Arc<str>, SmallVec<[NodeId; 1]>>,
    row_children: FxHashMap<(NodeId, RowKey), NodeId>,
    overlay_index: FxHashSet<NodeId>,
    // Retained owner edges also keep visual exits reachable until they settle.
    logical_portals: SecondaryMap<NodeId, SmallVec<[NodeId; 2]>>,
    native_index: FxHashSet<NodeId>,
    doc_selections: FxHashMap<NodeId, DocSelection>,
    exit_roots: FxHashMap<NodeId, usize>,
    pending_exits: FxHashMap<NodeId, NodeId>,
    focus: Option<NodeId>,
    focus_restore: FocusRestore,
    scroll_seq: u64,
    theme: Theme,
    root_theme_revision: u64,
    latest_theme_revision: u64,
    breakpoint: Breakpoint,
    responsive_breakpoint: Option<ResponsiveBreakpoint>,
    touch: bool,
    initial_inherited: Inherited,
    initial_style: Arc<ComputedStyle>,
    style_interner: StyleInterner,
    #[cfg(test)]
    style_visits: usize,
    #[cfg(test)]
    style_prunes: usize,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct StyleInvalidation {
    conditions: u64,
    local_state: u64,
    theme: u64,
    inherited_theme: bool,
    inherited_dirty: bool,
    force: bool,
}

impl Tree {
    pub fn new(id: TreeId, theme: Theme) -> Self {
        let mut nodes = SlotMap::with_key();
        let root = nodes.insert(NodeCore { flags: NodeFlags::ATTACHED, present: TableMask::CHILDREN, ..NodeCore::new(NodeKind::Root) });
        let overlay_root = nodes.insert(NodeCore { parent: Some(root), flags: NodeFlags::ATTACHED, ..NodeCore::new(NodeKind::Box) });
        let mut tables = Tables::default();
        tables.children.insert(root, vec![overlay_root]);
        let (initial_inherited, initial_visual) = StyleResolver::initial(&theme);
        let mut style_interner = StyleInterner::default();
        let initial_style = style_interner.intern(ComputedStyle { layout: LayoutStyle::default(), visual: initial_visual });
        Self {
            id,
            rev: TreeRev::default(),
            root,
            overlay_root,
            nodes,
            tables,
            id_index: FxHashMap::default(),
            test_id_index: FxHashMap::default(),
            row_children: FxHashMap::default(),
            overlay_index: FxHashSet::default(),
            logical_portals: SecondaryMap::new(),
            native_index: FxHashSet::default(),
            doc_selections: FxHashMap::default(),
            exit_roots: FxHashMap::default(),
            pending_exits: FxHashMap::default(),
            focus: None,
            focus_restore: FocusRestore::default(),
            scroll_seq: 0,
            theme,
            root_theme_revision: 0,
            latest_theme_revision: 0,
            breakpoint: Breakpoint::default(),
            responsive_breakpoint: None,
            touch: false,
            initial_inherited,
            initial_style,
            style_interner,
            #[cfg(test)]
            style_visits: 0,
            #[cfg(test)]
            style_prunes: 0,
        }
    }

    pub fn id(&self) -> TreeId {
        self.id
    }
    pub fn revision(&self) -> TreeRev {
        self.rev
    }
    pub fn root(&self) -> NodeId {
        self.root
    }
    pub fn overlay_root(&self) -> NodeId {
        self.overlay_root
    }
    pub fn focus(&self) -> Option<NodeId> {
        self.focus
    }
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// The effective theme for a node, including its own patch and ancestor scopes.
    pub fn theme_at(&self, node: NodeId) -> &Theme {
        self.theme_scope_at(Some(node)).map_or(&self.theme, |scope| &scope.theme)
    }

    /// Changes when a node starts using a different effective theme.
    pub fn theme_revision_at(&self, node: NodeId) -> u64 {
        self.theme_scope_at(Some(node)).map_or(self.root_theme_revision, |scope| scope.revision)
    }

    fn theme_scope_at(&self, mut node: Option<NodeId>) -> Option<&ThemeScope> {
        if !self.tables.theme_scopes.is_empty() {
            while let Some(current) = node {
                if let Some(scope) = self.theme_scope(current) {
                    return Some(scope);
                }
                node = self.logical_parent(current);
            }
        }
        None
    }

    fn theme_scope(&self, node: NodeId) -> Option<&ThemeScope> {
        self.has_table(node, TableMask::THEME_SCOPES).then_some(())?;
        self.tables.theme_scopes.get(node)
    }

    fn theme_patch(&self, node: NodeId) -> Option<&octara_model::theme::ThemePatch> {
        self.has_table(node, TableMask::THEME_PATCHES).then_some(())?;
        self.tables.theme_patches.get(node)
    }

    /// Whether this node establishes a theme scope for its descendants.
    #[doc(hidden)]
    pub fn has_theme_patch(&self, node: NodeId) -> bool {
        self.has_table(node, TableMask::THEME_PATCHES)
    }

    fn next_theme_revision(&mut self) -> u64 {
        self.latest_theme_revision = self.latest_theme_revision.checked_add(1).expect("theme revisions exhausted");
        self.latest_theme_revision
    }

    pub fn node(&self, node: NodeId) -> Option<&NodeCore> {
        self.nodes.get(node)
    }

    #[inline]
    fn has_table(&self, node: NodeId, table: TableMask) -> bool {
        self.nodes.get(node).is_some_and(|core| core.present.contains(table))
    }

    pub fn nodes(&self) -> impl Iterator<Item = (NodeId, &NodeCore)> {
        self.nodes.iter()
    }

    /// The parent used for layout, stacking, and clipping.
    pub fn physical_parent(&self, node: NodeId) -> Option<NodeId> {
        self.node(node)?.parent
    }

    /// The authored parent used for cascading properties and interaction.
    pub fn logical_parent(&self, node: NodeId) -> Option<NodeId> {
        let core = self.node(node)?;
        if core.flags.contains(NodeFlags::HAS_OVERLAY_MOUNT) {
            return self.tables.overlays[node].mount.map(|mount| mount.owner);
        }
        core.parent
    }

    fn portal_owner(&self, node: NodeId) -> Option<NodeId> {
        self.node(node)?.flags.contains(NodeFlags::HAS_OVERLAY_MOUNT).then_some(())?;
        self.overlay(node)?.mount.map(|mount| mount.owner)
    }

    pub(crate) fn logical_portals(&self, owner: NodeId) -> &[NodeId] {
        self.logical_portals.get(owner).map_or(&[], SmallVec::as_slice)
    }

    /// Authored children, including portals mounted elsewhere for layout.
    pub fn logical_children(&self, node: NodeId) -> impl DoubleEndedIterator<Item = NodeId> + '_ {
        self.children(node)
            .iter()
            .copied()
            .filter(|&child| !self.nodes[child].flags.contains(NodeFlags::HAS_OVERLAY_MOUNT))
            .chain(self.logical_portals(node).iter().copied())
    }

    fn index_portal(&mut self, node: NodeId) {
        let Some(owner) = self.overlay(node).and_then(|slot| slot.mount).map(|mount| mount.owner) else { return };
        self.nodes[node].flags.insert(NodeFlags::HAS_OVERLAY_MOUNT);
        self.logical_portals.entry(owner).expect("portal owner key").or_default().push(node);
        self.add_style_bloom(owner, self.nodes[node].style_bloom);
    }

    fn unindex_portal(&mut self, node: NodeId) {
        let Some(owner) = self.portal_owner(node) else { return };
        self.nodes[node].flags.remove(NodeFlags::HAS_OVERLAY_MOUNT);
        let portals = &mut self.logical_portals[owner];
        portals.retain(|other| *other != node);
        if portals.is_empty() {
            self.logical_portals.remove(owner);
        }
        self.refresh_style_bloom(owner);
    }

    pub fn children(&self, node: NodeId) -> &[NodeId] {
        if let Some(text) = self.text(node) {
            return &text.links;
        }
        if !self.has_table(node, TableMask::CHILDREN) {
            return &[];
        }
        self.tables.children.get(node).map_or(&[], Vec::as_slice)
    }

    /// Sibling stacking priority. Use authored order for ties.
    pub fn stacking_key(&self, node: NodeId) -> (bool, i32) {
        (node == self.overlay_root, self.style(node).map_or(0, |style| style.visual.z_index))
    }

    pub fn subtree(&self, node: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let mut remaining: SmallVec<[NodeId; 16]> = self.node(node).map(|_| node).into_iter().collect();
        std::iter::from_fn(move || {
            let node = remaining.pop()?;
            remaining.extend(self.children(node).iter().rev().copied());
            Some(node)
        })
    }

    pub fn is_live(&self, node: NodeId) -> bool {
        self.node(node).is_some_and(|core| core.flags.contains(NodeFlags::ATTACHED) && !core.flags.contains(NodeFlags::EXITING))
    }

    pub fn guard(&self, node: NodeId) -> TargetGuard {
        TargetGuard::for_node(self.id, node)
    }

    pub fn contains_target(&self, target: TargetGuard) -> bool {
        target.tree == self.id
            && match target.target {
                GuardTarget::Window => true,
                GuardTarget::Node(node) => self.is_live(node),
            }
    }

    pub fn node_ref(&self, id: NodeId) -> NodeRef {
        NodeRef::Existing { tree: self.id, id }
    }

    pub fn style(&self, node: NodeId) -> Option<&StyleSlot> {
        self.has_table(node, TableMask::STYLES).then_some(())?;
        self.tables.styles.get(node)
    }
    pub fn text(&self, node: NodeId) -> Option<&TextSlot> {
        self.has_table(node, TableMask::TEXTS).then_some(())?;
        self.tables.texts.get(node)
    }
    pub fn link(&self, node: NodeId) -> Option<&LinkSlot> {
        self.has_table(node, TableMask::LINKS).then_some(())?;
        self.tables.links.get(node)
    }
    pub fn editor(&self, node: NodeId) -> Option<&EditorSlot> {
        self.has_table(node, TableMask::EDITORS).then_some(())?;
        self.tables.editors.get(node)
    }
    pub fn image(&self, node: NodeId) -> Option<&ImageSlot> {
        self.has_table(node, TableMask::IMAGES).then_some(())?;
        self.tables.images.get(node)
    }
    pub fn media_source(&self, node: NodeId) -> Option<&MediaSourceSlot> {
        self.has_table(node, TableMask::MEDIA_SOURCES).then_some(())?;
        self.tables.media_sources.get(node)
    }

    pub fn media_sources(&self) -> impl Iterator<Item = (NodeId, &MediaSourceSlot)> + '_ {
        self.tables.media_sources.iter().filter(|(node, _)| self.is_live(*node))
    }

    pub fn inline_media_source(&self, node: NodeId, index: usize) -> Option<&MediaSourceSlot> {
        self.tables.inline_media.get(node)?.get(&index)
    }

    pub fn inline_media_on(&self, node: NodeId) -> impl Iterator<Item = (usize, &MediaSourceSlot)> + '_ {
        self.tables.inline_media.get(node).into_iter().flatten().map(|(&index, slot)| (index, slot))
    }

    pub fn inline_media_sources(&self) -> impl Iterator<Item = (NodeId, usize, &MediaSourceSlot)> + '_ {
        self.tables
            .inline_media
            .iter()
            .filter(|(node, _)| self.is_live(*node))
            .flat_map(|(node, slots)| slots.iter().map(move |(&index, slot)| (node, index, slot)))
    }

    pub fn text_reveal(&self, node: NodeId) -> Option<&octara_model::motion::TextReveal> {
        self.has_table(node, TableMask::TEXT_REVEALS).then_some(())?;
        self.tables.text_reveals.get(node)
    }

    pub fn text_reveals(&self) -> impl Iterator<Item = (NodeId, &octara_model::motion::TextReveal)> + '_ {
        self.tables.text_reveals.iter().filter(|(node, _)| self.is_live(*node))
    }

    pub fn icon(&self, node: NodeId) -> Option<&Arc<octara_model::paint::NodeIcon>> {
        self.has_table(node, TableMask::ICONS).then_some(())?;
        self.tables.icons.get(node)
    }
    pub fn canvas(&self, node: NodeId) -> Option<&[octara_model::paint::CanvasOp]> {
        self.has_table(node, TableMask::CANVASES).then_some(())?;
        self.tables.canvases.get(node).map(AsRef::as_ref)
    }
    pub fn native(&self, node: NodeId) -> Option<&NativeSlot> {
        self.has_table(node, TableMask::NATIVES).then_some(())?;
        self.tables.natives.get(node)
    }
    pub fn paint(&self, node: NodeId) -> Option<&PaintSlot> {
        self.has_table(node, TableMask::PAINTS).then_some(())?;
        self.tables.paints.get(node)
    }
    pub fn semantics(&self, node: NodeId) -> Option<&SemanticsSlot> {
        self.has_table(node, TableMask::SEMANTICS).then_some(())?;
        self.tables.semantics.get(node)
    }
    pub fn commands(&self, node: NodeId) -> Option<&CommandSlot> {
        self.has_table(node, TableMask::COMMANDS).then_some(())?;
        self.tables.commands.get(node)
    }
    pub fn overlay(&self, node: NodeId) -> Option<&OverlaySlot> {
        self.has_table(node, TableMask::OVERLAYS).then_some(())?;
        self.tables.overlays.get(node)
    }
    pub fn scroll(&self, node: NodeId) -> Option<&ScrollSlot> {
        self.has_table(node, TableMask::SCROLLS).then_some(())?;
        self.tables.scrolls.get(node)
    }

    /// Nodes with retained scroll state.
    pub fn scroll_nodes(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.tables.scrolls.keys()
    }

    /// Whether this port preserves the position of its visible content.
    pub fn maintains_scroll_anchor(&self, node: NodeId) -> bool {
        let Some(core) = self.node(node) else { return false };
        !core.flags.contains(NodeFlags::FOLLOW_TAIL_ACTIVE)
            && (core.flags.intersects(NodeFlags::FOLLOW_TAIL | NodeFlags::MAINTAIN_SCROLL_ANCHOR)
                || self.children(node).iter().any(|&child| self.nodes[child].flags.contains(NodeFlags::VIRTUAL_SCROLL_ANCHOR)))
    }
    pub fn provenance(&self, node: NodeId) -> Option<&Provenance> {
        self.has_table(node, TableMask::PROVENANCE).then_some(())?;
        self.tables.provenance.get(node)
    }
    pub fn doc_selection(&self, region: NodeId) -> Option<DocSelection> {
        self.doc_selections.get(&region).copied()
    }

    fn handler_slot(&self, node: NodeId) -> Option<&HandlerSlot> {
        self.has_table(node, TableMask::HANDLERS).then_some(())?;
        self.tables.handlers.get(node)
    }

    pub fn handler(&self, node: NodeId, kind: HandlerKind) -> Option<HandlerId> {
        self.handler_slot(node)?.callbacks.iter().find_map(|(candidate, handler)| (*candidate == kind).then_some(*handler))
    }

    pub fn drag_drop(&self, node: NodeId) -> Option<&DragDrop> {
        self.handler_slot(node)?.drag_drop.as_deref()
    }

    /// Mounted authoring IDs take precedence over diagnostic test IDs.
    pub fn named_nodes(&self, name: &str) -> &[NodeId] {
        self.id_index.get(name).map_or_else(|| self.test_ids(name), SmallVec::as_slice)
    }

    pub fn test_ids(&self, name: &str) -> &[NodeId] {
        self.test_id_index.get(name).map_or(&[], SmallVec::as_slice)
    }

    pub fn row_child(&self, parent: NodeId, key: &RowKey) -> Option<NodeId> {
        self.row_children.get(&(parent, key.clone())).copied()
    }

    pub fn overlay_nodes(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.overlay_index.iter().copied().filter(|&node| self.is_live(node))
    }

    /// Anchors whose references remain visible, with dependencies before nested overlays.
    pub fn anchored_nodes(&self) -> impl Iterator<Item = NodeId> + '_ {
        let mut anchors: Vec<_> = self
            .overlay_nodes()
            .filter_map(|node| {
                self.overlay(node)?.target?;
                if !self.is_shown(node) {
                    return None;
                }
                let depth = std::iter::successors(Some(node), |&node| self.anchor_or_parent(node)).count();
                Some((depth, node))
            })
            .collect();
        anchors.sort_unstable();
        anchors.into_iter().map(|(_, node)| node)
    }

    fn anchor_or_parent(&self, node: NodeId) -> Option<NodeId> {
        self.overlay(node).and_then(|slot| slot.target.and_then(|(target, _)| target.node())).or_else(|| self.physical_parent(node))
    }

    pub fn native_nodes(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.native_index.iter().copied().filter(|&node| self.is_live(node))
    }

    pub fn layout_style(&self, node: NodeId) -> LayoutStyle {
        self.style(node).map_or_else(LayoutStyle::default, |slot| slot.layout)
    }

    pub fn visual(&self, node: NodeId) -> Visual {
        if let Some(style) = self.style(node) {
            return style.visual;
        }
        let inherited = self.inherited_at(self.logical_parent(node));
        StyleResolver::new(self.theme_at(node), self.breakpoint, self.responsive_breakpoint, self.touch)
            .resolve(LayoutStyle::default(), &inherited, &[], &[], State::empty(), |_| false)
            .visual
    }

    /// The local text selection policy, if explicitly set.
    pub fn user_select(&self, node: NodeId) -> Option<UserSelect> {
        self.style(node).and_then(|style| style.visual.user_select).or_else(|| {
            let flags = self.node(node)?.flags;
            if flags.contains(NodeFlags::SELECT_NONE) {
                Some(UserSelect::None)
            } else {
                flags.contains(NodeFlags::SELECT_REGION).then_some(UserSelect::Text)
            }
        })
    }

    /// The first project sees both built-in roots, which precede authored transactions.
    pub fn initial_delta(&self) -> TreeDelta {
        let mut delta = TreeDelta::default();
        delta.created.extend([self.root, self.overlay_root]);
        delta.topology_dirty.insert(self.root);
        delta
    }

    fn allocate(&mut self, spec: NodeSpec, delta: &mut TreeDelta) -> NodeId {
        let mut core = NodeCore::new(spec.content.kind());
        if core.kind == NodeKind::TextInput {
            core.flags.insert(NodeFlags::FOCUSABLE);
        }
        core.present.insert(TableMask::PROVENANCE);
        let node = self.nodes.insert(core);
        self.tables
            .provenance
            .insert(node, Provenance { id: None, test_id: None, row_key: spec.row_key, component: spec.component, origin: spec.origin });
        match spec.content {
            NodeContent::Box => {}
            NodeContent::Text(text) => {
                self.nodes[node].present.insert(TableMask::TEXTS);
                self.tables.texts.insert(node, TextSlot { text, ..Default::default() });
            }
            NodeContent::TextInput(editor) => {
                self.nodes[node].present.insert(TableMask::EDITORS);
                self.tables.editors.insert(node, *editor);
            }
            NodeContent::Image { image, fit } => {
                self.nodes[node].present.insert(TableMask::IMAGES);
                self.tables.images.insert(node, ImageSlot { image, fit });
            }
            NodeContent::ImageSource { source, fit } => {
                self.nodes[node].present.insert(TableMask::MEDIA_SOURCES);
                self.tables.media_sources.insert(node, MediaSourceSlot::new(source, fit));
            }
            NodeContent::Icon(icon) => {
                self.nodes[node].present.insert(TableMask::ICONS);
                self.tables.icons.insert(node, icon);
            }
            NodeContent::Canvas(canvas) => {
                self.nodes[node].present.insert(TableMask::CANVASES);
                self.tables.canvases.insert(node, canvas);
            }
            NodeContent::Native { kind, slot } => {
                self.nodes[node].flags.insert(NodeFlags::HAS_NATIVE);
                self.nodes[node].present.insert(TableMask::NATIVES);
                self.tables
                    .natives
                    .insert(node, NativeSlot { kind, slot, props: serde_json::Value::Null, typed_props: None, props_rev: 0, live: false });
                self.native_index.insert(node);
            }
        }
        delta.created.push(node);
        self.refresh_style_bloom(node);
        self.sync_scroll_slot(node, delta);
        self.content_changed(node, delta);
        node
    }

    fn layout_changed(&mut self, node: NodeId, delta: &mut TreeDelta) {
        let rev = &mut self.nodes[node].layout_rev;
        *rev = rev.wrapping_add(1);
        delta.layout_dirty.insert(node);
    }

    fn paint_changed(&mut self, node: NodeId, delta: &mut TreeDelta) {
        let rev = &mut self.nodes[node].paint_rev;
        *rev = rev.wrapping_add(1);
        delta.paint_dirty.insert(node);
    }

    fn semantics_changed(&mut self, node: NodeId, delta: &mut TreeDelta) {
        let rev = &mut self.nodes[node].semantics_rev;
        *rev = rev.wrapping_add(1);
        delta.semantics_dirty.insert(node);
    }

    fn logical_semantics_changed(&mut self, root: NodeId, delta: &mut TreeDelta) {
        let mut pending: SmallVec<[NodeId; 8]> = smallvec::smallvec![root];
        while let Some(node) = pending.pop() {
            self.semantics_changed(node, delta);
            pending.extend(self.logical_children(node));
        }
    }

    fn content_changed(&mut self, node: NodeId, delta: &mut TreeDelta) {
        self.layout_changed(node, delta);
        self.paint_changed(node, delta);
        self.semantics_changed(node, delta);
    }

    fn index_names(&mut self, node: NodeId, add: bool) {
        if !self.has_table(node, TableMask::PROVENANCE) {
            return;
        }
        let provenance = &self.tables.provenance[node];
        for (id, index) in [(provenance.id.as_ref(), &mut self.id_index), (provenance.test_id.as_ref(), &mut self.test_id_index)] {
            let Some(id) = id else { continue };
            if add {
                index.entry(id.clone()).or_default().push(node);
            } else if let Some(nodes) = index.get_mut(id) {
                nodes.retain(|other| *other != node);
                if nodes.is_empty() {
                    index.remove(id);
                }
            }
        }
    }

    fn set_attachment(&mut self, root: NodeId, attached: bool, delta: &mut TreeDelta) {
        if self.nodes[root].flags.contains(NodeFlags::ATTACHED) == attached {
            return;
        }
        let subtree: SmallVec<[NodeId; 16]> = self.subtree(root).collect();
        for node in subtree {
            let core = &mut self.nodes[node];
            if core.flags.contains(NodeFlags::EXITING) {
                continue;
            }
            core.flags.set(NodeFlags::ATTACHED, attached);
            self.index_names(node, attached);
            self.semantics_changed(node, delta);
        }
    }

    fn detach(&mut self, child: NodeId, delta: &mut TreeDelta) {
        let Some(parent) = self.nodes[child].parent.take() else { return };
        let children = self.tables.children.get_mut(parent).expect("a placed node has a parent list");
        children.retain(|&node| node != child);
        if children.is_empty() {
            self.nodes[parent].present.remove(TableMask::CHILDREN);
            self.tables.children.remove(parent);
        }
        self.remove_row_index(parent, child);
        if self.nodes[child].style_bloom & !sensitivity::DEFAULT != 0 {
            self.refresh_style_bloom(parent);
        }
        self.set_attachment(child, false, delta);
        delta.topology_dirty.insert(parent);
        self.layout_changed(parent, delta);
    }

    fn attach(&mut self, parent: NodeId, index: usize, child: NodeId, delta: &mut TreeDelta) {
        self.nodes[child].parent = Some(parent);
        self.nodes[parent].present.insert(TableMask::CHILDREN);
        let children = self.tables.children.entry(parent).expect("live parent key").or_default();
        let candidates =
            children.iter().enumerate().filter(|&(_, &id)| id != self.overlay_root && !self.nodes[id].flags.contains(NodeFlags::EXITING));
        let index = candidates
            .map(|(index, _)| index)
            .nth(index)
            .unwrap_or_else(|| children.iter().position(|&id| id == self.overlay_root).unwrap_or(children.len()));
        children.insert(index, child);
        if let Some(key) = self.provenance(child).and_then(|p| p.row_key.clone()) {
            self.row_children.insert((parent, key), child);
        }
        self.set_attachment(child, self.is_live(parent), delta);
        self.add_style_bloom(parent, self.nodes[child].style_bloom);
        delta.topology_dirty.insert(parent);
        self.layout_changed(parent, delta);
    }

    fn remove_row_index(&mut self, parent: NodeId, node: NodeId) {
        if let Some(key) = self.provenance(node).and_then(|p| p.row_key.clone())
            && self.row_children.get(&(parent, key.clone())) == Some(&node)
        {
            self.row_children.remove(&(parent, key));
        }
    }

    fn clear_node(&mut self, node: NodeId, delta: &mut TreeDelta) {
        if let Some(parent) = self.physical_parent(node) {
            self.remove_row_index(parent, node);
        }
        self.index_names(node, false);
        self.overlay_index.remove(&node);
        self.native_index.remove(&node);
        self.doc_selections.remove(&node);
        if self.nodes[node].present.contains(TableMask::STYLES) {
            self.nodes[node].present.remove(TableMask::STYLES);
            let style = self.tables.styles.remove(node).expect("style slot");
            self.style_interner.release(style.resolved);
        }
        if self.nodes[node].present.contains(TableMask::NATIVES) {
            delta.retired_native_props.extend(self.tables.natives[node].typed_props.take());
        }
        if let Some(asset) = self.tables.remove(node, self.nodes[node].present) {
            delta.retired_assets.push(asset);
        }
        self.nodes.remove(node);
        delta.retired.push(RetiredNode::Destroyed(node));
    }

    fn replace_text(&mut self, node: NodeId, mut rich: RichText, delta: &mut TreeDelta) {
        let old = &mut self.tables.texts[node];
        rich.attachments.sort_by_key(|attachment| (attachment.range.start, attachment.range.end));
        if old.links.is_empty()
            && rich.links.is_empty()
            && old.runs == rich.runs
            && old.attachments == rich.attachments
            && old.text == rich.text
        {
            return;
        }
        let revision = old.text_rev.wrapping_add(1);
        let retired = std::mem::take(&mut old.links);
        for link in retired {
            self.clear_node(link, delta);
        }
        let mut links = Vec::with_capacity(rich.links.len());
        let attached = self.is_live(node);
        for link in rich.links {
            let mut core = NodeCore { parent: Some(node), ..NodeCore::new(NodeKind::Link) };
            core.flags.set(NodeFlags::ATTACHED, attached);
            core.flags.insert(NodeFlags::FOCUSABLE);
            let id = self.nodes.insert(core);
            self.nodes[id].present.insert(TableMask::LINKS);
            self.tables.links.insert(id, link);
            links.push(id);
            delta.created.push(id);
            self.semantics_changed(id, delta);
        }
        if self.has_table(node, TableMask::INLINE_MEDIA)
            || rich.attachments.iter().any(|attachment| matches!(attachment.content, octara_model::text::InlineContent::ImageSource { .. }))
        {
            use octara_model::text::InlineContent;
            // Reconcile image occurrences in text order even when preceding objects change their indices.
            let mut previous = self.tables.inline_media.remove(node).into_iter().flat_map(BTreeMap::into_values);
            let slots: BTreeMap<_, _> = rich
                .attachments
                .iter()
                .enumerate()
                .filter_map(|(index, attachment)| {
                    let InlineContent::ImageSource { source, fit, .. } = &attachment.content else { return None };
                    let slot = previous
                        .next()
                        .filter(|slot| slot.source == *source && slot.fit == *fit)
                        .unwrap_or_else(|| MediaSourceSlot::new(source.clone(), *fit));
                    Some((index, slot))
                })
                .collect();
            self.nodes[node].present.set(TableMask::INLINE_MEDIA, !slots.is_empty());
            if !slots.is_empty() {
                self.tables.inline_media.insert(node, slots);
            }
        }
        self.nodes[node].present.insert(TableMask::TEXTS);
        self.tables
            .texts
            .insert(node, TextSlot { text: rich.text, runs: rich.runs, attachments: rich.attachments, links, text_rev: revision });
        delta.topology_dirty.insert(node);
        self.content_changed(node, delta);
    }

    fn commit_change(
        &mut self,
        node: NodeId,
        change: NodeChange,
        resolve: &impl Fn(NodeRef) -> NodeId,
        style_roots: &mut FxHashSet<NodeId>,
        theme_roots: &mut FxHashSet<NodeId>,
        delta: &mut TreeDelta,
    ) {
        let sensitivity_changed = matches!(
            change,
            NodeChange::Style(_)
                | NodeChange::ThemePatch(_)
                | NodeChange::Text(_)
                | NodeChange::RichText(_)
                | NodeChange::SpliceText { .. }
                | NodeChange::TextReveal(_)
                | NodeChange::Paint(_)
                | NodeChange::Overlay(_)
        );
        match change {
            NodeChange::Style(authored) => {
                let from = self.inherited_at(self.logical_parent(node));
                self.nodes[node].present.insert(TableMask::STYLES);
                let style = self.tables.styles.entry(node).expect("style target key").or_insert_with(|| StyleSlot {
                    theme_revision: self.root_theme_revision,
                    sensitivity: 0,
                    authored: AuthoredStyle::default(),
                    resolved: Arc::clone(&self.initial_style),
                    inherited: from,
                    inherited_from: from,
                    opacity_motion_authored: false,
                });
                style.sensitivity = sensitivity::authored(&authored);
                style.authored = *authored;
                style_roots.insert(node);
            }
            NodeChange::ThemePatch(patch) => {
                if self.theme_patch(node) == patch.as_deref() {
                    return;
                }
                if let Some(scope) = self.tables.theme_scopes.get_mut(node) {
                    scope.base_revision = None;
                }
                self.nodes[node].present.set(TableMask::THEME_PATCHES, patch.is_some());
                if let Some(patch) = patch {
                    self.tables.theme_patches.insert(node, *patch);
                } else {
                    self.tables.theme_patches.remove(node);
                }
                theme_roots.insert(node);
            }
            NodeChange::Flags(flags) => {
                let core = &mut self.nodes[node];
                let pointer_events_none = flags.contains(NodeFlags::POINTER_EVENTS_NONE);
                if core.authored_pointer_events_none != pointer_events_none {
                    core.authored_pointer_events_none = pointer_events_none;
                    style_roots.insert(node);
                }
                let authored = flags & NodeFlags::AUTHORED;
                let following = authored.contains(NodeFlags::FOLLOW_TAIL);
                if following != core.flags.contains(NodeFlags::FOLLOW_TAIL) {
                    core.flags.set(NodeFlags::FOLLOW_TAIL_ACTIVE, following);
                    delta.scroll_dirty.insert(node);
                }
                if authored.contains(NodeFlags::MAINTAIN_SCROLL_ANCHOR) != core.flags.contains(NodeFlags::MAINTAIN_SCROLL_ANCHOR) {
                    delta.scroll_dirty.insert(node);
                }
                let inert_changed = core.flags.contains(NodeFlags::INERT) != authored.contains(NodeFlags::INERT);
                core.flags = (core.flags - NodeFlags::AUTHORED) | authored;
                if inert_changed {
                    self.logical_semantics_changed(node, delta);
                } else {
                    self.semantics_changed(node, delta);
                }
            }
            NodeChange::Disabled(disabled) => {
                if self.nodes[node].state.contains(State::DISABLED) == disabled {
                    return;
                }
                self.nodes[node].state.set(State::DISABLED, disabled);
                style_roots.insert(node);
                self.logical_semantics_changed(node, delta);
            }
            NodeChange::Text(text) => self.replace_text(node, RichText { text, ..Default::default() }, delta),
            NodeChange::RichText(rich) => self.replace_text(node, *rich, delta),
            NodeChange::SpliceText { range, insert } => {
                let mut text = self.tables.texts[node].text.to_string();
                text.replace_range(range, &insert);
                self.replace_text(node, RichText { text: text.into(), ..Default::default() }, delta);
            }
            NodeChange::EditorValue(value) => {
                if self.tables.editors[node].replace_value(value) {
                    self.editor_changed(node, delta);
                }
            }
            NodeChange::EditorRichText(rich) => {
                if self.tables.editors[node].set_rich_text(*rich) {
                    self.editor_changed(node, delta);
                }
            }
            NodeChange::EditorPlaceholder(placeholder) => {
                self.tables.editors[node].placeholder = placeholder;
                self.editor_changed(node, delta);
            }
            NodeChange::EditorSecure(secure) => {
                if self.tables.editors[node].set_secure(secure) {
                    self.editor_changed(node, delta);
                }
            }
            NodeChange::EditorInputTraits(traits) => {
                let editor = &mut self.tables.editors[node];
                if editor.input_traits != traits {
                    editor.input_traits = traits;
                    delta.editor_traits_dirty.insert(node);
                }
            }
            NodeChange::TextReveal(reveal) => {
                if self.text_reveal(node).copied() == reveal {
                    return;
                }
                self.nodes[node].present.set(TableMask::TEXT_REVEALS, reveal.is_some());
                if let Some(reveal) = reveal {
                    self.tables.text_reveals.insert(node, reveal);
                } else {
                    self.tables.text_reveals.remove(node);
                }
                self.paint_changed(node, delta);
            }
            NodeChange::Image { image, fit } => {
                self.nodes[node].present.insert(TableMask::IMAGES);
                self.tables.images.insert(node, ImageSlot { image, fit });
                self.content_changed(node, delta);
            }
            NodeChange::ImageSource { source, fit } => {
                self.nodes[node].present.insert(TableMask::MEDIA_SOURCES);
                if let Some(slot) = self.tables.media_sources.get_mut(node)
                    && slot.source == source
                {
                    if slot.fit == fit {
                        return;
                    }
                    slot.fit = fit;
                } else {
                    self.tables.media_sources.insert(node, MediaSourceSlot::new(source, fit));
                }
                self.content_changed(node, delta);
            }
            NodeChange::Icon(icon) => {
                self.nodes[node].present.insert(TableMask::ICONS);
                self.tables.icons.insert(node, icon);
                self.content_changed(node, delta);
            }
            NodeChange::Canvas(canvas) => {
                self.nodes[node].present.insert(TableMask::CANVASES);
                self.tables.canvases.insert(node, canvas);
                self.paint_changed(node, delta);
            }
            NodeChange::NativeProps(props) => {
                let native = &mut self.tables.natives[node];
                if native.typed_props.is_none() && native.props != props {
                    native.props = props;
                    native.props_rev = native.props_rev.wrapping_add(1);
                    self.content_changed(node, delta);
                }
            }
            NodeChange::TypedNativeProps(props) => {
                let native = &mut self.tables.natives[node];
                if native.typed_props.as_ref() != Some(&props) {
                    delta.retired_native_props.extend(native.typed_props.replace(props));
                    native.props_rev = native.props_rev.wrapping_add(1);
                    self.content_changed(node, delta);
                }
            }
            NodeChange::Paint(paint) => {
                let has_custom = paint.custom.is_some() || paint.transition.is_some();
                self.nodes[node].flags.set(NodeFlags::HAS_CUSTOM_PAINT, has_custom);
                self.nodes[node].present.set(TableMask::PAINTS, has_custom);
                let old = if has_custom { self.tables.paints.insert(node, *paint) } else { self.tables.paints.remove(node) };
                delta.retired_assets.extend(old);
                self.paint_changed(node, delta);
            }
            NodeChange::Semantics(mut semantics) => {
                let old = self.semantics(node);
                if old.and_then(|slot| slot.focus_group).is_some() != semantics.focus_group.is_some() {
                    let children: Vec<_> = self.subtree(node).skip(1).collect();
                    for child in children {
                        self.semantics_changed(child, delta);
                    }
                }
                semantics.group_current =
                    self.semantics(node).and_then(|old| old.group_current).filter(|_| semantics.focus_group.is_some());
                let state = &mut self.nodes[node].state;
                state.set(State::CHECKED, semantics.checked == Some(octara_model::semantics::Checked::Checked));
                state.set(State::SELECTED, semantics.selected == Some(true));
                state.set(State::EXPANDED, semantics.expanded == Some(true));
                self.nodes[node].present.insert(TableMask::SEMANTICS);
                self.tables.semantics.insert(node, *semantics);
                self.semantics_changed(node, delta);
                style_roots.insert(node);
            }
            NodeChange::Handler { kind, id } => {
                let handlers = self.tables.handlers.entry(node).expect("handler target key").or_default();
                handlers.callbacks.retain(|(old, _)| *old != kind);
                handlers.callbacks.extend(id.map(|id| (kind, id)));
                let empty = handlers.callbacks.is_empty() && handlers.drag_drop.is_none();
                self.nodes[node].present.set(TableMask::HANDLERS, !empty);
                if empty {
                    self.tables.handlers.remove(node);
                }
                self.semantics_changed(node, delta);
            }
            NodeChange::DragDrop(drag_drop) => {
                let handlers = self.tables.handlers.entry(node).expect("drag-and-drop target key").or_default();
                handlers.drag_drop = drag_drop;
                let empty = handlers.callbacks.is_empty() && handlers.drag_drop.is_none();
                self.nodes[node].present.set(TableMask::HANDLERS, !empty);
                if empty {
                    self.tables.handlers.remove(node);
                }
                self.paint_changed(node, delta);
            }
            NodeChange::Commands(commands) => {
                if commands.key_context.is_none() && commands.commands.is_empty() {
                    self.nodes[node].present.remove(TableMask::COMMANDS);
                    self.tables.commands.remove(node);
                } else {
                    self.nodes[node].present.insert(TableMask::COMMANDS);
                    self.tables.commands.insert(node, *commands);
                }
            }
            NodeChange::Overlay(spec) => {
                let target = spec.target.map(|(target, placement)| (target.map_node(resolve), placement));
                let mount = spec.mount.map(|(owner, order)| OverlayMount { owner: resolve(owner), order });
                self.nodes[node].flags.set(NodeFlags::HAS_ANCHOR, target.is_some());
                let has_overlay = target.is_some() || mount.is_some() || spec.layer.is_some() || spec.group.is_some();
                self.nodes[node].present.set(TableMask::OVERLAYS, has_overlay);
                if !has_overlay {
                    self.overlay_index.remove(&node);
                    self.tables.overlays.remove(node);
                } else {
                    let placed = self.overlay(node).filter(|old| old.target == target).and_then(|old| old.placed);
                    self.overlay_index.insert(node);
                    self.tables.overlays.insert(node, OverlaySlot { target, placed, mount, layer: spec.layer, group: spec.group });
                }
                if target.is_some() {
                    delta.geometry_demands.insert(GeometryDemand::Anchor(node));
                }
                self.sync_scroll_slot(node, delta);
                delta.overlay_dirty.insert(node);
                self.semantics_changed(node, delta);
            }
            NodeChange::Id(_) | NodeChange::TestId(_) => {
                let attached = self.is_live(node);
                if attached {
                    self.index_names(node, false);
                }
                self.nodes[node].present.insert(TableMask::PROVENANCE);
                let provenance = self.tables.provenance.entry(node).expect("provenance target key").or_default();
                match change {
                    NodeChange::Id(id) => provenance.id = id,
                    NodeChange::TestId(id) => provenance.test_id = id,
                    _ => unreachable!("name change"),
                }
                if attached {
                    self.index_names(node, true);
                }
                self.semantics_changed(node, delta);
            }
            NodeChange::DemandBounds => {
                delta.geometry_demands.insert(GeometryDemand::Bounds(node));
            }
        }
        if sensitivity_changed {
            self.refresh_style_bloom(node);
        }
    }

    fn sync_scroll_slot(&mut self, node: NodeId, delta: &mut TreeDelta) {
        let style = self.layout_style(node);
        let scrollable = style.overflow_x.scrolls()
            || style.overflow_y.scrolls()
            || self.editor(node).is_some_and(|editor| editor.multiline)
            || self.overlay(node).and_then(|overlay| overlay.target).is_some_and(|(_, placement)| placement.fit);
        if scrollable && !self.has_table(node, TableMask::SCROLLS) {
            self.nodes[node].present.insert(TableMask::SCROLLS);
            self.tables.scrolls.insert(node, ScrollSlot::default());
            delta.geometry_demands.insert(GeometryDemand::Scrollport(node));
        } else if !scrollable && self.has_table(node, TableMask::SCROLLS) {
            self.nodes[node].present.remove(TableMask::SCROLLS);
            self.tables.scrolls.remove(node);
            delta.scroll_dirty.insert(node);
        }
    }

    fn editor_changed(&mut self, node: NodeId, delta: &mut TreeDelta) {
        delta.editor_dirty.insert(node);
        delta.geometry_demands.insert(GeometryDemand::Editor(node));
        self.content_changed(node, delta);
    }

    fn reconcile_focus(&mut self, delta: &mut TreeDelta) {
        let mut restore = std::mem::take(&mut self.focus_restore);
        let next = restore.reconcile(self).or_else(|| {
            if self.focus.is_some() {
                return None;
            }
            delta
                .created
                .iter()
                .copied()
                .find(|&node| {
                    self.node(node).is_some_and(|core| core.flags.contains(NodeFlags::AUTOFOCUS))
                        && self.is_focusable(node)
                        && !self.is_in_layer_content(node)
                })
                .map(Some)
        });
        if let Some(focus) = next {
            self.change_focus(focus, self.focus_visible(), delta);
        }
        restore.remember(self, self.focus);
        self.focus_restore = restore;
        self.reconcile_group_stops(delta);
    }

    fn selection_projections(&self) -> impl Iterator<Item = (NodeId, SelectionProjection)> + '_ {
        self.doc_selections.iter().map(|(&region, &selection)| (region, self.selected_text_ranges(region, selection).unwrap_or_default()))
    }

    fn revalidate_selection(&mut self, previous: Vec<(NodeId, SelectionProjection)>, delta: &mut TreeDelta) {
        for (region, old) in previous {
            let current = self.doc_selection(region).and_then(|range| self.selected_text_ranges(region, range));
            if current.as_ref() == Some(&old) {
                continue;
            }
            if current.is_none() {
                self.doc_selections.remove(&region);
            }
            if self.node(region).is_some() {
                self.semantics_changed(region, delta);
            }
            let dirty: FxHashSet<_> = old.into_iter().chain(current.into_iter().flatten()).map(|(node, _)| node).collect();
            for node in dirty {
                if self.is_live(node) {
                    self.paint_changed(node, delta);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_core_fits_hot_memory_budget() {
        let bytes = std::mem::size_of::<NodeCore>();
        println!("NodeCore: {bytes} bytes");
        assert!(bytes <= 48);
    }
}

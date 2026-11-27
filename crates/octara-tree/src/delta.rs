//! Incremental changes produced by the authored and runtime write gates.

use crate::NodeId;
use crate::tables::PaintSlot;
use octara_model::diagnostics::Diagnostic;
use octara_model::identity::ExitTicket;
use rustc_hash::FxHashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetiredNode {
    Exiting { ticket: ExitTicket, visible: bool },
    Destroyed(NodeId),
}

impl RetiredNode {
    pub fn node(self) -> NodeId {
        match self {
            Self::Exiting { ticket, .. } => ticket.node,
            Self::Destroyed(node) => node,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GeometryDemand {
    Bounds(NodeId),
    Scrollport(NodeId),
    Editor(NodeId),
    Anchor(NodeId),
}

#[derive(Debug, Default)]
pub struct TreeDelta {
    pub created: Vec<NodeId>,
    pub retired: Vec<RetiredNode>,
    pub topology_dirty: FxHashSet<NodeId>,
    pub layout_dirty: FxHashSet<NodeId>,
    pub paint_dirty: FxHashSet<NodeId>,
    pub semantics_dirty: FxHashSet<NodeId>,
    pub scroll_dirty: FxHashSet<NodeId>,
    pub editor_dirty: FxHashSet<NodeId>,
    pub overlay_dirty: FxHashSet<NodeId>,
    pub geometry_demands: FxHashSet<GeometryDemand>,
    pub diagnostics: Vec<Diagnostic>,
    pub(crate) retired_assets: Vec<PaintSlot>,
}

impl TreeDelta {
    pub fn is_empty(&self) -> bool {
        self.created.is_empty()
            && self.retired.is_empty()
            && self.topology_dirty.is_empty()
            && self.layout_dirty.is_empty()
            && self.paint_dirty.is_empty()
            && self.semantics_dirty.is_empty()
            && self.scroll_dirty.is_empty()
            && self.editor_dirty.is_empty()
            && self.overlay_dirty.is_empty()
            && self.geometry_demands.is_empty()
            && self.diagnostics.is_empty()
    }

    pub fn merge(&mut self, next: Self) {
        self.created.extend(next.created);
        self.retired.extend(next.retired);
        self.topology_dirty.extend(next.topology_dirty);
        self.layout_dirty.extend(next.layout_dirty);
        self.paint_dirty.extend(next.paint_dirty);
        self.semantics_dirty.extend(next.semantics_dirty);
        self.scroll_dirty.extend(next.scroll_dirty);
        self.editor_dirty.extend(next.editor_dirty);
        self.overlay_dirty.extend(next.overlay_dirty);
        self.geometry_demands.extend(next.geometry_demands);
        self.diagnostics.extend(next.diagnostics);
        self.retired_assets.extend(next.retired_assets);
    }

    /// Release these assets outside the tree borrow, before entering user code again.
    pub fn take_retired_assets(&mut self) -> Vec<PaintSlot> {
        std::mem::take(&mut self.retired_assets)
    }
}

//! Incremental changes produced by the authored and runtime write gates.

use crate::tables::PaintSlot;
use crate::{DirtyBitmap, NodeId};
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
    pub topology_dirty: DirtyBitmap,
    pub layout_dirty: DirtyBitmap,
    pub paint_dirty: DirtyBitmap,
    pub semantics_dirty: DirtyBitmap,
    pub scroll_dirty: DirtyBitmap,
    pub editor_dirty: DirtyBitmap,
    pub overlay_dirty: DirtyBitmap,
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
        self.topology_dirty.union_with(&next.topology_dirty);
        self.layout_dirty.union_with(&next.layout_dirty);
        self.paint_dirty.union_with(&next.paint_dirty);
        self.semantics_dirty.union_with(&next.semantics_dirty);
        self.scroll_dirty.union_with(&next.scroll_dirty);
        self.editor_dirty.union_with(&next.editor_dirty);
        self.overlay_dirty.union_with(&next.overlay_dirty);
        self.geometry_demands.extend(next.geometry_demands);
        self.diagnostics.extend(next.diagnostics);
        self.retired_assets.extend(next.retired_assets);
    }

    /// Release these assets outside the tree borrow, before entering user code again.
    pub fn take_retired_assets(&mut self) -> Vec<PaintSlot> {
        std::mem::take(&mut self.retired_assets)
    }
}

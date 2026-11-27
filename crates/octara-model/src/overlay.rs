//! Window layer policy shared by authoring, the tree, and hosts.

use crate::CacheLine;
use crate::geometry::{Point, Rect};
use crate::identity::NodeId;
use std::sync::atomic::{AtomicU64, Ordering};

/// Geometry used to place a window overlay.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AnchorTarget<N = NodeId> {
    Node(N),
    Point(Point),
    Rect(Rect),
    EditorCaret(N),
    TextSelection(N),
}

impl<N> AnchorTarget<N> {
    pub fn node(self) -> Option<N> {
        match self {
            Self::Node(node) | Self::EditorCaret(node) | Self::TextSelection(node) => Some(node),
            Self::Point(_) | Self::Rect(_) => None,
        }
    }

    pub fn map_node<M>(self, mut map: impl FnMut(N) -> M) -> AnchorTarget<M> {
        match self {
            Self::Node(node) => AnchorTarget::Node(map(node)),
            Self::Point(point) => AnchorTarget::Point(point),
            Self::Rect(rect) => AnchorTarget::Rect(rect),
            Self::EditorCaret(node) => AnchorTarget::EditorCaret(map(node)),
            Self::TextSelection(node) => AnchorTarget::TextSelection(map(node)),
        }
    }

    pub fn fixed_rect(self) -> Option<Rect> {
        let rect = match self {
            Self::Point(point) => Rect::new(point.x, point.y, 0.0, 0.0),
            Self::Rect(rect) => rect,
            _ => return None,
        };
        ([rect.x, rect.y, rect.w, rect.h].into_iter().all(f32::is_finite) && rect.w >= 0.0 && rect.h >= 0.0).then_some(rect)
    }
}

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct FocusTrapFeatures: u8 {
        const NONE = 0;
        const INITIAL_FOCUS = 1 << 0;
        const TAB_LOCK = 1 << 1;
        const FOCUS_LOCK = 1 << 2;
        const RESTORE_FOCUS = 1 << 3;
        const AUTO_FOCUS = 1 << 4;
        const ALL = Self::INITIAL_FOCUS.bits() | Self::TAB_LOCK.bits() | Self::FOCUS_LOCK.bits()
            | Self::RESTORE_FOCUS.bits() | Self::AUTO_FOCUS.bits();
    }
}

/// Shared interaction scope for related layers, triggers, and other authored elements.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LayerGroupId(u64);

impl LayerGroupId {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Default for LayerGroupId {
    fn default() -> Self {
        static NEXT: CacheLine<AtomicU64> = CacheLine(AtomicU64::new(0));
        Self(NEXT.try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1)).expect("layer group identifier exhausted"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayerPolicy {
    pub inert_outside: bool,
    pub focus_trap: FocusTrapFeatures,
    pub dismiss_on_escape: bool,
    pub dismiss_on_outside_press: bool,
    pub dismiss_on_focus_leave: bool,
    pub group: Option<LayerGroupId>,
}

impl LayerPolicy {
    pub const fn modal() -> Self {
        Self {
            inert_outside: true,
            focus_trap: FocusTrapFeatures::ALL,
            dismiss_on_escape: true,
            dismiss_on_outside_press: true,
            dismiss_on_focus_leave: false,
            group: None,
        }
    }

    pub const fn non_modal() -> Self {
        Self {
            inert_outside: false,
            focus_trap: FocusTrapFeatures::NONE,
            dismiss_on_escape: true,
            dismiss_on_outside_press: true,
            dismiss_on_focus_leave: true,
            group: None,
        }
    }
}

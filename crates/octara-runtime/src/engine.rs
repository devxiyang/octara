//! Staging and publication of host-specific projections.

use octara_model::ask::{Ask, AskId, AskRoute, Menu, ServiceId, ServiceRequest};
use octara_model::geometry::{LayoutBox, Point, Rect, Vec2, WindowViewport};
use octara_model::input::{ContactTarget, HostReply, Modifiers};
use octara_model::paint::DisplayList;
use octara_model::semantics::TextClip;
use octara_model::text::TextGeometry;
use octara_model::window::WindowMaterial;
use octara_tree::arbiter::{ScrollbarHit, ScrollbarKey};
use octara_tree::{NodeId, RuntimeOp, Tree, TreeDelta};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug, Default)]
pub struct CommitOutput {
    pub draw: Option<Arc<DisplayList>>,
    pub frame: Option<u64>,
}

#[derive(Debug, Default)]
pub struct ProjectFeedback {
    pub updates: Vec<RuntimeOp>,
    pub replies: Vec<HostReply>,
    pub next_deadline: Option<Duration>,
}

/// The host's current scrollable interval in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollRange {
    pub max: Vec2,
}

impl ScrollRange {
    pub fn clamp(self, target: Vec2) -> Vec2 {
        Vec2::new(target.x.max(0.0).min(self.max.x.max(0.0)), target.y.max(0.0).min(self.max.y.max(0.0)))
    }

    /// Scroll the minimum distance needed to show the target on one axis.
    pub fn reveal_axis(offset: f32, extent: f32, start: f32, end: f32) -> f32 {
        match (start < offset, end > offset + extent) {
            (true, false) => start.max(end - extent),
            (false, true) => start.min(end - extent),
            _ => offset,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectError(pub String);

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ProjectError {}

/// Independent consumers of a window's platform accessibility projection.
#[derive(Clone, Copy, Debug)]
pub enum AccessibilityConsumer {
    Platform,
    Inspector,
}

pub trait HostEngine {
    fn project(&mut self, tree: &Tree, delta: &TreeDelta, viewport: WindowViewport, now: f64) -> Result<ProjectFeedback, ProjectError>;
    fn commit(&mut self) -> CommitOutput;
    fn rollback(&mut self);
    fn hit_test(&self, tree: &Tree, pos: Point) -> Option<ContactTarget>;
    fn geometry(&self, node: NodeId) -> Option<LayoutBox>;

    /// Node bounds in viewport coordinates for pointer delivery.
    fn interaction_bounds(&self, _node: NodeId) -> Option<Rect> {
        None
    }

    /// Viewport bounds of an inline attachment in a laid-out text node.
    fn inline_attachment_bounds(&self, _node: NodeId, _index: usize) -> Option<Rect> {
        None
    }

    fn text_geometry(&self, _tree: &Tree, _node: NodeId) -> Option<TextGeometry> {
        None
    }

    /// The live viewport-space editor caret.
    fn caret_rect(&self, _tree: &Tree, _node: NodeId) -> Option<Rect> {
        None
    }

    /// The viewport-space bounds of the current selection.
    fn selection_rect(&self, _tree: &Tree, _node: NodeId) -> Option<Rect> {
        None
    }

    /// Whether the host owns text pointer selection and document shortcuts.
    fn platform_text_selection(&self) -> bool {
        false
    }

    /// UTF-8 caret and the cluster start under a viewport-space pointer.
    fn text_pointer_hit(&self, _node: NodeId, _pos: Point) -> Option<(usize, usize)> {
        None
    }

    fn text_vertical(&self, _node: NodeId, _byte: usize, _direction: i32, _column: Option<f32>) -> Option<(usize, f32)> {
        None
    }

    fn clipped_text(&self, _tree: &Tree) -> Vec<(NodeId, TextClip)> {
        Vec::new()
    }

    fn scroll_range(&self, _node: NodeId) -> Option<ScrollRange> {
        None
    }

    fn scroll_anchor(&self, _tree: &Tree, _node: NodeId) -> Option<NodeId> {
        None
    }

    fn scrollbar_hit(&self, _tree: &Tree, _position: Point) -> Option<ScrollbarHit> {
        None
    }

    fn update_scrollbars(&mut self, _hovered: Option<ScrollbarKey>, _dragged: Option<ScrollbarKey>) -> bool {
        false
    }

    /// The modifier represented by `mod` on the platform hosting this window.
    fn shortcut_modifier(&self) -> Modifiers {
        if cfg!(target_os = "macos") { Modifiers::META } else { Modifiers::CTRL }
    }

    fn native_reply(&mut self, _reply: &HostReply) {}

    fn set_accessibility_active(&self, _tree: &Tree, _consumer: AccessibilityConsumer, _active: bool) {}

    /// Update the host's accessibility motion policy before the next projection.
    fn set_reduced_motion(&mut self, _reduced_motion: bool) {}

    fn set_window_active(&mut self, _active: bool) {}

    /// Change the clear mode for the next projection. Return true if a new frame is required.
    fn set_window_material(&mut self, _material: WindowMaterial) -> bool {
        false
    }
}

/// Platform requests are dispatched by the host after a successful cycle.
pub trait PlatformServices {
    fn set_menus(&mut self, menus: &[Menu]);
    fn ask_route(&self, ask: &Ask) -> AskRoute;
    fn ask(&mut self, id: AskId, ask: Ask);
    fn withdraw_ask(&mut self, id: AskId);
    fn request_service(&mut self, id: ServiceId, request: ServiceRequest);
}

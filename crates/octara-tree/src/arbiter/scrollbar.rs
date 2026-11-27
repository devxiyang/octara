use super::{InteractionDecision, InteractionIntent, finite, guarded_node};
use crate::{NodeFlags, NodeId, Tree};
use octara_model::{
    geometry::{Point, Vec2},
    identity::TargetGuard,
    input::{InteractionEvent, PointerId},
    style::ScrollbarVisibility,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollbarAxis {
    Horizontal,
    Vertical,
}

impl ScrollbarAxis {
    pub fn position(self, point: Point) -> f32 {
        match self {
            Self::Horizontal => point.x,
            Self::Vertical => point.y,
        }
    }

    pub fn offset(self, offset: Vec2) -> f32 {
        self.position(Point::new(offset.x, offset.y))
    }

    pub fn vector(self, distance: f32) -> Vec2 {
        match self {
            Self::Horizontal => Vec2::new(distance, 0.0),
            Self::Vertical => Vec2::new(0.0, distance),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollbarKey {
    pub node: NodeId,
    pub axis: ScrollbarAxis,
}

#[derive(Clone, Copy, Debug)]
pub enum ScrollbarPart {
    Thumb { scale: f32 },
    Track { page_delta: f32 },
}

#[derive(Clone, Copy, Debug)]
pub struct ScrollbarHit {
    pub key: ScrollbarKey,
    pub part: ScrollbarPart,
}

#[derive(Debug)]
pub struct ScrollbarScroll {
    pub target: TargetGuard,
    axis: ScrollbarAxis,
    movement: Movement,
}

#[derive(Debug)]
enum Movement {
    Page(f32),
    To(f32),
}

impl ScrollbarScroll {
    pub fn delta(&self, tree: &Tree) -> Option<Vec2> {
        let node = guarded_node(tree, self.target, tree.front_modal()).filter(|&node| scrollbar_target(tree, node))?;
        let offset = self.axis.offset(tree.scroll(node)?.offset);
        let distance = match self.movement {
            Movement::Page(delta) => delta,
            Movement::To(target) => target - offset,
        };
        Some(self.axis.vector(distance))
    }
}

pub(super) fn scrollbar_target(tree: &Tree, node: NodeId) -> bool {
    tree.is_interactive(node)
        && tree.scroll(node).is_some()
        && tree.visual(node).scrollbar != ScrollbarVisibility::Hidden
        && tree.node(node).is_some_and(|core| !core.flags.contains(NodeFlags::POINTER_EVENTS_NONE))
}

#[derive(Clone, Copy)]
pub(super) struct ScrollbarCapture {
    id: PointerId,
    pub(super) key: ScrollbarKey,
    pub(super) target: TargetGuard,
    start: f32,
    offset: f32,
    pub(super) scale: f32,
}

impl ScrollbarCapture {
    pub(super) fn resolve(self, tree: &Tree) -> Option<NodeId> {
        self.target.node().filter(|&node| tree.contains_target(self.target) && scrollbar_target(tree, node))
    }

    pub(super) fn begin(tree: &Tree, id: PointerId, pos: Point, hit: ScrollbarHit, decision: &mut InteractionDecision) -> Self {
        let target = tree.guard(hit.key.node);
        let scale = match hit.part {
            ScrollbarPart::Thumb { scale } => scale,
            ScrollbarPart::Track { page_delta } => {
                decision.intents.push(InteractionIntent::Scrollbar(ScrollbarScroll {
                    target,
                    axis: hit.key.axis,
                    movement: Movement::Page(page_delta),
                }));
                0.0
            }
        };
        let offset = tree.scroll(hit.key.node).map_or(0.0, |slot| hit.key.axis.offset(slot.offset));
        Self { id, key: hit.key, target, start: hit.key.axis.position(pos), offset, scale }
    }

    pub(super) fn handle(self, event: InteractionEvent, capture: &mut Option<Self>, decision: &mut InteractionDecision) -> bool {
        match event {
            InteractionEvent::PointerDown { .. } => {}
            InteractionEvent::PointerMove { id, pos, .. } if id == self.id => self.move_to(pos, decision),
            InteractionEvent::PointerUp { id, pos, .. } if id == self.id => {
                self.move_to(pos, decision);
                *capture = None;
            }
            InteractionEvent::PointerCancel { id } if id == self.id => *capture = None,
            _ => return false,
        }
        true
    }

    fn move_to(self, pos: Point, decision: &mut InteractionDecision) {
        if self.scale > 0.0 && finite(pos) {
            let delta = self.key.axis.position(pos) - self.start;
            decision.intents.push(InteractionIntent::Scrollbar(ScrollbarScroll {
                target: self.target,
                axis: self.key.axis,
                movement: Movement::To(self.offset + self.scale * delta),
            }));
        }
    }
}

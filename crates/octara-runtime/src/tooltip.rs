//! Window-scoped hover timing. The authoring layer owns the tooltip view.

use octara_model::geometry::Point;
use octara_tree::NodeId;
use std::{sync::Arc, time::Duration};

const OPEN_DELAY: Duration = Duration::from_millis(450);
const WARM_HOP: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoveredTooltip {
    pub node: NodeId,
    pub text: Arc<str>,
}

#[derive(Default)]
pub(crate) struct HoverTooltip {
    position: Option<Point>,
    modal: Option<NodeId>,
    target: Option<NodeId>,
    deadline: Option<Duration>,
    warm_until: Option<Duration>,
    visible: bool,
}

impl HoverTooltip {
    pub(crate) fn pointer_moved(&mut self, position: Point, modal: Option<NodeId>) {
        if self.modal != modal {
            self.dismiss();
            self.modal = modal;
        }
        self.position = Some(position);
    }

    pub(crate) fn position(&self) -> Option<Point> {
        self.position
    }

    pub(crate) fn dismiss(&mut self) {
        *self = Self { modal: self.modal, ..Self::default() };
    }

    pub(crate) fn deadline(&self) -> Option<Duration> {
        self.deadline
    }

    pub(crate) fn update(&mut self, modal: Option<NodeId>, hovered: Option<(NodeId, &Arc<str>)>, now: Duration) -> Option<HoveredTooltip> {
        if self.modal != modal {
            self.dismiss();
            self.modal = modal;
            return None;
        }
        let target = hovered.map(|(node, _)| node);
        if target != self.target {
            if self.visible {
                self.warm_until = Some(now.saturating_add(WARM_HOP));
            }
            self.visible = false;
            self.target = target;
            self.deadline = target.map(|_| {
                let delay = if self.warm_until.is_some_and(|until| now <= until) { WARM_HOP } else { OPEN_DELAY };
                now.saturating_add(delay)
            });
        }
        if self.deadline.is_some_and(|deadline| now >= deadline) {
            self.deadline = None;
            self.visible = true;
        }
        hovered.filter(|_| self.visible).map(|(node, text)| HoveredTooltip { node, text: text.clone() })
    }
}

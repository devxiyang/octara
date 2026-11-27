use super::{Contact, ContactMode, InteractionArbiter, InteractionDecision, InteractionIntent, distance, finite, guarded_node};
use crate::{HandlerId, HandlerKind, Tree};
use octara_model::{
    geometry::Point,
    identity::{NodeId, TargetGuard},
    input::{Button, ContactTarget, GestureEvent, GesturePhase},
};
use std::time::Duration;

const LONG_PRESS_TIME: Duration = Duration::from_millis(500);
const LONG_PRESS_SLOP: f32 = 8.0;

/// A gesture routed from a guarded hit through its logical ancestors.
#[derive(Clone, Copy, Debug)]
pub struct GestureIntent {
    hit: TargetGuard,
    pub event: GestureEvent,
}

impl GestureIntent {
    pub fn handlers(self, tree: &Tree) -> impl Iterator<Item = HandlerId> + '_ {
        gesture_handlers(tree, self.hit)
    }
}

fn gesture_handlers(tree: &Tree, hit: TargetGuard) -> impl Iterator<Item = HandlerId> + '_ {
    let modal = tree.front_modal();
    std::iter::successors(guarded_node(tree, hit, modal), |&node| tree.logical_parent(node))
        .filter(move |&node| tree.interactive_in(node, modal))
        .filter_map(|node| tree.handler(node, HandlerKind::Gesture))
}

#[derive(Default)]
pub(super) struct HardwareGestures {
    pinch: Option<TargetGuard>,
    rotate: Option<TargetGuard>,
}

impl HardwareGestures {
    pub(super) fn reconcile(&mut self, tree: &Tree, modal: Option<NodeId>) {
        for hit in [&mut self.pinch, &mut self.rotate] {
            hit.take_if(|hit| guarded_node(tree, *hit, modal).is_none());
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum ContactGesture {
    Pending(Duration),
    Holding,
}

impl Contact {
    pub(super) fn begin_gesture(&mut self, tree: &Tree, now: Duration) {
        if self.button == Button::Primary && gesture_handlers(tree, self.target.hit).next().is_some() {
            self.gesture = Some(ContactGesture::Pending(now.saturating_add(LONG_PRESS_TIME)));
        }
    }

    fn long_press(&self, phase: GesturePhase, position: Point) -> GestureIntent {
        GestureIntent { hit: self.target.hit, event: GestureEvent::LongPress { phase, position, pointer: self.kind } }
    }

    fn within_hold_slop(&self, position: Point) -> bool {
        matches!(self.mode, ContactMode::Pressed | ContactMode::Spent) && distance(self.down, position) <= LONG_PRESS_SLOP
    }

    pub(super) fn cancel_gesture(&mut self, decision: &mut InteractionDecision) {
        if matches!(self.gesture.take(), Some(ContactGesture::Holding)) {
            decision.intents.push(InteractionIntent::Gesture(self.long_press(GesturePhase::Cancelled, self.at)));
        }
    }
}

impl InteractionArbiter {
    /// Hit-test the first sample and keep its guarded hit until the gesture ends.
    pub fn gesture(
        &mut self,
        tree: &Tree,
        event: GestureEvent,
        hit_test: impl FnOnce(Point) -> Option<TargetGuard>,
    ) -> Option<GestureIntent> {
        if !self.active {
            return None;
        }
        let position = event.position();
        let hit_test = || finite(position).then_some(position).and_then(hit_test);
        let (capture, phase) = match event {
            GestureEvent::Pinch { phase, .. } => (&mut self.hardware_gestures.pinch, phase),
            GestureEvent::Rotate { phase, .. } => (&mut self.hardware_gestures.rotate, phase),
            GestureEvent::LongPress { .. } | GestureEvent::DoubleTap { .. } => return Some(GestureIntent { hit: hit_test()?, event }),
        };
        let previous = capture.take();
        let hit = if phase == GesturePhase::Started { hit_test() } else { previous }?;
        let modal = tree.front_modal();
        guarded_node(tree, hit, modal)?;
        if matches!(phase, GesturePhase::Started | GesturePhase::Changed) {
            *capture = Some(hit);
        }
        finite(position).then_some(GestureIntent { hit, event })
    }

    pub fn gesture_deadline(&self) -> Option<Duration> {
        match self.contact.as_ref()?.gesture? {
            ContactGesture::Pending(deadline) => Some(deadline),
            ContactGesture::Holding => None,
        }
    }

    /// Recognize a hold when the cycle reaches its virtual deadline.
    pub fn advance_gestures(&mut self, tree: &Tree, now: Duration) -> Option<GestureIntent> {
        self.gesture_deadline().filter(|&deadline| deadline <= now)?;
        let contact = self.contact.as_mut()?;
        self.last_tap = None;
        if gesture_handlers(tree, contact.target.hit).next().is_none() {
            contact.gesture = None;
            return None;
        }
        contact.gesture = Some(ContactGesture::Holding);
        Some(contact.long_press(GesturePhase::Started, contact.at))
    }

    pub(super) fn move_contact_gesture(&mut self, decision: &mut InteractionDecision, contact: &mut Contact, previous: Point) {
        let Some(gesture) = contact.gesture else { return };
        if !contact.within_hold_slop(contact.at) {
            contact.cancel_gesture(decision);
            self.last_tap = None;
        } else if matches!(gesture, ContactGesture::Holding) && contact.at != previous {
            decision.intents.push(InteractionIntent::Gesture(contact.long_press(GesturePhase::Changed, contact.at)));
        }
    }

    pub(super) fn finish_contact_gesture(
        &mut self,
        tree: &Tree,
        decision: &mut InteractionDecision,
        contact: &Contact,
        release: Option<ContactTarget>,
        position: Point,
        now: Duration,
    ) {
        let hit = contact.target.hit;
        let intent = match contact.gesture {
            Some(ContactGesture::Holding) => {
                let phase = if contact.within_hold_slop(position) { GesturePhase::Ended } else { GesturePhase::Cancelled };
                contact.long_press(phase, position)
            }
            Some(ContactGesture::Pending(deadline))
                if now < deadline
                    && matches!(contact.mode, ContactMode::Pressed)
                    && release.is_some_and(|release| release.hit == hit)
                    && gesture_handlers(tree, hit).next().is_some() =>
            {
                if self.tuning.record_press(&mut self.last_tap, hit, contact.kind, position, now) != 2 {
                    return;
                }
                GestureIntent { hit, event: GestureEvent::DoubleTap { position, pointer: contact.kind } }
            }
            _ => {
                self.last_tap = None;
                return;
            }
        };
        decision.intents.push(InteractionIntent::Gesture(intent));
    }
}

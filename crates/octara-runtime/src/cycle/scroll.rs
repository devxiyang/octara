use super::{AppRuntime, FeedbackSignal, HandlerEvent, Routed, WindowRuntime};
use crate::ReactiveWorld;
use octara_model::geometry::Vec2;
use octara_model::identity::WindowId;
use octara_model::input::{GesturePhase, OverscrollEvent};
use octara_model::motion::{PropClass, Spring, Track};
use octara_tree::arbiter::{InteractionPayload, TouchScrollIntent, TouchScrollPhase, TouchScrollStage};
use octara_tree::tables::HandlerKind;
use octara_tree::{NodeFlags, NodeId, NodeRef, RuntimeOp};
use std::collections::VecDeque;
use std::time::Duration;

const RESISTANCE: f32 = 0.5;
const KEYBOARD_DISMISS_DISTANCE: f32 = 24.0;
const STEP: Duration = Duration::from_millis(16);

#[derive(Default)]
pub(crate) struct ScrollMotion {
    keyboard_drag: f32,
    spring: Option<Rebound>,
}

struct Rebound {
    node: NodeId,
    track: Track,
    next: Duration,
}

impl ScrollMotion {
    pub(crate) fn deadline(&self) -> Option<Duration> {
        self.spring.as_ref().map(|spring| spring.next)
    }
}

impl AppRuntime {
    pub(super) fn route_touch_scroll(&mut self, id: WindowId, scroll: TouchScrollIntent, routed: &mut VecDeque<Routed>) -> bool {
        let Some(node) = scroll.target.node() else { return false };
        let Some(window) = self.windows.get_mut(&id).filter(|window| window.tree.contains_target(scroll.target)) else { return false };
        match scroll.phase {
            TouchScrollPhase::End | TouchScrollPhase::Cancel => return self.finish_overscroll(id, node, scroll.phase),
            TouchScrollPhase::Start => {
                window.scroll_motion.keyboard_drag = 0.0;
                if let Some(previous) = window.scroll_motion.spring.take() {
                    update_overscroll(window, &mut self.world, previous.node, 0.0);
                }
            }
            TouchScrollPhase::Move => {}
        }

        let mut dismiss = false;
        if window.keyboard_open
            && window.tree.focus().is_some_and(|focus| window.tree.editor(focus).is_some())
            && let Some(core) = window.tree.node(node)
        {
            let drag = &mut window.scroll_motion.keyboard_drag;
            *drag = (*drag - scroll.delta.y).max(0.0);
            dismiss = core.flags.contains(NodeFlags::DISMISS_KEYBOARD_IMMEDIATELY)
                || core.flags.contains(NodeFlags::DISMISS_KEYBOARD_INTERACTIVELY) && *drag >= KEYBOARD_DISMISS_DISTANCE;
            if dismiss {
                window.keyboard_open = false;
                self.apply_operation(id, RuntimeOp::SetFocus { node: None, visible: false }, routed);
            }
        }

        let handler = scroll.handler(&self.windows[&id].tree, TouchScrollStage::Pre, scroll.delta);
        let used = handler.map_or_default(|handler| self.invoke_intent(id, handler).scroll);
        let available = TouchScrollIntent::remaining(scroll.delta, used);
        if available != Vec2::default() {
            routed.push_front(Routed::ScrollViewport(id, scroll, available));
        }
        dismiss || used != Vec2::default()
    }

    pub(super) fn scroll_viewport(&mut self, id: WindowId, scroll: TouchScrollIntent, available: Vec2) -> Vec2 {
        let Some(window) = self.windows.get(&id) else { return Vec2::default() };
        let Some(mut delta) = scroll.viewport_delta(&window.tree, available) else { return Vec2::default() };
        let Some(node) = scroll.target.node() else { return Vec2::default() };
        let offset = window.tree.scroll(node).map_or(0.0, |slot| slot.overscroll_offset);
        let mut used = 0.0;
        if offset * delta.y > 0.0 {
            used = delta.y.signum() * delta.y.abs().min(offset.abs() / RESISTANCE);
            self.change_overscroll(id, node, offset - used * RESISTANCE);
            delta.y -= used;
        }
        let scrolled = self.scroll_by(id, node, delta);
        Vec2::new(scrolled.x, scrolled.y + used)
    }

    pub(super) fn scroll_post(&mut self, id: WindowId, scroll: TouchScrollIntent, available: Vec2) -> bool {
        let Some(window) = self.windows.get(&id) else { return false };
        let handler = scroll.handler(&window.tree, TouchScrollStage::Post, available);
        let outcome = handler.map_or_default(|handler| self.invoke_intent(id, handler));
        let remaining = TouchScrollIntent::remaining(available, outcome.scroll);
        let window = &self.windows[&id];
        let Some((node, slot)) = scroll.target.node().and_then(|node| Some((node, window.tree.scroll(node)?))) else {
            return outcome.consumed;
        };
        let Some(delta) = scroll.viewport_delta(&window.tree, remaining) else { return outcome.consumed };
        let changed = delta.y.is_finite() && self.change_overscroll(id, node, slot.overscroll_offset - delta.y * RESISTANCE);
        outcome.consumed || outcome.scroll != Vec2::default() || changed
    }

    fn change_overscroll(&mut self, id: WindowId, node: NodeId, offset: f32) -> bool {
        let window = self.windows.get_mut(&id).expect("scroll window");
        let Some(slot) = window.tree.scroll(node) else { return false };
        let previous = slot.overscroll_offset;
        if offset == previous || !offset.is_finite() {
            return false;
        }
        update_overscroll(window, &mut self.world, node, offset);
        let phase = if previous == 0.0 { GesturePhase::Started } else { GesturePhase::Changed };
        self.notify_overscroll(id, node, OverscrollEvent { phase, offset });
        true
    }

    fn finish_overscroll(&mut self, id: WindowId, node: NodeId, phase: TouchScrollPhase) -> bool {
        let now = self.now();
        let reduced_motion = self.prefers_reduced_motion();
        let window = self.windows.get_mut(&id).expect("scroll window");
        let offset = window.tree.scroll(node).map_or(0.0, |slot| slot.overscroll_offset);
        if offset == 0.0 {
            return false;
        }
        let phase = if phase == TouchScrollPhase::End { GesturePhase::Ended } else { GesturePhase::Cancelled };
        if reduced_motion {
            update_overscroll(window, &mut self.world, node, 0.0);
        } else {
            let mut track = Track::new(f64::from(offset));
            track.retarget(0.0, now.as_secs_f64(), Spring::SETTLE.into(), PropClass::Spatial);
            window.scroll_motion.spring = Some(Rebound { node, track, next: now + STEP });
        }
        self.notify_overscroll(id, node, OverscrollEvent { phase, offset });
        true
    }

    fn notify_overscroll(&mut self, id: WindowId, node: NodeId, overscroll: OverscrollEvent) {
        let tree = &self.windows[&id].tree;
        if !tree.is_interactive(node) {
            return;
        }
        let kind = HandlerKind::Overscroll;
        self.invoke_node(
            id,
            node,
            kind,
            HandlerEvent::Interaction {
                target: tree.guard(node),
                kind,
                payload: InteractionPayload::Overscroll(overscroll),
                local_pos: None,
            },
        );
    }

    pub(super) fn advance_overscrolls(&mut self, now: Duration) {
        let reduced_motion = self.prefers_reduced_motion();
        for window in self.windows.values_mut() {
            if let Some(spring) = window.scroll_motion.spring.as_mut()
                && (reduced_motion || now >= spring.next)
            {
                let node = spring.node;
                let offset =
                    if reduced_motion || window.tree.scroll(node).is_none() { 0.0 } else { spring.track.value(now.as_secs_f64()) as f32 };
                spring.next = now + STEP;
                if offset == 0.0 {
                    window.scroll_motion.spring = None;
                }
                update_overscroll(window, &mut self.world, node, offset);
            }
        }
    }
}

fn update_overscroll(window: &mut WindowRuntime, world: &mut ReactiveWorld, node: NodeId, offset: f32) {
    window.apply(RuntimeOp::SetOverscrollOffset { node, offset });
    if !window.tree.is_live(node) {
        return;
    }
    for binding in &window.feedback {
        if let FeedbackSignal::Overscroll(signal) = binding.signal
            && matches!(binding.node, NodeRef::Existing { id, .. } if id == node)
            && world.is_scope_alive(binding.scope)
        {
            world.write(binding.scope, Some(window.id)).set_if_changed(signal, offset);
        }
    }
}

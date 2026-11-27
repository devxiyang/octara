use octara::runtime::WindowId;
use octara_model::geometry::{Point, Vec2};
use octara_model::input::{InteractionEvent, Modifiers, PointerId, WheelPhase};

#[derive(Default)]
pub(super) struct FrameInput {
    pub window: Option<WindowId>,
    pub packets: Vec<InteractionEvent>,
}

impl FrameInput {
    pub fn push(&mut self, event: InteractionEvent, merge_wheel: bool) {
        // A cursor crossing divides wheel input because it may change the scroll target.
        let last = self.packets.last_mut();
        match (last, event) {
            (Some(previous @ InteractionEvent::PointerMove { .. }), next @ InteractionEvent::PointerMove { .. }) => *previous = next,
            (
                Some(InteractionEvent::Wheel { pos, delta, mods, phase }),
                InteractionEvent::Wheel { pos: next_pos, delta: next, mods: next_mods, phase: next_phase },
            ) if merge_wheel
                && (*pos, *mods, *phase) == (next_pos, next_mods, next_phase)
                && delta.x * next.x >= 0.0
                && delta.y * next.y >= 0.0 =>
            {
                *delta = Vec2::new(delta.x + next.x, delta.y + next.y);
            }
            (_, event) => self.packets.push(event),
        }
    }

    pub fn latest_cursor(&self) -> Option<(Point, Modifiers)> {
        self.packets.iter().rev().find_map(|event| match *event {
            InteractionEvent::PointerMove { pos, mods, .. } => Some((pos, mods)),
            _ => None,
        })
    }

    pub fn wheel_phase(&self) -> Option<WheelPhase> {
        self.packets.iter().rev().find_map(|event| match *event {
            InteractionEvent::Wheel { phase, .. } => Some(phase),
            _ => None,
        })
    }
}

pub(super) fn cursor(pos: Point, mods: Modifiers) -> InteractionEvent {
    InteractionEvent::PointerMove { id: PointerId(0), pos, mods }
}

#[cfg(test)]
mod tests {
    use super::*;
    use octara_model::input::WheelStage;

    #[test]
    fn combines_wheel_bursts_without_crossing_coordinates_or_reversals() {
        let a = Point::new(10.0, 10.0);
        let b = Point::new(10.0, 21.0);
        let mut input = FrameInput::default();
        let wheel = |pos, y| InteractionEvent::Wheel {
            pos,
            delta: Vec2::new(0.0, y),
            mods: Modifiers::empty(),
            phase: WheelPhase::Direct(WheelStage::Move),
        };
        input.push(cursor(a, Modifiers::empty()), true);
        input.push(wheel(a, 3.0), true);
        input.push(wheel(a, 7.0), true);
        input.push(cursor(b, Modifiers::empty()), true);
        input.push(wheel(b, 4.0), true);
        input.push(wheel(b, -5.0), true);
        assert_eq!(
            input.packets,
            [cursor(a, Modifiers::empty()), wheel(a, 10.0), cursor(b, Modifiers::empty()), wheel(b, 4.0), wheel(b, -5.0)]
        );
    }

    #[test]
    fn keeps_the_new_modifiers_when_the_cursor_does_not_move() {
        let at = Point::new(12.0, 8.0);
        let mut input = FrameInput::default();
        input.push(cursor(at, Modifiers::empty()), false);
        input.push(cursor(at, Modifiers::SHIFT), false);
        assert_eq!(input.packets, [cursor(at, Modifiers::SHIFT)]);
        assert_eq!(input.latest_cursor(), Some((at, Modifiers::SHIFT)));
    }

    #[test]
    fn leaves_separate_wheel_samples_for_a_moving_scroll_target() {
        let mut input = FrameInput::default();
        let wheel = InteractionEvent::Wheel {
            pos: Point::new(30.0, 70.0),
            delta: Vec2::new(0.0, 20.0),
            mods: Modifiers::empty(),
            phase: WheelPhase::Direct(WheelStage::Move),
        };
        input.push(wheel, false);
        input.push(wheel, false);
        assert_eq!(input.packets, [wheel, wheel]);
    }
}

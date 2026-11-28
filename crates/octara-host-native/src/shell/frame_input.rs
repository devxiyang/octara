use octara::runtime::WindowId;
use octara_model::geometry::{Point, Vec2};
use octara_model::input::{InteractionEvent, Modifiers, PointerId, WheelPhase};

#[derive(Default)]
pub(super) struct FrameInput {
    pub window: Option<WindowId>,
    pub packets: Vec<InteractionEvent>,
}

impl FrameInput {
    pub fn push(&mut self, event: InteractionEvent, can_coalesce_wheel: impl FnOnce(Point) -> bool) {
        // A cursor crossing divides wheel input because it may change the scroll target.
        let last = self.packets.last_mut();
        match (last, event) {
            (Some(previous @ InteractionEvent::PointerMove { .. }), next @ InteractionEvent::PointerMove { .. }) => *previous = next,
            (
                Some(InteractionEvent::Wheel { pos, delta, mods, phase }),
                InteractionEvent::Wheel { pos: next_pos, delta: next, mods: next_mods, phase: next_phase },
            ) if (*pos, *mods, *phase) == (next_pos, next_mods, next_phase)
                && delta.x * next.x >= 0.0
                && delta.y * next.y >= 0.0
                && can_coalesce_wheel(next_pos) =>
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
    use octara::{Input, prelude::*};
    use octara_model::input::WheelStage;
    use octara_pipeline::NativeEngine;
    use std::time::Duration;

    fn wheel(pos: Point, delta_y: f32) -> InteractionEvent {
        InteractionEvent::Wheel {
            pos,
            delta: Vec2::new(0.0, delta_y),
            mods: Modifiers::empty(),
            phase: WheelPhase::Direct(WheelStage::Move),
        }
    }

    #[test]
    fn combines_wheel_bursts_without_crossing_coordinates_or_reversals() {
        let a = Point::new(10.0, 10.0);
        let b = Point::new(10.0, 21.0);
        let mut input = FrameInput::default();
        input.push(cursor(a, Modifiers::empty()), |_| true);
        input.push(wheel(a, 3.0), |_| true);
        input.push(wheel(a, 7.0), |_| true);
        input.push(cursor(b, Modifiers::empty()), |_| true);
        input.push(wheel(b, 4.0), |_| true);
        input.push(wheel(b, -5.0), |_| true);
        assert_eq!(
            input.packets,
            [cursor(a, Modifiers::empty()), wheel(a, 10.0), cursor(b, Modifiers::empty()), wheel(b, 4.0), wheel(b, -5.0)]
        );
    }

    #[test]
    fn keeps_the_new_modifiers_when_the_cursor_does_not_move() {
        let at = Point::new(12.0, 8.0);
        let mut input = FrameInput::default();
        input.push(cursor(at, Modifiers::empty()), |_| false);
        input.push(cursor(at, Modifiers::SHIFT), |_| false);
        assert_eq!(input.packets, [cursor(at, Modifiers::SHIFT)]);
        assert_eq!(input.latest_cursor(), Some((at, Modifiers::SHIFT)));
    }

    #[test]
    fn leaves_separate_wheel_samples_for_a_moving_scroll_target() {
        let mut input = FrameInput::default();
        let wheel = wheel(Point::new(30.0, 70.0), 20.0);
        input.push(wheel, |_| false);
        input.push(wheel, |_| false);
        assert_eq!(input.packets, [wheel, wheel]);
    }

    #[test]
    fn coalesces_120_hz_wheel_input_for_a_virtual_list_at_60_hz() {
        let mut app = App::new(|_, _| Box::new(NativeEngine::new(Default::default())));
        let rows = app.build(|b| b.list::<usize, ()>()).unwrap();
        app.update(|w| {
            for key in 0..80 {
                w.push_row(rows, key, ());
            }
        })
        .unwrap();
        let id = app
            .open_window(WindowOptions::default(), move |_| {
                col().test_id("port").w(120).h(70).scroll_y().child(virtual_each(
                    rows,
                    VirtualOptions { estimated_row_height: 20.0, maintain_scroll_anchor: true, ..Default::default() },
                    |_, key, _| el().h(20).test_id(format!("row-{key}")),
                ))
            })
            .unwrap();
        let pos = Point::new(30.0, 20.0);
        let mut input = FrameInput::default();
        let mut samples = 0;
        for frame in 1..=60 {
            let tick = Duration::from_nanos(frame * 1_000_000_000 / 60);
            let runtime = app.runtime().window(id).unwrap();
            while Duration::from_nanos(samples * 1_000_000_000 / 120) < tick {
                input.push(wheel(pos, 5.0), |pos| runtime.can_coalesce_wheel(pos));
                samples += 1;
            }
            assert_eq!(input.packets, [wheel(pos, 10.0)], "frame {frame}");
            let receipt = app.runtime_mut().dispatch_frame_inputs(id, input.packets.drain(..).map(Input::Interaction), Some(tick)).unwrap();
            assert!(receipt.handled);
            let runtime = app.runtime().window(id).unwrap();
            let port = runtime.tree().test_ids("port")[0];
            assert_eq!(runtime.tree().scroll(port).unwrap().offset.y, (frame * 10) as f32);
        }
        assert_eq!(samples, 120);
        let runtime = app.runtime().window(id).unwrap();
        assert_eq!(runtime.hit_test(Point::new(10.0, 1.0)).unwrap().hit.node(), Some(runtime.tree().test_ids("row-30")[0]));
    }
}

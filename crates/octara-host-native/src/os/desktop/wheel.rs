use octara_model::geometry::{Point, Vec2};
use octara_model::input::{InteractionEvent, Modifiers, WheelPhase, WheelStage};
use winit::event::{MouseScrollDelta, TouchPhase};

pub(super) fn wheel_input(pos: Point, mods: Modifiers, scale: f64, delta: MouseScrollDelta, phase: TouchPhase) -> InteractionEvent {
    let phase = wheel_phase(delta, phase);
    let delta = match delta {
        MouseScrollDelta::LineDelta(x, y) => Vec2::new(-x * 32.0, -y * 32.0),
        MouseScrollDelta::PixelDelta(point) => {
            let point = point.to_logical::<f32>(scale);
            Vec2::new(-point.x, -point.y)
        }
    };
    InteractionEvent::Wheel { pos, delta, phase, mods }
}

fn wheel_phase(delta: MouseScrollDelta, phase: TouchPhase) -> WheelPhase {
    WheelPhase::Direct(match phase {
        TouchPhase::Started => WheelStage::Start,
        TouchPhase::Moved if matches!(delta, MouseScrollDelta::LineDelta(..)) => return WheelPhase::Discrete,
        TouchPhase::Moved => WheelStage::Move,
        TouchPhase::Ended => WheelStage::End,
        TouchPhase::Cancelled => WheelStage::Cancel,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_winit_touch_stages_and_discrete_line_wheels() {
        let precise = MouseScrollDelta::PixelDelta(winit::dpi::PhysicalPosition::new(0.0, 3.0));
        assert_eq!(wheel_phase(precise, TouchPhase::Started), WheelPhase::Direct(WheelStage::Start));
        assert_eq!(wheel_phase(precise, TouchPhase::Moved), WheelPhase::Direct(WheelStage::Move));
        assert_eq!(wheel_phase(precise, TouchPhase::Ended), WheelPhase::Direct(WheelStage::End));
        assert_eq!(wheel_phase(precise, TouchPhase::Cancelled), WheelPhase::Direct(WheelStage::Cancel));
        assert_eq!(wheel_phase(MouseScrollDelta::LineDelta(0.0, 1.0), TouchPhase::Moved), WheelPhase::Discrete);
    }
}

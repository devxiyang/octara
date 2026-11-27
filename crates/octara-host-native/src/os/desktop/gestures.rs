use octara_model::geometry::Point;
use octara_model::input::{GestureEvent, GesturePhase, Modifiers};
use winit::event::{TouchPhase, WindowEvent};

/// Convert winit's relative magnification and degree deltas to Octara gesture values.
#[derive(Default)]
pub(super) struct TrackpadGestures {
    pinch: Option<(f32, Point)>,
    rotation: Option<(f32, Point)>,
}

impl TrackpadGestures {
    pub(super) fn update(&mut self, event: &WindowEvent, anchor: Option<Point>, modifiers: Modifiers) -> Option<GestureEvent> {
        match *event {
            WindowEvent::PinchGesture { delta, phase, .. } => {
                let factor = 1.0 + delta as f32;
                let (phase, previous, scale, anchor) = accumulate(&mut self.pinch, phase, anchor, 1.0, |scale| {
                    let next = scale * factor;
                    (next.is_finite() && next > 0.0).then_some(next)
                })?;
                Some(GestureEvent::Pinch { phase, scale, delta_scale: scale / previous, anchor, modifiers })
            }
            WindowEvent::RotationGesture { delta, phase, .. } => {
                let (phase, previous, radians, anchor) = accumulate(&mut self.rotation, phase, anchor, 0.0, |radians| {
                    let next = radians + delta.to_radians();
                    next.is_finite().then_some(next)
                })?;
                Some(GestureEvent::Rotate { phase, radians, delta_radians: radians - previous, anchor, modifiers })
            }
            _ => None,
        }
    }
}

fn accumulate(
    state: &mut Option<(f32, Point)>,
    phase: TouchPhase,
    anchor: Option<Point>,
    identity: f32,
    update: impl FnOnce(f32) -> Option<f32>,
) -> Option<(GesturePhase, f32, f32, Point)> {
    let phase = match phase {
        TouchPhase::Started => GesturePhase::Started,
        TouchPhase::Moved => GesturePhase::Changed,
        TouchPhase::Ended => GesturePhase::Ended,
        TouchPhase::Cancelled => GesturePhase::Cancelled,
    };
    let (previous, anchor) = if phase == GesturePhase::Started {
        *state = None;
        (identity, anchor?)
    } else {
        let (previous, last_anchor) = (*state)?;
        (previous, anchor.unwrap_or(last_anchor))
    };
    let value = if phase == GesturePhase::Cancelled { previous } else { update(previous).unwrap_or(previous) };
    *state = matches!(phase, GesturePhase::Started | GesturePhase::Changed).then_some((value, anchor));
    Some((phase, previous, value, anchor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::event::DeviceId;

    #[test]
    fn pinch_accumulates_factors_and_cancels_at_last_anchor() {
        let mut state = TrackpadGestures::default();
        let anchor = Point::new(20.0, 30.0);
        let modifiers = Modifiers::SHIFT;
        let pinch = |phase, delta| WindowEvent::PinchGesture { device_id: DeviceId::dummy(), delta, phase };
        assert_eq!(
            state.update(&pinch(TouchPhase::Started, 0.5), Some(anchor), modifiers),
            Some(GestureEvent::Pinch { phase: GesturePhase::Started, scale: 1.5, delta_scale: 1.5, anchor, modifiers }),
        );
        assert_eq!(
            state.update(&pinch(TouchPhase::Moved, -0.5), Some(anchor), modifiers),
            Some(GestureEvent::Pinch { phase: GesturePhase::Changed, scale: 0.75, delta_scale: 0.5, anchor, modifiers }),
        );
        assert_eq!(
            state.update(&pinch(TouchPhase::Cancelled, f64::NAN), None, modifiers),
            Some(GestureEvent::Pinch { phase: GesturePhase::Cancelled, scale: 0.75, delta_scale: 1.0, anchor, modifiers }),
        );
        assert_eq!(state.update(&pinch(TouchPhase::Moved, 0.1), Some(anchor), modifiers), None);
    }

    #[test]
    fn rotation_converts_degrees_and_resets_after_end() {
        let mut state = TrackpadGestures::default();
        let anchor = Point::new(20.0, 30.0);
        let modifiers = Modifiers::empty();
        let rotate = |phase, delta| WindowEvent::RotationGesture { device_id: DeviceId::dummy(), delta, phase };
        state.update(&rotate(TouchPhase::Started, 0.0), Some(anchor), modifiers);
        state.update(&rotate(TouchPhase::Moved, 90.0), Some(anchor), modifiers);
        assert_eq!(
            state.update(&rotate(TouchPhase::Ended, 90.0), None, modifiers),
            Some(GestureEvent::Rotate {
                phase: GesturePhase::Ended,
                radians: std::f32::consts::PI,
                delta_radians: std::f32::consts::FRAC_PI_2,
                anchor,
                modifiers,
            }),
        );
        assert_eq!(state.update(&rotate(TouchPhase::Moved, 0.0), Some(anchor), modifiers), None);
    }
}

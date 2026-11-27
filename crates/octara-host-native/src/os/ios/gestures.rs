use super::view::keyboard_modifiers;
use crate::os::{PlatformEvent, PlatformSender};
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_foundation::{NSArray, NSPoint};
use objc2_ui_kit::{
    UIEvent, UIGestureRecognizer, UIGestureRecognizerDelegate, UIGestureRecognizerState, UIHoverGestureRecognizer, UIPanGestureRecognizer,
    UIPinchGestureRecognizer, UIRotationGestureRecognizer, UIScrollTypeMask, UIView,
};
use octara::runtime::WindowId;
use octara_model::geometry::{Point, Vec2};
use octara_model::input::{GestureEvent, GesturePhase, InteractionEvent, Modifiers, WheelPhase, WheelStage};
use std::cell::Cell;

struct State {
    proxy: PlatformSender,
    window: WindowId,
    pinch: Cell<Option<Sample>>,
    rotation: Cell<Option<Sample>>,
    pointer: Cell<Option<Point>>,
    modifiers: Cell<Modifiers>,
}

#[derive(Clone, Copy)]
struct Sample {
    value: f32,
    anchor: Point,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraGestureTarget"]
    #[ivars = State]
    struct GestureTarget;

    unsafe impl NSObjectProtocol for GestureTarget {}

    unsafe impl UIGestureRecognizerDelegate for GestureTarget {
        #[unsafe(method(gestureRecognizer:shouldReceiveEvent:))]
        fn receive_event(&self, _: &UIGestureRecognizer, event: &UIEvent) -> bool {
            self.ivars().modifiers.set(keyboard_modifiers(event.modifierFlags()));
            true
        }

        #[unsafe(method(gestureRecognizer:shouldRecognizeSimultaneouslyWithGestureRecognizer:))]
        fn simultaneous(&self, _: &UIGestureRecognizer, other: &UIGestureRecognizer) -> bool {
            other.delegate().is_some_and(|delegate| std::ptr::eq(&*delegate, ProtocolObject::from_ref(self)))
        }
    }

    impl GestureTarget {
        #[unsafe(method(hover:))]
        fn hover(&self, recognizer: &UIHoverGestureRecognizer) {
            let state = self.ivars();
            let position = match recognizer.state() {
                UIGestureRecognizerState::Began | UIGestureRecognizerState::Changed => {
                    let point = recognizer.locationInView(recognizer.view().as_deref());
                    Some(Point::new(point.x as f32, point.y as f32))
                }
                UIGestureRecognizerState::Ended | UIGestureRecognizerState::Cancelled | UIGestureRecognizerState::Failed => None,
                _ => return,
            };
            state.pointer.set(position);
            state.proxy.send_event(position.map_or(PlatformEvent::CursorLeft(state.window), |position| PlatformEvent::CursorMoved {
                window: state.window, position, modifiers: state.modifiers.get(),
            }));
        }

        #[unsafe(method(scroll:))]
        fn scroll(&self, recognizer: &UIPanGestureRecognizer) {
            let stage = match recognizer.state() {
                UIGestureRecognizerState::Began => WheelStage::Start,
                UIGestureRecognizerState::Changed => WheelStage::Move,
                UIGestureRecognizerState::Ended => WheelStage::End,
                UIGestureRecognizerState::Cancelled | UIGestureRecognizerState::Failed => WheelStage::Cancel,
                _ => return,
            };
            let view = recognizer.view();
            let offset = recognizer.translationInView(view.as_deref());
            recognizer.setTranslation_inView(NSPoint::ZERO, view.as_deref());
            let pos = self.ivars().pointer.get().unwrap_or_else(|| {
                let point = recognizer.locationInView(view.as_deref());
                Point::new(point.x as f32, point.y as f32)
            });
            let delta = Vec2::new(-offset.x as f32, -offset.y as f32);
            let state = self.ivars();
            self.interaction(InteractionEvent::Wheel { pos, delta, phase: WheelPhase::Direct(stage), mods: state.modifiers.get() });
        }

        #[unsafe(method(pinch:))]
        fn pinch(&self, recognizer: &UIPinchGestureRecognizer) {
            let scale = recognizer.scale() as f32;
            let Some((phase, sample, previous)) = sample(&self.ivars().pinch, recognizer, 1.0, scale, scale > 0.0) else { return };
            self.send(GestureEvent::Pinch {
                phase,
                scale: sample.value,
                delta_scale: sample.value / previous,
                anchor: sample.anchor,
                modifiers: self.ivars().modifiers.get(),
            });
        }

        #[unsafe(method(rotate:))]
        fn rotate(&self, recognizer: &UIRotationGestureRecognizer) {
            // UIKit measures clockwise; Octara uses counterclockwise-positive rotation.
            let Some((phase, sample, previous)) = sample(&self.ivars().rotation, recognizer, 0.0, -recognizer.rotation() as f32, true)
            else { return };
            self.send(GestureEvent::Rotate {
                phase,
                radians: sample.value,
                delta_radians: sample.value - previous,
                anchor: sample.anchor,
                modifiers: self.ivars().modifiers.get(),
            });
        }
    }
);

impl GestureTarget {
    fn send(&self, event: GestureEvent) {
        self.interaction(InteractionEvent::Gesture(event));
    }

    fn interaction(&self, event: InteractionEvent) {
        let State { proxy, window, .. } = self.ivars();
        proxy.send_event(PlatformEvent::Interaction { window: *window, event });
    }
}

fn sample(
    state: &Cell<Option<Sample>>,
    recognizer: &UIGestureRecognizer,
    identity: f32,
    value: f32,
    valid: bool,
) -> Option<(GesturePhase, Sample, f32)> {
    let phase = match recognizer.state() {
        UIGestureRecognizerState::Began => GesturePhase::Started,
        UIGestureRecognizerState::Changed => GesturePhase::Changed,
        UIGestureRecognizerState::Ended => GesturePhase::Ended,
        UIGestureRecognizerState::Cancelled | UIGestureRecognizerState::Failed => GesturePhase::Cancelled,
        _ => return None,
    };
    let previous = state.take();
    let sample = if recognizer.numberOfTouches() >= 2 {
        let point = recognizer.locationInView(recognizer.view().as_deref());
        Point::new(point.x as f32, point.y as f32)
    } else {
        // UIKit can release the touches before delivering the terminal state.
        previous?.anchor
    };
    let previous = if phase == GesturePhase::Started { identity } else { previous?.value };
    let value = if valid && value.is_finite() && phase != GesturePhase::Cancelled { value } else { previous };
    let sample = Sample { value, anchor: sample };
    if matches!(phase, GesturePhase::Started | GesturePhase::Changed) {
        state.set(Some(sample));
    }
    Some((phase, sample, previous))
}

pub(super) struct Recognizers {
    view: Retained<UIView>,
    recognizers: [Retained<UIGestureRecognizer>; 4],
    _target: Retained<GestureTarget>,
}

impl Recognizers {
    pub(super) fn new(view: &UIView, proxy: PlatformSender, window: WindowId) -> Self {
        let mtm = MainThreadMarker::from(view);
        let state = State {
            proxy,
            window,
            pinch: Cell::default(),
            rotation: Cell::default(),
            pointer: Cell::default(),
            modifiers: Cell::default(),
        };
        // SAFETY: Initialize the NSObject superclass on the UIKit thread.
        let target: Retained<GestureTarget> = unsafe { msg_send![super(mtm.alloc::<GestureTarget>().set_ivars(state)), init] };
        // SAFETY: The target stays alive while the recognizers are installed and defines their selectors.
        let (pinch, rotation, hover, scroll) = unsafe {
            (
                UIPinchGestureRecognizer::initWithTarget_action(mtm.alloc(), Some(&target), Some(sel!(pinch:))),
                UIRotationGestureRecognizer::initWithTarget_action(mtm.alloc(), Some(&target), Some(sel!(rotate:))),
                UIHoverGestureRecognizer::initWithTarget_action(mtm.alloc(), Some(&target), Some(sel!(hover:))),
                UIPanGestureRecognizer::initWithTarget_action(mtm.alloc(), Some(&target), Some(sel!(scroll:))),
            )
        };
        scroll.setAllowedTouchTypes(&NSArray::new());
        scroll.setAllowedScrollTypesMask(UIScrollTypeMask::All);
        let recognizers =
            [Retained::into_super(pinch), Retained::into_super(rotation), Retained::into_super(hover), Retained::into_super(scroll)];
        for recognizer in &recognizers {
            recognizer.setDelegate(Some(ProtocolObject::from_ref(&*target)));
            view.addGestureRecognizer(recognizer);
        }
        Self { view: view.retain(), recognizers, _target: target }
    }
}

impl Drop for Recognizers {
    fn drop(&mut self) {
        for recognizer in &self.recognizers {
            self.view.removeGestureRecognizer(recognizer);
        }
    }
}

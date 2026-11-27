use crate::ShellEvent;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_ui_kit::{
    UIEvent, UIGestureRecognizer, UIGestureRecognizerDelegate, UIGestureRecognizerState, UIKeyModifierFlags, UIPinchGestureRecognizer,
    UIRotationGestureRecognizer, UIView,
};
use octara::runtime::{Input, WindowId};
use octara_model::geometry::Point;
use octara_model::input::{GestureEvent, GesturePhase, InteractionEvent, Modifiers};
use std::cell::Cell;
use winit::event_loop::EventLoopProxy;

struct State {
    proxy: EventLoopProxy<ShellEvent>,
    window: WindowId,
    pinch: Cell<Option<Sample>>,
    rotation: Cell<Option<Sample>>,
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
            let flags = event.modifierFlags();
            let mut modifiers = Modifiers::empty();
            modifiers.set(Modifiers::SHIFT, flags.contains(UIKeyModifierFlags::Shift));
            modifiers.set(Modifiers::CTRL, flags.contains(UIKeyModifierFlags::Control));
            modifiers.set(Modifiers::ALT, flags.contains(UIKeyModifierFlags::Alternate));
            modifiers.set(Modifiers::META, flags.contains(UIKeyModifierFlags::Command));
            self.ivars().modifiers.set(modifiers);
            true
        }

        #[unsafe(method(gestureRecognizer:shouldRecognizeSimultaneouslyWithGestureRecognizer:))]
        fn simultaneous(&self, _: &UIGestureRecognizer, other: &UIGestureRecognizer) -> bool {
            other.delegate().is_some_and(|delegate| std::ptr::eq(&*delegate, ProtocolObject::from_ref(self)))
        }
    }

    impl GestureTarget {
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
            // UIKit measures clockwise; Octara uses winit's counterclockwise convention.
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
        let State { proxy, window, .. } = self.ivars();
        let _ = proxy.send_event(ShellEvent::Input { window: *window, input: Input::Interaction(InteractionEvent::Gesture(event)) });
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
    pinch: Retained<UIPinchGestureRecognizer>,
    rotation: Retained<UIRotationGestureRecognizer>,
    _target: Retained<GestureTarget>,
}

impl Recognizers {
    pub(super) fn new(view: &UIView, proxy: EventLoopProxy<ShellEvent>, window: WindowId) -> Self {
        let mtm = MainThreadMarker::from(view);
        let state = State { proxy, window, pinch: Cell::default(), rotation: Cell::default(), modifiers: Cell::default() };
        // SAFETY: Initialize the NSObject superclass on the UIKit thread.
        let target: Retained<GestureTarget> = unsafe { msg_send![super(mtm.alloc::<GestureTarget>().set_ivars(state)), init] };
        // SAFETY: The target stays alive while the view has the recognizers and defines both selectors.
        let (pinch, rotation) = unsafe {
            (
                UIPinchGestureRecognizer::initWithTarget_action(mtm.alloc(), Some(&target), Some(sel!(pinch:))),
                UIRotationGestureRecognizer::initWithTarget_action(mtm.alloc(), Some(&target), Some(sel!(rotate:))),
            )
        };
        for recognizer in [&*pinch as &UIGestureRecognizer, &*rotation] {
            recognizer.setDelegate(Some(ProtocolObject::from_ref(&*target)));
            view.addGestureRecognizer(recognizer);
        }
        Self { view: view.retain(), pinch, rotation, _target: target }
    }
}

impl Drop for Recognizers {
    fn drop(&mut self) {
        self.view.removeGestureRecognizer(&self.pinch);
        self.view.removeGestureRecognizer(&self.rotation);
    }
}

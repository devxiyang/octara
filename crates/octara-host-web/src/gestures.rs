use crate::{Driver, modifiers};
use octara::App;
use octara::runtime::{Input, WindowId, cycle::AppError};
use octara_model::geometry::Point;
use octara_model::input::{GestureEvent, GesturePhase, InteractionEvent, Modifiers, PointerId};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Event, PointerEvent, WheelEvent};

const WHEEL_IDLE: Duration = Duration::from_millis(120);

#[derive(Default)]
pub(super) struct Gestures {
    touches: RefCell<Touches>,
    wheel: Cell<Option<WheelPinch>>,
    safari: Cell<Option<Pinch>>,
    pub(super) cursor: Cell<Option<Point>>,
}

#[derive(Clone, Copy)]
struct Pinch {
    anchor: Point,
    scale: f32,
    modifiers: Modifiers,
    claimed: bool,
}

impl Pinch {
    fn event(self, phase: GesturePhase, delta_scale: f32) -> GestureEvent {
        GestureEvent::Pinch { phase, scale: self.scale, delta_scale, anchor: self.anchor, modifiers: self.modifiers }
    }
}

#[derive(Clone, Copy)]
struct WheelPinch {
    pinch: Pinch,
    deadline: Duration,
}

#[derive(Default)]
struct Touches {
    contacts: [Option<(PointerId, Point)>; 2],
    pinch: Option<TouchPinch>,
    claimed: bool,
}

#[derive(Clone, Copy)]
struct TouchPinch {
    initial_distance: f32,
    value: Pinch,
}

impl Driver {
    pub(super) fn connect_gestures(self: &Rc<Self>) -> Result<(), JsValue> {
        self.listen(self.mount.as_ref(), "wheel", false, |driver, event| driver.pinch_wheel(&event))?;
        for name in ["touchstart", "touchmove"] {
            self.listen(self.mount.as_ref(), name, false, |driver, event| {
                // Cancel native zoom only after an Octara gesture handler claims the touch pair.
                if driver.gestures.touches.borrow().claimed {
                    event.prevent_default();
                }
            })?;
        }
        for (name, phase) in
            [("gesturestart", GesturePhase::Started), ("gesturechange", GesturePhase::Changed), ("gestureend", GesturePhase::Ended)]
        {
            self.listen(self.mount.as_ref(), name, false, move |driver, event| driver.pinch_safari(&event, phase))?;
        }
        Ok(())
    }

    pub(super) fn window_point(&self, x: f32, y: f32) -> Point {
        let rect = self.mount.get_bounding_client_rect();
        Point::new(x - rect.x() as f32, y - rect.y() as f32)
    }

    pub(super) fn pinch_pointer(&self, name: &str, id: PointerId, pointer: &PointerEvent) -> bool {
        // A DOM event fired during dispatch cannot start another app cycle.
        if self.app.try_borrow_mut().is_err() {
            return true;
        }
        let modifiers = modifiers(pointer.shift_key(), pointer.ctrl_key(), pointer.alt_key(), pointer.meta_key());
        let position = Point::new(pointer.client_x() as f32, pointer.client_y() as f32);
        let (was_claimed, cancel, gesture) = {
            let mut touches = self.gestures.touches.borrow_mut();
            let was_claimed = touches.claimed;
            let was_pair = touches.contacts.iter().all(Option::is_some);
            let gesture = touches.update(name, id, position, modifiers, |point| self.window_point(point.x, point.y));
            let cancel = if !was_pair && touches.contacts.iter().all(Option::is_some) {
                self.pointer.get().filter(|id| touches.contacts.iter().flatten().any(|(contact, _)| contact == id))
            } else {
                None
            };
            (was_claimed, cancel, gesture)
        };
        if cancel.is_none() && gesture.is_none() {
            return was_claimed;
        }
        let result = self.cycle(|app, window| {
            if let Some(id) = cancel
                && self.pointer.get() == Some(id)
            {
                self.pointer.set(None);
                app.dispatch(window, Input::Interaction(InteractionEvent::PointerCancel { id }))?;
            }
            gesture.map_or(Ok(false), |gesture| dispatch(app, window, gesture))
        });
        let handled = result.unwrap_or_else(|error| {
            self.report_error(error);
            false
        });
        let mut touches = self.gestures.touches.borrow_mut();
        if handled && touches.contacts.iter().any(Option::is_some) {
            touches.claimed = true;
        }
        handled || was_claimed
    }

    fn pinch_wheel(&self, event: &Event) {
        let Some(wheel) = event.dyn_ref::<WheelEvent>().filter(|event| event.ctrl_key()) else { return };
        let units = match wheel.delta_mode() {
            WheelEvent::DOM_DELTA_LINE => 16.0,
            WheelEvent::DOM_DELTA_PAGE => f64::from(self.mount.client_height()),
            _ => 1.0,
        };
        let delta = wheel.delta_y() * units;
        if !delta.is_finite() || delta == 0.0 {
            return;
        }
        let factor = (-0.002 * delta).clamp(-1.0, 1.0).exp() as f32;
        let result = self.cycle(|app, window| {
            let anchor = self.window_point(wheel.client_x() as f32, wheel.client_y() as f32);
            let modifiers = modifiers(wheel.shift_key(), wheel.ctrl_key(), wheel.alt_key(), wheel.meta_key());
            let mut pinch = if let Some(active) = self.gestures.wheel.get() {
                Pinch { anchor, modifiers, ..active.pinch }
            } else {
                let mut pinch = Pinch { anchor, scale: 1.0, modifiers, claimed: false };
                pinch.claimed = dispatch(app, window, pinch.event(GesturePhase::Started, 1.0))?;
                pinch
            };
            let scale = pinch.scale * factor;
            if scale.is_finite() && scale > 0.0 {
                let delta_scale = scale / pinch.scale;
                pinch.scale = scale;
                pinch.claimed |= dispatch(app, window, pinch.event(GesturePhase::Changed, delta_scale))?;
            }
            self.gestures.wheel.set(Some(WheelPinch { pinch, deadline: self.elapsed() + WHEEL_IDLE }));
            Ok(pinch.claimed)
        });
        self.prevent_handled(event, result);
    }

    fn pinch_safari(&self, event: &Event, phase: GesturePhase) {
        // Safari reports trackpad pinch via GestureEvent. Touches already have Pointer Events.
        if self.gestures.touches.borrow().contacts.iter().any(Option::is_some) {
            if self.gestures.touches.borrow().claimed {
                event.prevent_default();
            }
            return;
        }
        let property = |name: &str| js_sys::Reflect::get(event.as_ref(), &JsValue::from_str(name)).ok();
        let number = |name| property(name).and_then(|value| value.as_f64()).map(|value| value as f32);
        let flag = |name| property(name).and_then(|value| value.as_bool()).unwrap_or(false);
        let Some(scale) = number("scale").filter(|scale| scale.is_finite() && *scale > 0.0) else { return };
        let position = number("clientX").zip(number("clientY")).filter(|(x, y)| x.is_finite() && y.is_finite());
        let anchor = position.map(|(x, y)| self.window_point(x, y)).or(self.gestures.cursor.get());
        let Some(anchor) = anchor.or_else(|| self.gestures.safari.get().map(|pinch| pinch.anchor)) else { return };
        let modifiers = modifiers(flag("shiftKey"), flag("ctrlKey"), flag("altKey"), flag("metaKey"));
        let previous = match phase {
            GesturePhase::Started => Pinch { anchor, scale: 1.0, modifiers, claimed: false },
            _ => {
                let Some(previous) = self.gestures.safari.take() else { return };
                previous
            }
        };
        let result = self.cycle(|app, window| {
            let mut current = Pinch { anchor, scale, modifiers, claimed: previous.claimed };
            current.claimed |= dispatch(app, window, current.event(phase, scale / previous.scale))?;
            if phase != GesturePhase::Ended {
                self.gestures.safari.set(Some(current));
            }
            Ok(current.claimed)
        });
        self.prevent_handled(event, result);
    }

    fn prevent_handled(&self, event: &Event, result: Result<bool, AppError>) {
        match result {
            Ok(true) => event.prevent_default(),
            Ok(false) => {}
            Err(error) => self.report_error(error),
        }
    }

    pub(super) fn wheel_pinch_deadline(&self) -> Option<Duration> {
        self.gestures.wheel.get().map(|wheel| wheel.deadline)
    }

    pub(super) fn finish_wheel_pinch(&self, app: &mut App, window: WindowId) -> Result<(), AppError> {
        if let Some(wheel) = self.gestures.wheel.get().filter(|wheel| wheel.deadline <= self.elapsed()) {
            self.gestures.wheel.set(None);
            dispatch(app, window, wheel.pinch.event(GesturePhase::Ended, 1.0))?;
        }
        Ok(())
    }

    pub(super) fn cancel_gestures(&self) {
        self.gestures.cursor.set(None);
        let touches = self.gestures.touches.take();
        let wheel = self.gestures.wheel.take().map(|wheel| wheel.pinch);
        let safari = self.gestures.safari.take();
        let mut pinches = [touches.pinch.map(|pinch| pinch.value), wheel, safari].into_iter().flatten().peekable();
        if pinches.peek().is_some()
            && let Err(error) = self.cycle(|app, window| {
                for pinch in pinches {
                    dispatch(app, window, pinch.event(GesturePhase::Cancelled, 1.0))?;
                }
                Ok(())
            })
        {
            self.report_error(error);
        }
    }
}

impl Touches {
    fn update(
        &mut self,
        name: &str,
        id: PointerId,
        position: Point,
        modifiers: Modifiers,
        to_window: impl FnOnce(Point) -> Point,
    ) -> Option<GestureEvent> {
        let known = self.contacts.iter().position(|contact| contact.is_some_and(|(contact, _)| contact == id));
        match name {
            "pointerdown" => {
                if known.is_some() {
                    return None;
                }
                let slot = self.contacts.iter_mut().find(|contact| contact.is_none())?;
                *slot = Some((id, position));
            }
            "pointermove" => self.contacts[known?] = Some((id, position)),
            "pointerup" | "pointercancel" => {
                self.contacts[known?] = None;
                if self.contacts.iter().all(Option::is_none) {
                    self.claimed = false;
                }
                let mut pinch = self.pinch.take()?.value;
                pinch.modifiers = modifiers;
                let phase = if name == "pointerup" { GesturePhase::Ended } else { GesturePhase::Cancelled };
                return Some(pinch.event(phase, 1.0));
            }
            _ => return None,
        }
        let [Some((_, a)), Some((_, b))] = self.contacts else { return None };
        let distance = (a.x - b.x).hypot(a.y - b.y);
        let anchor = to_window(Point::new((a.x + b.x) * 0.5, (a.y + b.y) * 0.5));
        if let Some(pinch) = &mut self.pinch {
            let scale = (distance / pinch.initial_distance).max(f32::EPSILON);
            let delta_scale = scale / pinch.value.scale;
            pinch.value = Pinch { anchor, scale, modifiers, claimed: self.claimed };
            Some(pinch.value.event(GesturePhase::Changed, delta_scale))
        } else if distance >= 1.0 {
            let value = Pinch { anchor, scale: 1.0, modifiers, claimed: self.claimed };
            self.pinch = Some(TouchPinch { initial_distance: distance, value });
            Some(value.event(GesturePhase::Started, 1.0))
        } else {
            None
        }
    }
}

fn dispatch(app: &mut App, window: WindowId, event: GestureEvent) -> Result<bool, AppError> {
    app.dispatch(window, Input::Interaction(InteractionEvent::Gesture(event))).map(|receipt| receipt.handled)
}

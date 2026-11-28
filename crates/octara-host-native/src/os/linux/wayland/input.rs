use super::{NativeEvent, Protocol, Shared, data::DataSeat, ime::Ime, keyboard::Keyboard};
use crate::os::{PlatformEvent, PointerPhase};
use octara::runtime::WindowId;
use octara_model::geometry::{Point, Vec2};
use octara_model::input::{
    Button, CursorStyle, GestureEvent, GesturePhase, InteractionEvent, Modifiers, PointerId, PointerKind, WheelPhase, WheelStage,
};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;
use wayland_client::backend::ObjectId;
use wayland_client::protocol::{wl_pointer, wl_seat, wl_surface, wl_touch};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_cursor::CursorTheme;
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::{self, Shape, WpCursorShapeDeviceV1};
use wayland_protocols::wp::pointer_gestures::zv1::client::zwp_pointer_gesture_pinch_v1::{self, ZwpPointerGesturePinchV1};

pub(super) struct Seat {
    pub native: wl_seat::WlSeat,
    pub keyboard: Keyboard,
    pub ime: Ime,
    pub data: DataSeat,
    pub user_serial: Option<UserSerial>,
    pub clipboard_serial: Option<u32>,
    pointer: Option<Pointer>,
    touch: Option<wl_touch::WlTouch>,
    contacts: HashMap<i32, Contact>,
}

pub(super) struct UserSerial {
    pub surface: wl_surface::WlSurface,
    pub serial: u32,
    pub time: Instant,
}

/// The implicit grab observed when a native input event was queued for the shell.
pub(super) struct GrabOrigin {
    seat: ObjectId,
    contact: GrabContact,
    serial: u32,
}

#[derive(Clone, Copy)]
enum GrabContact {
    Pointer,
    Touch(i32),
}

struct Pointer {
    object: wl_pointer::WlPointer,
    focus: Option<PointerFocus>,
    grab: Option<u32>,
    cursor_shape: Option<WpCursorShapeDeviceV1>,
    cursor_surface: Option<wl_surface::WlSurface>,
    pinch: Option<ZwpPointerGesturePinchV1>,
    gesture: Option<Pinch>,
    scroll: Scroll,
}

struct PointerFocus {
    surface: wl_surface::WlSurface,
    window: WindowId,
    position: Point,
    enter_serial: u32,
}

struct Contact {
    window: WindowId,
    id: PointerId,
    position: Point,
    serial: Option<u32>,
}

struct Pinch {
    window: WindowId,
    anchor: Point,
    scale: f32,
    rotation: f32,
}

impl Pinch {
    fn update(&mut self, scale: f32, clockwise_degrees: f32) -> Option<(f32, f32)> {
        let delta_scale = scale / self.scale;
        let delta_radians = -clockwise_degrees.to_radians();
        let rotation = self.rotation + delta_radians;
        if delta_scale.is_finite() && delta_scale > 0.0 && rotation.is_finite() {
            self.scale = scale;
            self.rotation = rotation;
            Some((delta_scale, delta_radians))
        } else {
            None
        }
    }

    fn emit(&self, shared: &Shared, phase: GesturePhase, delta_scale: f32, delta_radians: f32, modifiers: Modifiers) {
        for gesture in [
            GestureEvent::Pinch { phase, scale: self.scale, delta_scale, anchor: self.anchor, modifiers },
            GestureEvent::Rotate { phase, radians: self.rotation, delta_radians, anchor: self.anchor, modifiers },
        ] {
            shared.emit(PlatformEvent::Interaction { window: self.window, event: InteractionEvent::Gesture(gesture) });
        }
    }
}

#[derive(Default)]
struct Scroll {
    delta: [f32; 2],
    wheel_ticks: [Option<f32>; 2],
    active: [bool; 2],
    stopped: [bool; 2],
    source: Option<wl_pointer::AxisSource>,
}

impl Scroll {
    fn emit(&self, shared: &Shared, focus: &PointerFocus, mods: Modifiers, delta: Vec2, phase: WheelPhase) {
        shared.emit(PlatformEvent::Interaction {
            window: focus.window,
            event: InteractionEvent::Wheel { pos: focus.position, delta, phase, mods },
        });
    }

    fn flush(&mut self, shared: &Shared, focus: &PointerFocus, mods: Modifiers) {
        let direct = self.source == Some(wl_pointer::AxisSource::Finger);
        let before = self.active.contains(&true);
        let [x, y] = if direct {
            self.delta
        } else {
            std::array::from_fn(|index| self.wheel_ticks[index].map_or(self.delta[index], |ticks| ticks * 32.0))
        };
        if x != 0.0 || y != 0.0 {
            if direct {
                for index in 0..2 {
                    self.active[index] |= self.delta[index] != 0.0;
                }
            }
            let phase = if !direct {
                WheelPhase::Discrete
            } else if before {
                WheelPhase::Direct(WheelStage::Move)
            } else {
                WheelPhase::Direct(WheelStage::Start)
            };
            self.emit(shared, focus, mods, Vec2::new(x, y), phase);
        }
        let was_active = self.active.contains(&true);
        for index in 0..2 {
            self.active[index] &= !self.stopped[index];
        }
        if was_active && !self.active.contains(&true) {
            self.emit(shared, focus, mods, Vec2::default(), WheelPhase::Direct(WheelStage::End));
        }
        self.delta = [0.0; 2];
        self.wheel_ticks = [None; 2];
        self.stopped = [false; 2];
    }
}

impl Seat {
    pub(super) fn record_serial(&mut self, surface: wl_surface::WlSurface, serial: u32) {
        if self.keyboard.focus.as_ref().is_some_and(|(_, focused)| focused == &surface) {
            self.clipboard_serial = Some(serial);
        }
        self.user_serial = Some(UserSerial { surface, serial, time: Instant::now() });
    }

    fn remove(&mut self, shared: &Shared, capability: Capability) {
        match capability {
            Capability::Pointer => {
                if let Some(mut pointer) = self.pointer.take() {
                    pointer.leave(shared, self.keyboard.modifiers);
                    if let Some(shape) = pointer.cursor_shape {
                        shape.destroy();
                    }
                    if let Some(surface) = pointer.cursor_surface {
                        surface.destroy();
                    }
                    if let Some(pinch) = pointer.pinch {
                        pinch.destroy();
                    }
                    if pointer.object.version() >= 3 {
                        pointer.object.release();
                    }
                }
            }
            Capability::Keyboard => {
                self.clipboard_serial = None;
                self.keyboard.release();
            }
            Capability::Touch => {
                self.cancel_contacts(shared);
                if let Some(touch) = self.touch.take()
                    && touch.version() >= 3
                {
                    touch.release();
                }
            }
            _ => {}
        }
    }

    fn cancel_contacts(&mut self, shared: &Shared) {
        for contact in self.contacts.drain().map(|(_, contact)| contact).filter(|contact| contact.serial.is_some()) {
            contact.emit(shared, PointerPhase::Cancelled, self.keyboard.modifiers);
        }
    }
}

impl Contact {
    fn emit(&self, shared: &Shared, phase: PointerPhase, modifiers: Modifiers) {
        shared.emit(self.event(phase, modifiers));
    }

    fn event(&self, phase: PointerPhase, modifiers: Modifiers) -> PlatformEvent {
        PlatformEvent::Pointer {
            window: self.window,
            phase,
            position: Some(self.position),
            id: self.id,
            kind: PointerKind::Touch,
            button: Button::Primary,
            modifiers,
        }
    }
}

impl Pointer {
    fn leave(&mut self, shared: &Shared, modifiers: Modifiers) {
        if let Some(gesture) = self.gesture.take() {
            gesture.emit(shared, GesturePhase::Cancelled, 0.0, 0.0, modifiers);
        }
        let Some(focus) = self.focus.take() else { return };
        if self.scroll.active.contains(&true) {
            self.scroll.emit(shared, &focus, modifiers, Vec2::default(), WheelPhase::Direct(WheelStage::Cancel));
        }
        self.scroll = Scroll::default();
        if self.grab.take().is_some() {
            shared.emit(focus.button(PointerPhase::Cancelled, Button::Primary, modifiers));
        }
        shared.emit(PlatformEvent::CursorLeft(focus.window));
    }
}

impl PointerFocus {
    fn motion(&self, shared: &Shared, modifiers: Modifiers) {
        shared.emit(PlatformEvent::CursorMoved { window: self.window, position: self.position, modifiers });
    }

    fn button(&self, phase: PointerPhase, button: Button, modifiers: Modifiers) -> PlatformEvent {
        PlatformEvent::Pointer {
            window: self.window,
            phase,
            position: Some(self.position),
            id: PointerId(0),
            kind: PointerKind::Mouse,
            button,
            modifiers,
        }
    }
}

impl Shared {
    pub(super) fn seat(&self, id: &ObjectId) -> Option<Rc<RefCell<Seat>>> {
        self.seats.borrow().get(id).cloned()
    }

    pub(super) fn activation_serial(&self) -> Option<(wl_seat::WlSeat, u32, wl_surface::WlSurface)> {
        self.seats
            .borrow()
            .values()
            .filter_map(|seat| {
                let seat = seat.borrow();
                let event = seat.user_serial.as_ref().filter(|event| self.window(&event.surface).is_some())?;
                Some((event.time, seat.native.clone(), event.serial, event.surface.clone()))
            })
            .max_by_key(|event| event.0)
            .map(|(_, seat, serial, surface)| (seat, serial, surface))
    }

    fn emit_grab(&self, event: PlatformEvent, seat: &ObjectId, contact: GrabContact, serial: u32) {
        let origin = GrabOrigin { seat: seat.clone(), contact, serial };
        self.events.borrow_mut().push_back(NativeEvent::Grab(event, origin));
    }

    pub(super) fn take_grab(&self, window: WindowId) -> Option<(wl_seat::WlSeat, u32)> {
        // A single protocol dispatch can queue events from more than one grab.
        let origin = self.dispatch_grab.borrow();
        let origin = origin.as_ref()?;
        let seat = self.seat(&origin.seat)?;
        let mut seat = seat.borrow_mut();
        let modifiers = seat.keyboard.modifiers;
        match origin.contact {
            GrabContact::Pointer => {
                let pointer = seat.pointer.as_mut()?;
                let focus = pointer.focus.as_ref().filter(|focus| focus.window == window)?;
                pointer.grab.take_if(|serial| *serial == origin.serial)?;
                self.emit(focus.button(PointerPhase::Cancelled, Button::Primary, modifiers));
            }
            GrabContact::Touch(id) => {
                let contact = seat.contacts.get_mut(&id).filter(|contact| contact.window == window)?;
                contact.serial.take_if(|serial| *serial == origin.serial)?;
                contact.emit(self, PointerPhase::Cancelled, modifiers);
            }
        }
        Some((seat.native.clone(), origin.serial))
    }

    pub(super) fn update_cursor(&self, window: WindowId) {
        for seat in self.seats.borrow().values() {
            if let Some(pointer) = &mut seat.borrow_mut().pointer
                && pointer.focus.as_ref().is_some_and(|focus| focus.window == window)
            {
                self.apply_cursor(pointer);
            }
        }
    }

    fn apply_cursor(&self, pointer: &mut Pointer) {
        let Some(focus) = &pointer.focus else { return };
        let Some(window) = self.window(&focus.surface) else { return };
        let style = window.cursor.get();
        if style == CursorStyle::None {
            pointer.object.set_cursor(focus.enter_serial, None, 0, 0);
            return;
        }
        let (shape, name, fallback) = cursor_style(style);
        if let Some(device) = &pointer.cursor_shape {
            device.set_shape(focus.enter_serial, shape);
        } else if let Some(shm) = &self.cursor_shm {
            let surface = pointer.cursor_surface.get_or_insert_with(|| self.compositor.create_surface(&self.queue));
            let scale = if surface.version() >= 3 { (window.scale().ceil() as u32).clamp(1, 8) } else { 1 };
            let mut cursors = self.cursors.borrow_mut();
            let base = cursors.size;
            let theme = cursors.themes.entry(scale);
            let theme = match theme {
                std::collections::hash_map::Entry::Occupied(theme) => theme.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    let name = std::env::var("XCURSOR_THEME").unwrap_or_else(|_| "default".into());
                    let Ok(theme) = CursorTheme::load_from_name(&self.connection, shm.clone(), &name, base * scale) else { return };
                    entry.insert(theme)
                }
            };
            let cursor_name = [name, fallback, "left_ptr"].into_iter().find(|name| theme.get_cursor(name).is_some());
            if let Some(cursor) = cursor_name.and_then(|name| theme.get_cursor(name)) {
                let frame = &cursor[0];
                let (x, y) = frame.hotspot();
                let (width, height) = frame.dimensions();
                let scale = if width.is_multiple_of(scale) && height.is_multiple_of(scale) { scale } else { 1 };
                pointer.object.set_cursor(focus.enter_serial, Some(surface), (x / scale) as i32, (y / scale) as i32);
                if surface.version() >= 3 {
                    surface.set_buffer_scale(scale as i32);
                }
                surface.attach(Some(frame), 0, 0);
                surface.damage(0, 0, width.div_ceil(scale) as i32, height.div_ceil(scale) as i32);
                surface.commit();
            }
        }
    }
}

impl Protocol {
    pub(super) fn add_seat(&self, native: wl_seat::WlSeat) {
        self.shared.seats.borrow_mut().entry(native.id()).or_insert_with(|| {
            Rc::new(RefCell::new(Seat {
                ime: Ime::new(&self.shared, &native),
                data: DataSeat::new(&self.shared, &native),
                native,
                keyboard: Keyboard::default(),
                user_serial: None,
                clipboard_serial: None,
                pointer: None,
                touch: None,
                contacts: HashMap::new(),
            }))
        });
    }
}

impl SeatHandler for Protocol {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seats
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, seat: wl_seat::WlSeat) {
        self.add_seat(seat);
    }

    fn new_capability(&mut self, _: &Connection, queue: &QueueHandle<Self>, native: wl_seat::WlSeat, capability: Capability) {
        let Some(seat) = self.shared.seat(&native.id()) else { return };
        let mut seat = seat.borrow_mut();
        match capability {
            Capability::Pointer => {
                let object = native.get_pointer(queue, native.id());
                let cursor_shape = self.shared.cursor_shape.as_ref().map(|manager| manager.get_pointer(&object, queue, ()));
                let pinch = self.shared.gestures.as_ref().map(|manager| manager.get_pinch_gesture(&object, queue, native.id()));
                seat.pointer = Some(Pointer {
                    object,
                    focus: None,
                    grab: None,
                    cursor_shape,
                    cursor_surface: None,
                    pinch,
                    gesture: None,
                    scroll: Scroll::default(),
                });
            }
            Capability::Keyboard => seat.keyboard.object = Some(native.get_keyboard(queue, native.id())),
            Capability::Touch => seat.touch = Some(native.get_touch(queue, native.id())),
            _ => {}
        }
    }

    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, native: wl_seat::WlSeat, capability: Capability) {
        if let Some(seat) = self.shared.seat(&native.id()) {
            seat.borrow_mut().remove(&self.shared, capability);
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, native: wl_seat::WlSeat) {
        if let Some(seat) = self.shared.seats.borrow_mut().remove(&native.id()) {
            let mut seat = seat.borrow_mut();
            for capability in [Capability::Pointer, Capability::Keyboard, Capability::Touch] {
                seat.remove(&self.shared, capability);
            }
            seat.ime.leave(&self.shared, native.id());
            seat.data.leave(&self.shared);
        }
    }
}

fn axis_index(axis: WEnum<wl_pointer::Axis>) -> Option<usize> {
    match axis {
        WEnum::Value(wl_pointer::Axis::HorizontalScroll) => Some(0),
        WEnum::Value(wl_pointer::Axis::VerticalScroll) => Some(1),
        _ => None,
    }
}

impl Dispatch<wl_pointer::WlPointer, ObjectId> for Protocol {
    fn event(
        state: &mut Self,
        object: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        data: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wl_pointer::Event::*;

        let Some(seat) = state.shared.seat(data) else { return };
        let mut seat = seat.borrow_mut();
        let modifiers = seat.keyboard.modifiers;
        let Some(pointer) = seat.pointer.as_mut().filter(|pointer| pointer.object == *object) else { return };
        match event {
            Enter { serial, surface, surface_x, surface_y } => {
                if let Some(window) = state.shared.window(&surface) {
                    let focus = PointerFocus {
                        surface,
                        window: window.id,
                        position: Point::new(surface_x as f32, surface_y as f32),
                        enter_serial: serial,
                    };
                    focus.motion(&state.shared, modifiers);
                    pointer.focus = Some(focus);
                    state.shared.apply_cursor(pointer);
                }
            }
            Leave { .. } => pointer.leave(&state.shared, modifiers),
            Motion { surface_x, surface_y, .. } => {
                if let Some(focus) = &mut pointer.focus {
                    focus.position = Point::new(surface_x as f32, surface_y as f32);
                    if let Some(serial) = pointer.grab {
                        let event = focus.button(PointerPhase::Moved, self::Button::Primary, modifiers);
                        state.shared.emit_grab(event, data, GrabContact::Pointer, serial);
                    } else {
                        focus.motion(&state.shared, modifiers);
                    }
                }
            }
            Button { button, state: input, serial, .. } => {
                let button = match button {
                    0x110 => self::Button::Primary,
                    0x111 => self::Button::Secondary,
                    0x112 => self::Button::Middle,
                    _ => return,
                };
                let Some(focus) = &pointer.focus else { return };
                match input {
                    WEnum::Value(wl_pointer::ButtonState::Pressed) => {
                        let event = focus.button(PointerPhase::Started, button, modifiers);
                        if button == self::Button::Primary {
                            pointer.grab = Some(serial);
                            state.shared.emit_grab(event, data, GrabContact::Pointer, serial);
                        } else {
                            state.shared.emit(event);
                        }
                        let surface = focus.surface.clone();
                        seat.record_serial(surface, serial);
                    }
                    WEnum::Value(wl_pointer::ButtonState::Released) => {
                        if button == self::Button::Primary {
                            pointer.grab = None;
                        }
                        state.shared.emit(focus.button(PointerPhase::Ended, button, modifiers));
                    }
                    _ => {}
                }
            }
            AxisSource { axis_source } => pointer.scroll.source = axis_source.into_result().ok(),
            Axis { axis, value, .. } => {
                if let Some(index) = axis_index(axis) {
                    pointer.scroll.delta[index] += value as f32;
                    if object.version() < 5
                        && let Some(focus) = &pointer.focus
                    {
                        pointer.scroll.flush(&state.shared, focus, modifiers);
                    }
                }
            }
            AxisStop { axis, .. } => {
                if let Some(index) = axis_index(axis) {
                    pointer.scroll.stopped[index] = true;
                }
            }
            AxisDiscrete { axis, discrete } => {
                if let Some(index) = axis_index(axis) {
                    pointer.scroll.wheel_ticks[index] = Some(discrete as f32);
                }
            }
            AxisValue120 { axis, value120 } => {
                if let Some(index) = axis_index(axis) {
                    pointer.scroll.wheel_ticks[index] = Some(value120 as f32 / 120.0);
                }
            }
            Frame => {
                if let Some(focus) = &pointer.focus {
                    pointer.scroll.flush(&state.shared, focus, modifiers);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_touch::WlTouch, ObjectId> for Protocol {
    fn event(state: &mut Self, object: &wl_touch::WlTouch, event: wl_touch::Event, data: &ObjectId, _: &Connection, _: &QueueHandle<Self>) {
        use wl_touch::Event::*;

        let Some(seat) = state.shared.seat(data) else { return };
        let mut seat = seat.borrow_mut();
        if seat.touch.as_ref() != Some(object) {
            return;
        }
        let modifiers = seat.keyboard.modifiers;
        match event {
            Down { serial, surface, id, x, y, .. } => {
                if let Some(window) = state.shared.window(&surface) {
                    let pointer = (u64::from(data.protocol_id()) << 32) | u64::from(id as u32);
                    let contact = Contact {
                        window: window.id,
                        id: PointerId(pointer),
                        position: Point::new(x as f32, y as f32),
                        serial: Some(serial),
                    };
                    let event = contact.event(PointerPhase::Started, modifiers);
                    state.shared.emit_grab(event, data, GrabContact::Touch(id), serial);
                    seat.contacts.insert(id, contact);
                    seat.record_serial(surface, serial);
                }
            }
            Motion { id, x, y, .. } => {
                if let Some(contact) = seat.contacts.get_mut(&id)
                    && contact.serial.is_some()
                {
                    contact.position = Point::new(x as f32, y as f32);
                    contact.emit(&state.shared, PointerPhase::Moved, modifiers);
                }
            }
            Up { id, .. } => {
                if let Some(contact) = seat.contacts.remove(&id)
                    && contact.serial.is_some()
                {
                    contact.emit(&state.shared, PointerPhase::Ended, modifiers);
                }
            }
            Cancel => seat.cancel_contacts(&state.shared),
            _ => {}
        }
    }
}

impl Dispatch<ZwpPointerGesturePinchV1, ObjectId> for Protocol {
    fn event(
        state: &mut Self,
        object: &ZwpPointerGesturePinchV1,
        event: zwp_pointer_gesture_pinch_v1::Event,
        data: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwp_pointer_gesture_pinch_v1::Event::*;

        let Some(seat) = state.shared.seat(data) else { return };
        let mut seat = seat.borrow_mut();
        let modifiers = seat.keyboard.modifiers;
        let Some(pointer) = seat.pointer.as_mut().filter(|pointer| pointer.pinch.as_ref() == Some(object)) else { return };
        match event {
            Begin { surface, .. } => {
                if let Some(focus) = pointer.focus.as_ref().filter(|focus| focus.surface == surface) {
                    let gesture = Pinch { window: focus.window, anchor: focus.position, scale: 1.0, rotation: 0.0 };
                    gesture.emit(&state.shared, GesturePhase::Started, 1.0, 0.0, modifiers);
                    pointer.gesture = Some(gesture);
                }
            }
            Update { scale, rotation, .. } => {
                if let Some(gesture) = &mut pointer.gesture
                    && let Some((delta_scale, delta_radians)) = gesture.update(scale as f32, rotation as f32)
                {
                    gesture.emit(&state.shared, GesturePhase::Changed, delta_scale, delta_radians, modifiers);
                }
            }
            End { cancelled, .. } => {
                if let Some(gesture) = pointer.gesture.take() {
                    let phase = if cancelled == 0 { GesturePhase::Ended } else { GesturePhase::Cancelled };
                    gesture.emit(&state.shared, phase, 1.0, 0.0, modifiers);
                }
            }
            _ => {}
        }
    }
}

pub(super) struct Cursors {
    size: u32,
    themes: HashMap<u32, CursorTheme>,
}

impl Default for Cursors {
    fn default() -> Self {
        let size = std::env::var("XCURSOR_SIZE").ok().and_then(|value| value.parse::<u32>().ok()).unwrap_or(24).clamp(1, 128);
        Self { size, themes: HashMap::new() }
    }
}

fn cursor_style(style: CursorStyle) -> (Shape, &'static str, &'static str) {
    match style {
        CursorStyle::Arrow | CursorStyle::Auto | CursorStyle::None => (Shape::Default, "default", "left_ptr"),
        CursorStyle::IBeam => (Shape::Text, "text", "xterm"),
        CursorStyle::PointingHand => (Shape::Pointer, "pointer", "hand2"),
        CursorStyle::Crosshair => (Shape::Crosshair, "crosshair", "crosshair"),
        CursorStyle::OpenHand => (Shape::Grab, "grab", "hand1"),
        CursorStyle::ClosedHand => (Shape::Grabbing, "grabbing", "hand2"),
        CursorStyle::ResizeLeftRight => (Shape::EwResize, "ew-resize", "sb_h_double_arrow"),
        CursorStyle::ResizeUpDown => (Shape::NsResize, "ns-resize", "sb_v_double_arrow"),
        CursorStyle::ResizeColumn => (Shape::ColResize, "col-resize", "sb_h_double_arrow"),
        CursorStyle::ResizeRow => (Shape::RowResize, "row-resize", "sb_v_double_arrow"),
        CursorStyle::OperationNotAllowed => (Shape::NotAllowed, "not-allowed", "crossed_circle"),
        CursorStyle::Wait => (Shape::Wait, "wait", "watch"),
        CursorStyle::Move => (Shape::Move, "move", "fleur"),
        CursorStyle::Help => (Shape::Help, "help", "question_arrow"),
        CursorStyle::ContextMenu => (Shape::ContextMenu, "context-menu", "left_ptr"),
        CursorStyle::Progress => (Shape::Progress, "progress", "left_ptr_watch"),
        CursorStyle::Cell => (Shape::Cell, "cell", "plus"),
        CursorStyle::VerticalText => (Shape::VerticalText, "vertical-text", "xterm"),
        CursorStyle::Alias => (Shape::Alias, "alias", "hand2"),
        CursorStyle::Copy => (Shape::Copy, "copy", "hand2"),
        CursorStyle::NoDrop => (Shape::NoDrop, "no-drop", "crossed_circle"),
        CursorStyle::AllScroll => (Shape::AllScroll, "all-scroll", "fleur"),
        CursorStyle::ResizeNorth => (Shape::NResize, "n-resize", "top_side"),
        CursorStyle::ResizeEast => (Shape::EResize, "e-resize", "right_side"),
        CursorStyle::ResizeSouth => (Shape::SResize, "s-resize", "bottom_side"),
        CursorStyle::ResizeWest => (Shape::WResize, "w-resize", "left_side"),
        CursorStyle::ResizeNorthEast => (Shape::NeResize, "ne-resize", "top_right_corner"),
        CursorStyle::ResizeNorthWest => (Shape::NwResize, "nw-resize", "top_left_corner"),
        CursorStyle::ResizeSouthEast => (Shape::SeResize, "se-resize", "bottom_right_corner"),
        CursorStyle::ResizeSouthWest => (Shape::SwResize, "sw-resize", "bottom_left_corner"),
        CursorStyle::ResizeNorthEastSouthWest => (Shape::NeswResize, "nesw-resize", "fd_double_arrow"),
        CursorStyle::ResizeNorthWestSouthEast => (Shape::NwseResize, "nwse-resize", "bd_double_arrow"),
        CursorStyle::ZoomIn => (Shape::ZoomIn, "zoom-in", "left_ptr"),
        CursorStyle::ZoomOut => (Shape::ZoomOut, "zoom-out", "left_ptr"),
    }
}

delegate_noop!(Protocol: ignore wp_cursor_shape_device_v1::WpCursorShapeDeviceV1);

#[cfg(test)]
mod tests {
    use super::Pinch;
    use octara_model::geometry::Point;

    #[test]
    fn pinch_uses_relative_scale_and_counterclockwise_rotation() {
        let mut pinch = Pinch { window: Default::default(), anchor: Point::new(1.0, 2.0), scale: 1.0, rotation: 0.0 };
        assert_eq!(pinch.update(2.0, 90.0), Some((2.0, -std::f32::consts::FRAC_PI_2)));
        assert_eq!(pinch.update(1.5, -90.0), Some((0.75, std::f32::consts::FRAC_PI_2)));
        assert_eq!(pinch.rotation, 0.0);
        assert_eq!(pinch.update(0.0, 20.0), None);
        assert_eq!(pinch.scale, 1.5);
    }
}

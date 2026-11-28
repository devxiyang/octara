use super::{Native, XResult};
use crate::os::linux::compose::KeySymbol;
use crate::os::{PlatformEvent, PlatformKey, PointerPhase};
use octara_model::geometry::{Point, Vec2};
use octara_model::input::{Button, InteractionEvent, Key, KeyEvent, Modifiers, PointerId, PointerKind, WheelPhase};
use std::ptr::NonNull;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xkb::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{ButtonPressEvent, ConnectionExt as _, KeyButMask, KeyPressEvent, NotifyDetail, NotifyMode};
use xkbcommon_dl::{self as xkb_api, keysyms as sym, x11 as xkb_x11};

#[derive(Clone, Copy)]
pub(super) struct PointerPosition {
    pub root_x: i16,
    pub root_y: i16,
}

pub(super) struct Keyboard {
    state: Option<XkbState>,
    pressed: [bool; 256],
    released: Option<(u32, u8)>,
    alt: u16,
    meta: u16,
}

impl Keyboard {
    pub fn new(native: &Native) -> Self {
        let _ = setup_xkb_events(native);
        let mut keyboard = Self { state: XkbState::new(native), pressed: [false; 256], released: None, alt: 1 << 3, meta: 1 << 6 };
        keyboard.refresh_modifiers(native);
        keyboard
    }

    fn refresh(&mut self, native: &Native) {
        self.state = XkbState::new(native);
        self.refresh_modifiers(native);
    }

    fn refresh_modifiers(&mut self, native: &Native) {
        let Some(mapping) = native.conn.get_modifier_mapping().ok().and_then(|cookie| cookie.reply().ok()) else { return };
        let per_modifier = usize::from(mapping.keycodes_per_modifier());
        if per_modifier == 0 {
            return;
        }
        let setup = native.conn.setup();
        let min = setup.min_keycode;
        let Some(keymap) = native.conn.get_keyboard_mapping(min, setup.max_keycode - min + 1).ok().and_then(|cookie| cookie.reply().ok())
        else {
            return;
        };
        let mut alt = 0;
        let mut meta = 0;
        for (index, keys) in mapping.keycodes.chunks_exact(per_modifier).enumerate() {
            for key in keys.iter().copied().filter(|key| *key >= min) {
                let offset = usize::from(key - min) * usize::from(keymap.keysyms_per_keycode);
                match keymap.keysyms.get(offset).copied() {
                    Some(sym::Alt_L | sym::Alt_R) => alt |= 1 << index,
                    Some(sym::Super_L | sym::Super_R | sym::Hyper_L | sym::Hyper_R | sym::Meta_L | sym::Meta_R) => meta |= 1 << index,
                    _ => {}
                }
            }
        }
        self.alt = alt;
        self.meta = meta & !alt;
    }

    fn modifiers(&self, state: KeyButMask) -> Modifiers {
        let mut modifiers = Modifiers::empty();
        modifiers.set(Modifiers::SHIFT, state.contains(KeyButMask::SHIFT));
        modifiers.set(Modifiers::CTRL, state.contains(KeyButMask::CONTROL));
        modifiers.set(Modifiers::ALT, u16::from(state) & self.alt != 0);
        modifiers.set(Modifiers::META, u16::from(state) & self.meta != 0);
        modifiers
    }

    pub fn keysym(&self, native: &Native, event: &KeyPressEvent) -> u32 {
        if let Some(state) = &self.state {
            return state.keysym(event.detail, event.state);
        }
        let mut modifiers = 0;
        let mut keysym = 0;
        // SAFETY: The display is live and both XKB outputs point to writable values.
        unsafe {
            (native.display.xlib.XkbLookupKeySym)(
                native.display.ptr.as_ptr(),
                event.detail,
                u32::from(u16::from(event.state)),
                &mut modifiers,
                &mut keysym,
            );
        }
        keysym as u32
    }

    fn press(&mut self, native: &Native, event: &KeyPressEvent) -> (KeySymbol, PlatformKey) {
        let repeat = std::mem::replace(&mut self.pressed[usize::from(event.detail)], true)
            || self.released.take() == Some((event.time, event.detail));
        let modifiers = self.modifiers(event.state);
        let symbol = self.keysym(native, event);
        let logical = logical_key(symbol).map(|key| KeyEvent { key, modifiers, code: physical_code(event.detail), repeat });
        let text = if modifiers.intersects(Modifiers::CTRL | Modifiers::META) {
            None
        } else {
            keysym_character(symbol).map(|character| character.to_string())
        };
        (KeySymbol { value: symbol, modifiers }, PlatformKey { event: logical, text })
    }

    fn release(&mut self, event: &KeyPressEvent) {
        self.pressed[usize::from(event.detail)] = false;
        self.released = Some((event.time, event.detail));
    }

    fn focus_lost(&mut self) {
        self.pressed.fill(false);
        self.released = None;
    }
}

fn setup_xkb_events(native: &Native) -> XResult<()> {
    let conn = &native.conn;
    if !conn.xkb_use_extension(1, 0)?.reply()?.supported {
        return Ok(());
    }
    let device = u16::from(xkb::ID::USE_CORE_KBD);
    let events = xkb::EventType::NEW_KEYBOARD_NOTIFY | xkb::EventType::MAP_NOTIFY;
    let parts = xkb::MapPart::KEY_TYPES
        | xkb::MapPart::KEY_SYMS
        | xkb::MapPart::MODIFIER_MAP
        | xkb::MapPart::EXPLICIT_COMPONENTS
        | xkb::MapPart::KEY_ACTIONS
        | xkb::MapPart::KEY_BEHAVIORS
        | xkb::MapPart::VIRTUAL_MODS
        | xkb::MapPart::VIRTUAL_MOD_MAP;
    conn.xkb_select_events(device, xkb::EventType::default(), events, parts, parts, &xkb::SelectEventsAux::default())?.check()?;
    let flags = xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT;
    let no_controls = xkb::BoolCtrl::default();
    conn.xkb_per_client_flags(device, flags, flags, no_controls, no_controls, no_controls)?.reply()?;
    Ok(())
}

struct XkbState {
    library: &'static xkb_api::XkbCommon,
    state: NonNull<xkb_api::xkb_state>,
}

impl XkbState {
    fn new(native: &Native) -> Option<Self> {
        let library = xkb_api::xkbcommon_option()?;
        let x11 = xkb_x11::xkbcommon_x11_option()?;
        let connection = native.conn.get_raw_xcb_connection();
        // SAFETY: XCB owns the event queue and provides a live connection. Each created
        // context/keymap reference is released after the next owner takes its own reference.
        unsafe {
            let device = (x11.xkb_x11_get_core_keyboard_device_id)(connection);
            if device < 0 {
                return None;
            }
            let context = NonNull::new((library.xkb_context_new)(xkb_api::xkb_context_flags::XKB_CONTEXT_NO_FLAGS))?;
            let keymap = (x11.xkb_x11_keymap_new_from_device)(
                context.as_ptr(),
                connection,
                device,
                xkb_api::xkb_keymap_compile_flags::XKB_KEYMAP_COMPILE_NO_FLAGS,
            );
            (library.xkb_context_unref)(context.as_ptr());
            let keymap = NonNull::new(keymap)?;
            let state = (x11.xkb_x11_state_new_from_device)(keymap.as_ptr(), connection, device);
            (library.xkb_keymap_unref)(keymap.as_ptr());
            Some(Self { library, state: NonNull::new(state)? })
        }
    }

    fn keysym(&self, keycode: u8, state: KeyButMask) -> u32 {
        let bits = u32::from(u16::from(state));
        // Core key events carry the effective XKB modifier bits in 0–7 and group in 13–14.
        // SAFETY: The state is owned and accessed on the event loop thread.
        unsafe {
            (self.library.xkb_state_update_mask)(self.state.as_ptr(), bits & 0xff, 0, 0, 0, 0, (bits >> 13) & 3);
            (self.library.xkb_state_key_get_one_sym)(self.state.as_ptr(), u32::from(keycode))
        }
    }
}

impl Drop for XkbState {
    fn drop(&mut self) {
        // SAFETY: This instance owns one state reference.
        unsafe { (self.library.xkb_state_unref)(self.state.as_ptr()) };
    }
}

fn keysym_character(symbol: u32) -> Option<char> {
    let value = if let Some(lib) = xkb_api::xkbcommon_option() {
        // SAFETY: The conversion operates on a scalar and has no lifetime requirements.
        unsafe { (lib.xkb_keysym_to_utf32)(symbol) }
    } else {
        match symbol {
            0x20..=0x7e | 0xa0..=0xff => symbol,
            0x0100_0100..=0x0110_ffff => symbol & 0x00ff_ffff,
            _ => return None,
        }
    };
    char::from_u32(value).filter(|character| !character.is_control())
}

fn logical_key(symbol: u32) -> Option<Key> {
    Some(match symbol {
        sym::Tab | sym::ISO_Left_Tab | sym::KP_Tab => Key::Tab,
        sym::Return | sym::KP_Enter | sym::ISO_Enter => Key::Enter,
        sym::Escape => Key::Escape,
        sym::BackSpace => Key::Backspace,
        sym::Delete | sym::KP_Delete => Key::Delete,
        sym::space | sym::KP_Space => Key::Space,
        sym::Up | sym::KP_Up => Key::Up,
        sym::Down | sym::KP_Down => Key::Down,
        sym::Left | sym::KP_Left => Key::Left,
        sym::Right | sym::KP_Right => Key::Right,
        sym::Home | sym::KP_Home => Key::Home,
        sym::End | sym::KP_End => Key::End,
        sym::Page_Up | sym::KP_Page_Up => Key::PageUp,
        sym::Page_Down | sym::KP_Page_Down => Key::PageDown,
        sym::Menu => Key::Menu,
        sym::F1..=sym::F24 => Key::F((symbol - sym::F1 + 1) as u8),
        _ => Key::Char(keysym_character(symbol)?),
    })
}

fn physical_code(keycode: u8) -> Option<char> {
    // Xorg's evdev keycodes for the printable positions of a US keyboard.
    let (row, start) = match keycode {
        10..=21 => (b"1234567890-=".as_slice(), 10),
        24..=35 => (b"qwertyuiop[]".as_slice(), 24),
        38..=49 => (b"asdfghjkl;'`".as_slice(), 38),
        51..=61 => (b"\\zxcvbnm,./".as_slice(), 51),
        _ => return None,
    };
    Some(char::from(row[usize::from(keycode - start)]))
}

pub(super) fn translate(native: &Native, keyboard: &mut Keyboard, event: Event) -> Option<PlatformEvent> {
    let scale = native.scale.get();
    match event {
        Event::Expose(event) if event.count == 0 => Some(PlatformEvent::WindowRedrawRequested(native.window(event.window)?.id)),
        Event::ConfigureNotify(event) => {
            let window = native.window(event.window)?;
            let size = [u32::from(event.width), u32::from(event.height)];
            let changed = window.size.replace(size) != size;
            // The shell refreshes position when it processes either event.
            Some(if changed { PlatformEvent::WindowResized(window.id) } else { PlatformEvent::WindowMoved(window.id) })
        }
        Event::PropertyNotify(event) if event.atom == native.atoms._NET_WM_STATE || event.atom == native.atoms._NET_FRAME_EXTENTS => {
            let window = native.window(event.window)?;
            Some(PlatformEvent::WindowMoved(window.id))
        }
        Event::ClientMessage(event) if event.type_ == native.atoms.WM_PROTOCOLS && event.format == 32 => {
            let window = native.window(event.window)?;
            (event.data.as_data32()[0] == native.atoms.WM_DELETE_WINDOW).then_some(PlatformEvent::WindowCloseRequested(window.id))
        }
        Event::DestroyNotify(event) => {
            let window = native.window(event.window)?;
            window.destroyed.set(true);
            native.unregister(event.window);
            Some(PlatformEvent::WindowDestroyed(window.id))
        }
        Event::MapNotify(event) => {
            let window = native.window(event.window)?;
            window.mapped.set(true);
            window.update_accessibility(native);
            window.show_without_window_manager(native);
            Some(PlatformEvent::WindowRedrawRequested(window.id))
        }
        Event::UnmapNotify(event) => {
            let window = native.window(event.window)?;
            window.mapped.set(false);
            window.primary_down.set(false);
            None
        }
        Event::FocusIn(event) => focus(native, keyboard, event.event, event.mode, event.detail, true),
        Event::FocusOut(event) => focus(native, keyboard, event.event, event.mode, event.detail, false),
        Event::EnterNotify(event) if event.mode == NotifyMode::NORMAL && event.detail != NotifyDetail::INFERIOR => {
            let window = native.window(event.event)?;
            window.pointer.set(Some(PointerPosition { root_x: event.root_x, root_y: event.root_y }));
            Some(PlatformEvent::CursorMoved {
                window: window.id,
                position: position(event.event_x, event.event_y, scale),
                modifiers: keyboard.modifiers(event.state),
            })
        }
        Event::LeaveNotify(event) if event.mode == NotifyMode::NORMAL && event.detail != NotifyDetail::INFERIOR => {
            let window = native.window(event.event)?;
            window.pointer.set(None);
            Some(PlatformEvent::CursorLeft(window.id))
        }
        Event::MotionNotify(event) => {
            let window = native.window(event.event)?;
            native.timestamp.set(event.time);
            window.pointer.set(Some(PointerPosition { root_x: event.root_x, root_y: event.root_y }));
            Some(PlatformEvent::CursorMoved {
                window: window.id,
                position: position(event.event_x, event.event_y, scale),
                modifiers: keyboard.modifiers(event.state),
            })
        }
        Event::ButtonPress(event) => button(native, keyboard, event, true),
        Event::ButtonRelease(event) => button(native, keyboard, event, false),
        Event::KeyPress(event) => {
            let window = native.window(event.event)?;
            native.timestamp.set(event.time);
            let (symbol, event) = keyboard.press(native, &event);
            native.pending_keysym.set(Some((window.id, symbol)));
            Some(PlatformEvent::Key { window: window.id, event })
        }
        Event::KeyRelease(event) => {
            keyboard.release(&event);
            None
        }
        Event::XkbNewKeyboardNotify(_) | Event::XkbMapNotify(_) => {
            keyboard.refresh(native);
            None
        }
        Event::MappingNotify(event) => {
            let mut event = x11_dl::xlib::XMappingEvent {
                type_: x11_dl::xlib::MappingNotify,
                serial: 0,
                send_event: 0,
                display: native.display.ptr.as_ptr(),
                event: 0,
                request: u8::from(event.request).into(),
                first_keycode: event.first_keycode.into(),
                count: event.count.into(),
            };
            // SAFETY: This event mirrors an X11 MappingNotify from the current display.
            unsafe { (native.display.xlib.XRefreshKeyboardMapping)(&mut event) };
            keyboard.refresh(native);
            None
        }
        _ => None,
    }
}

fn focus(
    native: &Native,
    keyboard: &mut Keyboard,
    xid: u32,
    mode: NotifyMode,
    detail: NotifyDetail,
    active: bool,
) -> Option<PlatformEvent> {
    let window = native.window(xid)?;
    if !matches!(mode, NotifyMode::NORMAL | NotifyMode::WHILE_GRABBED) || detail == NotifyDetail::INFERIOR || window.focused.get() == active
    {
        return None;
    }
    window.set_focused(native, active);
    if !active {
        keyboard.focus_lost();
    }
    Some(PlatformEvent::WindowFocused { window: window.id, active })
}

fn button(native: &Native, keyboard: &Keyboard, event: ButtonPressEvent, pressed: bool) -> Option<PlatformEvent> {
    let window = native.window(event.event)?;
    native.timestamp.set(event.time);
    let point = position(event.event_x, event.event_y, native.scale.get());
    let mods = keyboard.modifiers(event.state);
    window.pointer.set(Some(PointerPosition { root_x: event.root_x, root_y: event.root_y }));
    let button = match event.detail {
        1 => {
            window.primary_down.set(pressed);
            Button::Primary
        }
        2 => Button::Middle,
        3 => Button::Secondary,
        4..=7 if pressed => {
            let delta = match event.detail {
                4 => Vec2::new(0.0, -32.0),
                5 => Vec2::new(0.0, 32.0),
                6 => Vec2::new(-32.0, 0.0),
                _ => Vec2::new(32.0, 0.0),
            };
            return Some(PlatformEvent::Interaction {
                window: window.id,
                event: InteractionEvent::Wheel { pos: point, delta, phase: WheelPhase::Discrete, mods },
            });
        }
        _ => return None,
    };
    Some(PlatformEvent::Pointer {
        window: window.id,
        phase: if pressed { PointerPhase::Started } else { PointerPhase::Ended },
        position: Some(point),
        id: PointerId(0),
        kind: PointerKind::Mouse,
        button,
        modifiers: mods,
    })
}

pub(super) fn position(x: i16, y: i16, scale: f32) -> Point {
    Point::new(f32::from(x) / scale, f32::from(y) / scale)
}

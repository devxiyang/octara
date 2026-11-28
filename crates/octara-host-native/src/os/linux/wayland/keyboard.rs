use super::Protocol;
use crate::os::{PlatformEvent, PlatformKey};
use octara::runtime::WindowId;
use octara_model::input::{Key, KeyEvent, Modifiers};
use std::ffi::CString;
use std::fs::File;
use std::os::fd::OwnedFd;
use std::os::unix::fs::FileExt;
use std::ptr::{self, NonNull};
use std::time::{Duration, Instant};
use wayland_client::backend::ObjectId;
use wayland_client::protocol::{wl_keyboard, wl_surface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use xkbcommon_dl as xkb;

#[derive(Default)]
pub(super) struct Keyboard {
    pub object: Option<wl_keyboard::WlKeyboard>,
    pub modifiers: Modifiers,
    pub focus: Option<(WindowId, wl_surface::WlSurface)>,
    xkb: Option<Keymap>,
    repeat_config: Option<RepeatConfig>,
    repeat: Option<(u32, Instant)>,
}

struct RepeatConfig {
    delay: Duration,
    interval: Duration,
}

impl Keyboard {
    pub(super) fn release(&mut self) {
        if let Some(object) = self.object.take()
            && object.version() >= 3
        {
            object.release();
        }
        *self = Self::default();
    }

    fn translate(&mut self, code: u32, repeat: bool) -> Option<PlatformEvent> {
        let window = self.focus.as_ref()?.0;
        let mut event = self.xkb.as_mut()?.key(code, self.modifiers, repeat);
        if self.modifiers.intersects(Modifiers::CTRL | Modifiers::META) {
            event.text = None;
        }
        (event.event.is_some() || event.text.is_some()).then_some(PlatformEvent::Key { window, event })
    }
}

impl Protocol {
    pub(super) fn repeat_deadline(&self) -> Option<Instant> {
        self.shared.seats.borrow().values().filter_map(|seat| seat.borrow().keyboard.repeat.map(|(_, at)| at)).min()
    }

    pub(super) fn repeat_keys(&mut self, now: Instant) {
        for seat in self.shared.seats.borrow().values() {
            let mut seat = seat.borrow_mut();
            let keyboard = &mut seat.keyboard;
            if let (Some((code, at)), Some(config)) = (keyboard.repeat, keyboard.repeat_config.as_ref())
                && at <= now
            {
                keyboard.repeat = Some((code, now + config.interval));
                if let Some(event) = keyboard.translate(code, true) {
                    self.shared.emit(event);
                }
            }
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ObjectId> for Protocol {
    fn event(
        state: &mut Self,
        object: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        seat: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wl_keyboard::Event::*;

        let Some(seat) = state.shared.seat(seat) else { return };
        let mut seat = seat.borrow_mut();
        let keyboard = &mut seat.keyboard;
        if keyboard.object.as_ref() != Some(object) {
            return;
        }
        match event {
            Keymap { format, fd, size } => {
                keyboard.repeat = None;
                keyboard.xkb = (format == WEnum::Value(wl_keyboard::KeymapFormat::XkbV1)).then(|| self::Keymap::new(fd, size)).flatten();
            }
            Enter { surface, .. } => {
                let focus = state.shared.window(&surface).map(|window| (window.id, surface));
                if keyboard.focus != focus {
                    keyboard.focus = focus;
                    seat.clipboard_serial = None;
                }
                seat.keyboard.repeat = None;
            }
            Leave { .. } => {
                keyboard.focus = None;
                keyboard.repeat = None;
                keyboard.modifiers = self::Modifiers::empty();
                if let Some(keymap) = &mut keyboard.xkb {
                    keymap.reset_compose();
                }
                seat.clipboard_serial = None;
            }
            Key { key, state: input, serial, .. } => match input {
                WEnum::Value(wl_keyboard::KeyState::Pressed) => {
                    if let Some(config) = &keyboard.repeat_config
                        && keyboard.xkb.as_ref().is_some_and(|keymap| keymap.repeats(key))
                    {
                        keyboard.repeat = Some((key, Instant::now() + config.delay));
                    }
                    if let Some(event) = keyboard.translate(key, false) {
                        state.shared.emit(event);
                    }
                    if let Some((_, surface)) = &keyboard.focus {
                        let surface = surface.clone();
                        seat.record_serial(surface, serial);
                    }
                }
                WEnum::Value(wl_keyboard::KeyState::Released) if keyboard.repeat.is_some_and(|(code, _)| code == key) => {
                    keyboard.repeat = None;
                }
                _ => {}
            },
            Modifiers { mods_depressed, mods_latched, mods_locked, group, .. } => {
                if let Some(keymap) = &mut keyboard.xkb {
                    keyboard.modifiers = keymap.modifiers(mods_depressed, mods_latched, mods_locked, group);
                }
            }
            RepeatInfo { rate, delay } => {
                keyboard.repeat_config = (rate > 0 && delay >= 0).then(|| RepeatConfig {
                    delay: Duration::from_millis(delay as u64),
                    interval: Duration::from_secs_f64(1.0 / f64::from(rate)).max(Duration::from_millis(1)),
                });
                keyboard.repeat = None;
            }
            _ => {}
        }
    }
}

struct Keymap {
    api: &'static xkb::XkbCommon,
    keymap: NonNull<xkb::xkb_keymap>,
    state: NonNull<xkb::xkb_state>,
    compose: Option<Compose>,
}

impl Keymap {
    fn new(fd: OwnedFd, size: u32) -> Option<Self> {
        let api = xkb::xkbcommon_option()?;
        // Keymaps are bounded before allocation and must contain exactly the advertised bytes.
        let size = usize::try_from(size).ok().filter(|size| *size <= 16 * 1024 * 1024)?;
        let mut buffer = vec![0; size];
        File::from(fd).read_exact_at(&mut buffer, 0).ok()?;
        let contents = std::ffi::CStr::from_bytes_with_nul(&buffer).ok()?;
        // SAFETY: libxkbcommon returns a new context or null. Each successful allocation
        // is released here or transferred into Keymap and Compose.
        unsafe {
            let context = NonNull::new((api.xkb_context_new)(xkb::xkb_context_flags::XKB_CONTEXT_NO_FLAGS))?;
            let keymap = NonNull::new((api.xkb_keymap_new_from_string)(
                context.as_ptr(),
                contents.as_ptr(),
                xkb::xkb_keymap_format::XKB_KEYMAP_FORMAT_TEXT_V1,
                xkb::xkb_keymap_compile_flags::XKB_KEYMAP_COMPILE_NO_FLAGS,
            ));
            let compose = keymap.and_then(|_| Compose::new(context));
            (api.xkb_context_unref)(context.as_ptr());
            let keymap = keymap?;
            let Some(state) = NonNull::new((api.xkb_state_new)(keymap.as_ptr())) else {
                (api.xkb_keymap_unref)(keymap.as_ptr());
                return None;
            };
            Some(Self { api, keymap, state, compose })
        }
    }

    fn reset_compose(&mut self) {
        if let Some(compose) = &mut self.compose {
            // SAFETY: the compose state is owned by this instance.
            unsafe { (compose.api.xkb_compose_state_reset)(compose.state.as_ptr()) };
        }
    }

    fn modifiers(&mut self, depressed: u32, latched: u32, locked: u32, group: u32) -> Modifiers {
        let mut modifiers = Modifiers::empty();
        let state = self.state.as_ptr();
        // SAFETY: the layout state is live and modifier names are NUL-terminated literals.
        unsafe {
            (self.api.xkb_state_update_mask)(state, depressed, latched, locked, 0, 0, group);
            for (flag, name) in
                [(Modifiers::SHIFT, c"Shift"), (Modifiers::CTRL, c"Control"), (Modifiers::ALT, c"Mod1"), (Modifiers::META, c"Mod4")]
            {
                let active =
                    (self.api.xkb_state_mod_name_is_active)(state, name.as_ptr(), xkb::xkb_state_component::XKB_STATE_MODS_EFFECTIVE) > 0;
                modifiers.set(flag, active);
            }
        }
        modifiers
    }

    fn repeats(&self, evdev: u32) -> bool {
        // SAFETY: the keymap is live for the duration of the call.
        unsafe { (self.api.xkb_keymap_key_repeats)(self.keymap.as_ptr(), evdev.saturating_add(8)) != 0 }
    }

    fn key(&mut self, evdev: u32, modifiers: Modifiers, repeat: bool) -> PlatformKey {
        let code = evdev.saturating_add(8);
        // SAFETY: state and keymap are live. libxkbcommon permits arbitrary keycodes.
        let (symbol, unicode) = unsafe {
            let symbol = (self.api.xkb_state_key_get_one_sym)(self.state.as_ptr(), code);
            (symbol, (self.api.xkb_keysym_to_utf32)(symbol))
        };
        let printable = char::from_u32(unicode).filter(|value| *value != '\0' && !value.is_control());
        let event = logical_key(symbol, printable).map(|key| KeyEvent { key, code: physical_character(evdev), modifiers, repeat });
        let text = if !modifiers.intersects(Modifiers::CTRL | Modifiers::META)
            && let Some(composed) = self.compose.as_mut().and_then(|compose| compose.feed(symbol))
        {
            composed
        } else {
            printable.map(|value| value.to_string())
        };
        PlatformKey { event, text }
    }
}

impl Drop for Keymap {
    fn drop(&mut self) {
        // SAFETY: both objects are owned here and each is released exactly once.
        unsafe {
            (self.api.xkb_state_unref)(self.state.as_ptr());
            (self.api.xkb_keymap_unref)(self.keymap.as_ptr());
        }
    }
}

struct Compose {
    api: &'static xkb::XkbCommonCompose,
    state: NonNull<xkb::xkb_compose_state>,
}

impl Compose {
    fn new(context: NonNull<xkb::xkb_context>) -> Option<Self> {
        let api = xkb::xkbcommon_compose_option()?;
        let locale =
            ["LC_ALL", "LC_CTYPE", "LANG"].into_iter().find_map(|name| std::env::var(name).ok().filter(|locale| !locale.is_empty()));
        let locale = CString::new(locale.unwrap_or_else(|| "C.UTF-8".into())).ok()?;
        // SAFETY: the context is live, the locale is NUL-terminated, and the table is
        // released after creation of the state, which retains its own reference.
        unsafe {
            let table = NonNull::new((api.xkb_compose_table_new_from_locale)(
                context.as_ptr(),
                locale.as_ptr(),
                xkb::xkb_compose_compile_flags::XKB_COMPOSE_COMPILE_NO_FLAGS,
            ))?;
            let state = (api.xkb_compose_state_new)(table.as_ptr(), xkb::xkb_compose_state_flags::XKB_COMPOSE_STATE_NO_FLAGS);
            (api.xkb_compose_table_unref)(table.as_ptr());
            Some(Self { api, state: NonNull::new(state)? })
        }
    }

    /// An outer None uses the key's normal text. An inner None waits for more compose keys.
    fn feed(&mut self, symbol: u32) -> Option<Option<String>> {
        use xkb::xkb_compose_status::*;

        let state = self.state.as_ptr();
        // SAFETY: the state is owned here. Both UTF-8 calls obey the library's buffer-size
        // contract, first querying the extent and then supplying writable storage and a NUL byte.
        unsafe {
            (self.api.xkb_compose_state_feed)(state, symbol);
            match (self.api.xkb_compose_state_get_status)(state) {
                XKB_COMPOSE_NOTHING => None,
                XKB_COMPOSE_COMPOSING => Some(None),
                XKB_COMPOSE_CANCELLED => {
                    (self.api.xkb_compose_state_reset)(state);
                    None
                }
                XKB_COMPOSE_COMPOSED => {
                    let size = (self.api.xkb_compose_state_get_utf8)(state, ptr::null_mut(), 0);
                    let mut bytes = vec![0; usize::try_from(size).ok()?.checked_add(1)?];
                    let written = (self.api.xkb_compose_state_get_utf8)(state, bytes.as_mut_ptr().cast(), bytes.len());
                    (self.api.xkb_compose_state_reset)(state);
                    let written = usize::try_from(written).ok().filter(|written| *written < bytes.len())?;
                    bytes.truncate(written);
                    Some(String::from_utf8(bytes).ok())
                }
            }
        }
    }
}

impl Drop for Compose {
    fn drop(&mut self) {
        // SAFETY: this is the compose state's last and only owning Rust handle.
        unsafe { (self.api.xkb_compose_state_unref)(self.state.as_ptr()) };
    }
}

#[allow(non_upper_case_globals)]
fn logical_key(symbol: u32, printable: Option<char>) -> Option<Key> {
    use xkb::keysyms::*;

    Some(match symbol {
        Tab | ISO_Left_Tab | KP_Tab => Key::Tab,
        Return | KP_Enter => Key::Enter,
        Escape => Key::Escape,
        BackSpace => Key::Backspace,
        Delete | KP_Delete => Key::Delete,
        space | KP_Space => Key::Space,
        Up | KP_Up => Key::Up,
        Down | KP_Down => Key::Down,
        Left | KP_Left => Key::Left,
        Right | KP_Right => Key::Right,
        Home | KP_Home => Key::Home,
        End | KP_End => Key::End,
        Page_Up | KP_Page_Up => Key::PageUp,
        Page_Down | KP_Page_Down => Key::PageDown,
        Menu => Key::Menu,
        F1..=F24 => Key::F((symbol - F1 + 1) as u8),
        _ => Key::Char(printable?.to_ascii_lowercase()),
    })
}

fn physical_character(evdev: u32) -> Option<char> {
    let (keys, index): (&[u8], u32) = match evdev {
        2..=13 => (b"1234567890-=", evdev - 2),
        16..=27 => (b"qwertyuiop[]", evdev - 16),
        30..=41 => (b"asdfghjkl;'\x60", evdev - 30),
        43 => return Some('\\'),
        44..=53 => (b"zxcvbnm,./", evdev - 44),
        _ => return None,
    };
    Some(char::from(keys[index as usize]))
}

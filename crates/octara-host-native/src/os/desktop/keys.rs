use crate::os::PlatformKey;
use octara_model::input::{Key, KeyEvent, Modifiers};
use winit::event::KeyEvent as WinitKeyEvent;
use winit::keyboard::{Key as NativeKey, KeyCode, NamedKey, PhysicalKey};

pub(super) fn key(event: &WinitKeyEvent, modifiers: Modifiers) -> PlatformKey {
    let text = event.text.as_deref().filter(|_| !modifiers.intersects(Modifiers::CTRL | Modifiers::META)).map(str::to_owned);
    PlatformKey { event: translate_key(event, modifiers), text }
}

fn translate_key(event: &WinitKeyEvent, modifiers: Modifiers) -> Option<KeyEvent> {
    let key = match event.logical_key.as_ref() {
        NativeKey::Character(text) => Key::Char(text.chars().next()?),
        NativeKey::Named(name) => match name {
            NamedKey::Tab => Key::Tab,
            NamedKey::Enter => Key::Enter,
            NamedKey::Escape => Key::Escape,
            NamedKey::Backspace => Key::Backspace,
            NamedKey::Delete => Key::Delete,
            NamedKey::Space => Key::Space,
            NamedKey::ArrowUp => Key::Up,
            NamedKey::ArrowDown => Key::Down,
            NamedKey::ArrowLeft => Key::Left,
            NamedKey::ArrowRight => Key::Right,
            NamedKey::Home => Key::Home,
            NamedKey::End => Key::End,
            NamedKey::PageUp => Key::PageUp,
            NamedKey::PageDown => Key::PageDown,
            NamedKey::ContextMenu => Key::Menu,
            _ => Key::F(function_key(name)?),
        },
        _ => return None,
    };
    let mut key = KeyEvent::new(key, modifiers);
    key.repeat = event.repeat;
    key.code = physical_character(event.physical_key);
    Some(key)
}

fn function_key(key: NamedKey) -> Option<u8> {
    const KEYS: [NamedKey; 24] = [
        NamedKey::F1,
        NamedKey::F2,
        NamedKey::F3,
        NamedKey::F4,
        NamedKey::F5,
        NamedKey::F6,
        NamedKey::F7,
        NamedKey::F8,
        NamedKey::F9,
        NamedKey::F10,
        NamedKey::F11,
        NamedKey::F12,
        NamedKey::F13,
        NamedKey::F14,
        NamedKey::F15,
        NamedKey::F16,
        NamedKey::F17,
        NamedKey::F18,
        NamedKey::F19,
        NamedKey::F20,
        NamedKey::F21,
        NamedKey::F22,
        NamedKey::F23,
        NamedKey::F24,
    ];
    KEYS.iter().position(|&candidate| candidate == key).map(|index| index as u8 + 1)
}

fn physical_character(key: PhysicalKey) -> Option<char> {
    let PhysicalKey::Code(key) = key else { return None };
    const LETTERS: [KeyCode; 26] = [
        KeyCode::KeyA,
        KeyCode::KeyB,
        KeyCode::KeyC,
        KeyCode::KeyD,
        KeyCode::KeyE,
        KeyCode::KeyF,
        KeyCode::KeyG,
        KeyCode::KeyH,
        KeyCode::KeyI,
        KeyCode::KeyJ,
        KeyCode::KeyK,
        KeyCode::KeyL,
        KeyCode::KeyM,
        KeyCode::KeyN,
        KeyCode::KeyO,
        KeyCode::KeyP,
        KeyCode::KeyQ,
        KeyCode::KeyR,
        KeyCode::KeyS,
        KeyCode::KeyT,
        KeyCode::KeyU,
        KeyCode::KeyV,
        KeyCode::KeyW,
        KeyCode::KeyX,
        KeyCode::KeyY,
        KeyCode::KeyZ,
    ];
    const DIGITS: [KeyCode; 10] = [
        KeyCode::Digit0,
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
    ];
    if let Some(index) = LETTERS.iter().position(|&candidate| candidate == key) {
        return Some(char::from(b'a' + index as u8));
    }
    if let Some(index) = DIGITS.iter().position(|&candidate| candidate == key) {
        return Some(char::from(b'0' + index as u8));
    }
    Some(match key {
        KeyCode::Backquote => '`',
        KeyCode::Backslash => '\\',
        KeyCode::BracketLeft => '[',
        KeyCode::BracketRight => ']',
        KeyCode::Comma => ',',
        KeyCode::Equal => '=',
        KeyCode::Minus => '-',
        KeyCode::Period => '.',
        KeyCode::Quote => '\'',
        KeyCode::Semicolon => ';',
        KeyCode::Slash => '/',
        _ => return None,
    })
}

//! Normalization of AppKit input for the native host.

use super::super::PlatformKey;
use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSCursor, NSCursorFrameResizeDirections, NSCursorFrameResizePosition, NSEvent, NSEventModifierFlags, NSEventPhase, NSImage,
};
use objc2_foundation::{NSPoint, NSSize};
use octara_model::geometry::{Point, Vec2};
use octara_model::input::{CursorStyle, GestureEvent, GesturePhase, InteractionEvent, Key, KeyEvent, Modifiers, WheelPhase, WheelStage};

pub(super) fn modifiers(flags: NSEventModifierFlags) -> Modifiers {
    let mut result = Modifiers::empty();
    result.set(Modifiers::SHIFT, flags.contains(NSEventModifierFlags::Shift));
    result.set(Modifiers::CTRL, flags.contains(NSEventModifierFlags::Control));
    result.set(Modifiers::ALT, flags.contains(NSEventModifierFlags::Option));
    result.set(Modifiers::META, flags.contains(NSEventModifierFlags::Command));
    result
}

pub(super) fn key(event: &NSEvent) -> PlatformKey {
    let flags = event.modifierFlags();
    // Interpret shortcuts in the active layout without the control characters
    // that AppKit supplies in the printable text of Command and Control chords.
    let command_modifiers = NSEventModifierFlags::Control | NSEventModifierFlags::Command;
    let characters = event.characters().map(|text| text.to_string());
    let logical = if flags.intersects(command_modifiers) {
        event
            .charactersByApplyingModifiers(flags - command_modifiers)
            .or_else(|| event.charactersIgnoringModifiers())
            .map(|logical| logical.to_string())
    } else {
        characters.clone()
    };
    let event_key = logical.and_then(|logical| {
        let key = native_key(logical.chars().next()?)?;
        Some(KeyEvent { key, code: physical_character(event.keyCode()), modifiers: modifiers(flags), repeat: event.isARepeat() })
    });
    let text = characters.filter(|text| {
        !text.is_empty() && text.chars().all(|character| !character.is_control() && !matches!(character, '\u{F700}'..='\u{F8FF}'))
    });
    PlatformKey { event: event_key, text }
}

fn native_key(character: char) -> Option<Key> {
    const F1: char = '\u{F704}';
    Some(match character {
        '\t' | '\u{19}' => Key::Tab,
        '\r' | '\n' | '\u{3}' => Key::Enter,
        '\u{1B}' => Key::Escape,
        '\u{8}' | '\u{7F}' => Key::Backspace,
        ' ' => Key::Space,
        '\u{F700}' => Key::Up,
        '\u{F701}' => Key::Down,
        '\u{F702}' => Key::Left,
        '\u{F703}' => Key::Right,
        F1..='\u{F71B}' => Key::F((character as u32 - F1 as u32 + 1) as u8),
        '\u{F728}' => Key::Delete,
        '\u{F729}' => Key::Home,
        '\u{F72B}' => Key::End,
        '\u{F72C}' => Key::PageUp,
        '\u{F72D}' => Key::PageDown,
        '\u{F735}' => Key::Menu,
        '\u{F700}'..='\u{F8FF}' => return None,
        c if c.is_control() => return None,
        c => Key::Char(c),
    })
}

fn physical_character(code: u16) -> Option<char> {
    // AppKit virtual key codes index the physical US ANSI positions. Zero bytes
    // represent non-printable positions in the contiguous range through kVK_ANSI_Grave.
    const ANSI: &[u8] = b"asdfhgzxcv\0bqwerty123465=97-80]ou[ip\0lj'k;\\,/nm.\0\0`";
    let character = *ANSI.get(usize::from(code))?;
    (character != 0).then_some(char::from(character))
}

pub(super) fn wheel(event: &NSEvent, pos: Point) -> InteractionEvent {
    let multiplier = if event.hasPreciseScrollingDeltas() { -1.0 } else { -32.0 };
    let delta = Vec2::new(event.scrollingDeltaX() as f32 * multiplier, event.scrollingDeltaY() as f32 * multiplier);
    let phase = wheel_phase(event.phase(), event.momentumPhase());
    InteractionEvent::Wheel { pos, delta, phase, mods: modifiers(event.modifierFlags()) }
}

fn wheel_phase(direct: NSEventPhase, momentum: NSEventPhase) -> WheelPhase {
    if let Some(stage) = wheel_stage(momentum) {
        WheelPhase::Momentum(stage)
    } else if let Some(stage) = wheel_stage(direct) {
        WheelPhase::Direct(stage)
    } else {
        WheelPhase::Discrete
    }
}

fn wheel_stage(phase: NSEventPhase) -> Option<WheelStage> {
    Some(match phase {
        NSEventPhase::None => return None,
        NSEventPhase::MayBegin | NSEventPhase::Began => WheelStage::Start,
        NSEventPhase::Ended => WheelStage::End,
        NSEventPhase::Cancelled => WheelStage::Cancel,
        _ => WheelStage::Move,
    })
}

#[derive(Default)]
pub(super) struct GestureState {
    scale: Option<f32>,
    rotation: Option<f32>,
}

impl GestureState {
    pub(super) fn pinch(&mut self, event: &NSEvent, anchor: Point) -> Option<GestureEvent> {
        let phase = gesture_phase(event.phase())?;
        let (previous, scale) = accumulate(&mut self.scale, phase, 1.0, |scale| {
            let next = scale * (1.0 + event.magnification() as f32);
            (next.is_finite() && next > 0.0).then_some(next)
        })?;
        let modifiers = modifiers(event.modifierFlags());
        Some(GestureEvent::Pinch { phase, scale, delta_scale: scale / previous, anchor, modifiers })
    }

    pub(super) fn rotate(&mut self, event: &NSEvent, anchor: Point) -> Option<GestureEvent> {
        let phase = gesture_phase(event.phase())?;
        let (previous, radians) = accumulate(&mut self.rotation, phase, 0.0, |radians| {
            let next = radians + event.rotation().to_radians();
            next.is_finite().then_some(next)
        })?;
        let modifiers = modifiers(event.modifierFlags());
        Some(GestureEvent::Rotate { phase, radians, delta_radians: radians - previous, anchor, modifiers })
    }
}

fn gesture_phase(phase: NSEventPhase) -> Option<GesturePhase> {
    Some(match phase {
        NSEventPhase::Began => GesturePhase::Started,
        NSEventPhase::Changed => GesturePhase::Changed,
        NSEventPhase::Ended => GesturePhase::Ended,
        NSEventPhase::Cancelled => GesturePhase::Cancelled,
        _ => return None,
    })
}

fn accumulate(state: &mut Option<f32>, phase: GesturePhase, identity: f32, update: impl FnOnce(f32) -> Option<f32>) -> Option<(f32, f32)> {
    let previous = if phase == GesturePhase::Started { identity } else { (*state)? };
    let value = if phase == GesturePhase::Cancelled { previous } else { update(previous).unwrap_or(previous) };
    *state = matches!(phase, GesturePhase::Started | GesturePhase::Changed).then_some(value);
    Some((previous, value))
}

pub(super) fn cursor(style: CursorStyle) -> Retained<NSCursor> {
    use CursorStyle::*;
    use NSCursorFrameResizePosition as Position;
    let resize = |at| NSCursor::frameResizeCursorFromPosition_inDirections(at, NSCursorFrameResizeDirections::All);
    match style {
        Arrow | Auto | Wait | Progress | Help => NSCursor::arrowCursor(),
        IBeam => NSCursor::IBeamCursor(),
        VerticalText => NSCursor::IBeamCursorForVerticalLayout(),
        PointingHand => NSCursor::pointingHandCursor(),
        Crosshair | Cell => NSCursor::crosshairCursor(),
        OpenHand | AllScroll => NSCursor::openHandCursor(),
        ClosedHand | Move => NSCursor::closedHandCursor(),
        ResizeLeftRight | ResizeColumn => NSCursor::columnResizeCursor(),
        ResizeUpDown | ResizeRow => NSCursor::rowResizeCursor(),
        ResizeNorth => resize(Position::Top),
        ResizeSouth => resize(Position::Bottom),
        ResizeWest => resize(Position::Left),
        ResizeEast => resize(Position::Right),
        ResizeNorthWest | ResizeNorthWestSouthEast => resize(Position::TopLeft),
        ResizeNorthEast | ResizeNorthEastSouthWest => resize(Position::TopRight),
        ResizeSouthWest => resize(Position::BottomLeft),
        ResizeSouthEast => resize(Position::BottomRight),
        OperationNotAllowed | NoDrop => NSCursor::operationNotAllowedCursor(),
        ContextMenu => NSCursor::contextualMenuCursor(),
        Alias => NSCursor::dragLinkCursor(),
        Copy => NSCursor::dragCopyCursor(),
        ZoomIn => NSCursor::zoomInCursor(),
        ZoomOut => NSCursor::zoomOutCursor(),
        None => NSCursor::initWithImage_hotSpot(
            NSCursor::alloc(),
            &NSImage::initWithSize(NSImage::alloc(), NSSize::new(1.0, 1.0)),
            NSPoint::ZERO,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_both_appkit_wheel_lifecycles() {
        let none = NSEventPhase::None;
        for (native, stage) in [
            (NSEventPhase::MayBegin, WheelStage::Start),
            (NSEventPhase::Began, WheelStage::Start),
            (NSEventPhase::Changed, WheelStage::Move),
            (NSEventPhase::Stationary, WheelStage::Move),
            (NSEventPhase::Ended, WheelStage::End),
            (NSEventPhase::Cancelled, WheelStage::Cancel),
        ] {
            assert_eq!(wheel_phase(native, none), WheelPhase::Direct(stage));
            assert_eq!(wheel_phase(none, native), WheelPhase::Momentum(stage));
        }
        assert_eq!(wheel_phase(NSEventPhase::Ended, NSEventPhase::Began), WheelPhase::Momentum(WheelStage::Start));
        assert_eq!(wheel_phase(none, none), WheelPhase::Discrete);
    }

    #[test]
    fn trackpad_gesture_resets_after_cancel() {
        let mut state = None;
        assert_eq!(accumulate(&mut state, GesturePhase::Started, 1.0, |v| Some(v * 1.5)), Some((1.0, 1.5)));
        assert_eq!(accumulate(&mut state, GesturePhase::Changed, 1.0, |v| Some(v * 0.5)), Some((1.5, 0.75)));
        assert_eq!(accumulate(&mut state, GesturePhase::Cancelled, 1.0, |_| unreachable!()), Some((0.75, 0.75)));
        assert_eq!(accumulate(&mut state, GesturePhase::Changed, 1.0, |_| unreachable!()), None);
    }
}

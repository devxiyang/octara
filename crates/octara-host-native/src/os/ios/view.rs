//! Native UIKit objects that own the GPU surface and route responder events.

use super::{
    app,
    editor::{ContentView, EditorState},
    scene,
};
use crate::os::{PlatformEvent, PlatformKey, PlatformSender, PointerPhase};
use objc2::rc::Retained;
use objc2::runtime::NSObjectProtocol;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_foundation::{NSArray, NSRect, NSSet};
use objc2_ui_kit::{
    UIEvent, UIEventButtonMask, UIKey, UIKeyModifierFlags, UIPress, UIPressPhase, UIPressesEvent, UIStatusBarStyle, UITouch, UITouchType,
    UITraitCollection, UIViewController, UIWindow, UIWindowScene,
};
use octara::runtime::WindowId;
use octara_model::geometry::Point;
use octara_model::input::{Button, Key, KeyEvent, Modifiers, PointerId, PointerKind};
use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;

pub(super) struct ViewState {
    pub(super) id: WindowId,
    pub(super) sender: PlatformSender,
    pub(super) editor: EditorState,
    pub(super) accessibility_elements: RefCell<Option<Retained<NSArray>>>,
    contacts: RefCell<HashMap<PointerId, Button>>,
    extent: Cell<Option<([u32; 2], f32)>>,
}

impl ContentView {
    pub(super) fn new(main: MainThreadMarker, frame: NSRect, id: WindowId, sender: PlatformSender) -> Retained<Self> {
        let state = ViewState {
            id,
            sender,
            editor: EditorState::default(),
            accessibility_elements: RefCell::default(),
            contacts: RefCell::default(),
            extent: Cell::default(),
        };
        // SAFETY: Initialize the UIView superclass with its frame on the UIKit thread.
        let view: Retained<Self> = unsafe { msg_send![super(main.alloc::<Self>().set_ivars(state)), initWithFrame: frame] };
        view.setMultipleTouchEnabled(true);
        view
    }

    pub(super) fn publish_geometry(&self) {
        let (size, scale) = (self.physical_size(), self.contentScaleFactor() as f32);
        let state = self.ivars();
        if !size.contains(&0) && state.extent.replace(Some((size, scale))) != Some((size, scale)) {
            state.sender.send_event(PlatformEvent::WindowResized(state.id));
        }
        self.refresh_viewport();
    }

    pub(super) fn refresh_viewport(&self) {
        let state = self.ivars();
        state.sender.send_event(PlatformEvent::RefreshViewport { window: state.id });
    }

    pub(super) fn physical_size(&self) -> [u32; 2] {
        let size = self.bounds().size;
        let scale = self.contentScaleFactor();
        [(size.width * scale).round() as u32, (size.height * scale).round() as u32]
    }

    pub(super) fn touches(&self, touches: &NSSet<UITouch>, event: Option<&UIEvent>, phase: PointerPhase) {
        let modifiers = event.map_or_else(Modifiers::empty, |event| keyboard_modifiers(event.modifierFlags()));
        let buttons = event.map_or_else(UIEventButtonMask::empty, UIEvent::buttonMask);
        let state = self.ivars();
        for touch in touches {
            let kind = match touch.r#type() {
                UITouchType::Pencil => PointerKind::Pen,
                UITouchType::IndirectPointer => PointerKind::Mouse,
                _ => PointerKind::Touch,
            };
            let id = PointerId(if kind == PointerKind::Mouse { 0 } else { Retained::as_ptr(&touch) as usize as u64 });
            let button = if matches!(phase, PointerPhase::Started) {
                let button = if buttons.contains(UIEventButtonMask::Secondary) {
                    Button::Secondary
                } else if buttons.contains(UIEventButtonMask::for_button_number(3)) {
                    Button::Middle
                } else {
                    Button::Primary
                };
                state.contacts.borrow_mut().insert(id, button);
                button
            } else if matches!(phase, PointerPhase::Ended | PointerPhase::Cancelled) {
                state.contacts.borrow_mut().remove(&id).unwrap_or_default()
            } else {
                state.contacts.borrow().get(&id).copied().unwrap_or_default()
            };
            let point = touch.locationInView(Some(self));
            state.sender.send_event(PlatformEvent::Pointer {
                window: state.id,
                phase,
                position: Some(Point::new(point.x as f32, point.y as f32)),
                id,
                kind,
                button,
                modifiers,
            });
        }
    }
}

pub(super) struct ControllerState {
    style: Cell<UIStatusBarStyle>,
    hidden: Cell<bool>,
    view: Retained<ContentView>,
}

define_class!(
    #[unsafe(super(UIViewController))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraViewController"]
    #[ivars = ControllerState]
    pub(super) struct Controller;

    unsafe impl NSObjectProtocol for Controller {}

    impl Controller {
        #[unsafe(method(preferredStatusBarStyle))]
        fn status_style(&self) -> UIStatusBarStyle {
            self.ivars().style.get()
        }

        #[unsafe(method(prefersStatusBarHidden))]
        fn status_hidden(&self) -> bool {
            self.ivars().hidden.get()
        }

        #[unsafe(method(viewSafeAreaInsetsDidChange))]
        fn safe_area_changed(&self) {
            // SAFETY: Call the inherited controller lifecycle method.
            let _: () = unsafe { msg_send![super(self), viewSafeAreaInsetsDidChange] };
            self.ivars().view.refresh_viewport();
        }

        #[unsafe(method(traitCollectionDidChange:))]
        fn traits_changed(&self, previous: Option<&UITraitCollection>) {
            // SAFETY: Call the inherited controller lifecycle method.
            let _: () = unsafe { msg_send![super(self), traitCollectionDidChange: previous] };
            self.ivars().view.ivars().sender.send_event(PlatformEvent::RefreshSystemPreferences);
        }
    }
);

impl Controller {
    pub(super) fn new(view: Retained<ContentView>, style: UIStatusBarStyle, hidden: bool) -> Retained<Self> {
        let main = view.mtm();
        let state = ControllerState { style: Cell::new(style), hidden: Cell::new(hidden), view };
        // SAFETY: Initialize UIKit's view controller with its programmatically created view.
        let controller: Retained<Self> = unsafe {
            msg_send![super(main.alloc::<Self>().set_ivars(state)), initWithNibName: None::<&objc2_foundation::NSString>, bundle: None::<&objc2_foundation::NSBundle>]
        };
        controller.setView(Some(&controller.ivars().view));
        controller
    }

    pub(super) fn set_status(&self, style: UIStatusBarStyle, hidden: bool) {
        let state = self.ivars();
        let style_changed = state.style.replace(style) != style;
        let visibility_changed = state.hidden.replace(hidden) != hidden;
        if style_changed || visibility_changed {
            self.setNeedsStatusBarAppearanceUpdate();
        }
    }
}

pub(super) struct WindowState {
    id: WindowId,
    sender: PlatformSender,
    shown: Cell<bool>,
    focused: Cell<bool>,
    pressed: RefCell<HashMap<isize, bool>>,
    native_key: Cell<bool>,
    forwarding_press: Cell<bool>,
}

define_class!(
    #[unsafe(super(UIWindow))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraWindow"]
    #[ivars = WindowState]
    pub(super) struct NativeWindow;

    unsafe impl NSObjectProtocol for NativeWindow {}

    impl NativeWindow {
        #[unsafe(method(canBecomeFirstResponder))]
        fn can_handle_keys(&self) -> bool {
            true
        }

        #[unsafe(method(becomeKeyWindow))]
        fn become_key(&self) {
            // SAFETY: This is UIWindow's key-window callback.
            let _: () = unsafe { msg_send![super(self), becomeKeyWindow] };
            self.update_focus();
        }

        #[unsafe(method(resignKeyWindow))]
        fn resign_key(&self) {
            // SAFETY: This is UIWindow's key-window callback.
            let _: () = unsafe { msg_send![super(self), resignKeyWindow] };
            self.set_focused(false);
        }

        #[unsafe(method(pressesBegan:withEvent:))]
        fn presses_began(&self, presses: &NSSet<UIPress>, event: Option<&UIPressesEvent>) {
            self.route_presses(presses, UIPressPhase::Began, |presses| {
                // SAFETY: Let UIKit dispatch unhandled presses on its responder chain.
                unsafe { msg_send![super(self), pressesBegan: presses, withEvent: event] }
            });
        }

        #[unsafe(method(pressesEnded:withEvent:))]
        fn presses_ended(&self, presses: &NSSet<UIPress>, event: Option<&UIPressesEvent>) {
            self.route_presses(presses, UIPressPhase::Ended, |presses| {
                // SAFETY: Match the begin event with UIKit's corresponding release.
                unsafe { msg_send![super(self), pressesEnded: presses, withEvent: event] }
            });
        }

        #[unsafe(method(pressesCancelled:withEvent:))]
        fn presses_cancelled(&self, presses: &NSSet<UIPress>, event: Option<&UIPressesEvent>) {
            self.route_presses(presses, UIPressPhase::Cancelled, |presses| {
                // SAFETY: Match the begin event with UIKit's corresponding cancellation.
                unsafe { msg_send![super(self), pressesCancelled: presses, withEvent: event] }
            });
        }
    }
);

impl NativeWindow {
    pub(super) fn new(
        main: MainThreadMarker,
        id: WindowId,
        sender: PlatformSender,
        scene: Option<&UIWindowScene>,
        frame: NSRect,
    ) -> Retained<Self> {
        let state = WindowState {
            id,
            sender,
            shown: Cell::new(false),
            focused: Cell::new(false),
            pressed: RefCell::default(),
            native_key: Cell::default(),
            forwarding_press: Cell::default(),
        };
        let window = main.alloc::<Self>().set_ivars(state);
        // SAFETY: UIKit supplies either the owning scene or a legacy window frame on the main thread.
        unsafe {
            if let Some(scene) = scene {
                msg_send![super(window), initWithWindowScene: scene]
            } else {
                msg_send![super(window), initWithFrame: frame]
            }
        }
    }

    pub(super) fn attach(&self, scene: &UIWindowScene) {
        self.setWindowScene(Some(scene));
        if self.ivars().shown.get() {
            self.makeKeyAndVisible();
        }
    }

    pub(super) fn show(&self) {
        self.ivars().shown.set(true);
        self.makeKeyAndVisible();
        self.becomeFirstResponder();
        self.update_focus();
    }

    pub(super) fn hide(&self) {
        self.ivars().shown.set(false);
        self.setHidden(true);
        self.set_focused(false);
    }

    pub(super) fn focused(&self) -> bool {
        self.ivars().focused.get()
    }

    pub(super) fn update_focus(&self) {
        self.set_focused(scene::active() && self.isKeyWindow());
    }

    fn set_focused(&self, active: bool) {
        let state = self.ivars();
        if state.focused.replace(active) != active {
            if !active {
                state.pressed.borrow_mut().clear();
            }
            state.sender.send_event(PlatformEvent::WindowFocused { window: state.id, active });
            state.sender.send_event(PlatformEvent::RefreshViewport { window: state.id });
        }
    }

    pub(super) fn forward_key_to_uikit(&self) {
        self.ivars().native_key.set(true);
    }

    pub(super) fn route_presses(&self, presses: &NSSet<UIPress>, phase: UIPressPhase, forward: impl FnOnce(&NSSet<UIPress>)) {
        let forwarding = &self.ivars().forwarding_press;
        if forwarding.get() {
            forward(presses);
            return;
        }
        let unhandled: Vec<_> = presses.iter().filter(|press| !self.intercept_press(press, phase)).collect();
        if !unhandled.is_empty() {
            forwarding.set(true);
            let _guard = std::mem::DropGuard::new((), |()| forwarding.set(false));
            if unhandled.len() == presses.count() {
                forward(presses);
            } else {
                forward(&NSSet::from_retained_slice(&unhandled));
            }
        }
    }

    fn intercept_press(&self, press: &UIPress, phase: UIPressPhase) -> bool {
        let Some(key) = press.key(self.mtm()) else { return false };
        let state = self.ivars();
        let usage = key.keyCode().0;
        match phase {
            UIPressPhase::Began => {
                let repeat = state.pressed.borrow().contains_key(&usage);
                state.native_key.set(false);
                let handled = key_event(&key, repeat).is_some_and(|event| app::send_key(PlatformEvent::Key { window: state.id, event }));
                let handled = handled && !state.native_key.replace(false);
                state.pressed.borrow_mut().insert(usage, handled);
                handled
            }
            UIPressPhase::Ended | UIPressPhase::Cancelled => state.pressed.borrow_mut().remove(&usage).unwrap_or(false),
            _ => false,
        }
    }
}

pub(super) fn keyboard_modifiers(flags: UIKeyModifierFlags) -> Modifiers {
    let mut modifiers = Modifiers::empty();
    modifiers.set(Modifiers::SHIFT, flags.contains(UIKeyModifierFlags::Shift));
    modifiers.set(Modifiers::CTRL, flags.contains(UIKeyModifierFlags::Control));
    modifiers.set(Modifiers::ALT, flags.contains(UIKeyModifierFlags::Alternate));
    modifiers.set(Modifiers::META, flags.contains(UIKeyModifierFlags::Command));
    modifiers
}

fn key_event(key: &UIKey, repeat: bool) -> Option<PlatformKey> {
    let usage = key.keyCode().0;
    let characters = key.characters().to_string();
    let logical = match usage {
        0x28 | 0x58 => Key::Enter,
        0x29 => Key::Escape,
        0x2a => Key::Backspace,
        0x2b => Key::Tab,
        0x2c => Key::Space,
        0x3a..=0x45 => Key::F((usage - 0x3a + 1) as u8),
        0x4a => Key::Home,
        0x4b => Key::PageUp,
        0x4c => Key::Delete,
        0x4d => Key::End,
        0x4e => Key::PageDown,
        0x4f => Key::Right,
        0x50 => Key::Left,
        0x51 => Key::Down,
        0x52 => Key::Up,
        0x65 | 0x76 => Key::Menu,
        0x68..=0x73 => Key::F((usage - 0x68 + 13) as u8),
        _ => {
            let character = key.charactersIgnoringModifiers().to_string().chars().next()?;
            if character.is_control() || (0xe000..=0xf8ff).contains(&(character as u32)) {
                return None;
            }
            Key::Char(character.to_ascii_lowercase())
        }
    };
    let code = match usage {
        0x04..=0x1d => char::from_u32(u32::from(b'a') + (usage - 0x04) as u32),
        0x1e..=0x27 => "1234567890".chars().nth((usage - 0x1e) as usize),
        0x2d..=0x38 => "-=[]\\#;'\x60,./".chars().nth((usage - 0x2d) as usize),
        _ => None,
    };
    let modifiers = keyboard_modifiers(key.modifierFlags());
    let text = (!characters.is_empty() && characters.chars().all(|c| !c.is_control() && !(0xe000..=0xf8ff).contains(&(c as u32))))
        .then_some(characters);
    Some(PlatformKey { event: Some(KeyEvent { key: logical, code, modifiers, repeat }), text })
}

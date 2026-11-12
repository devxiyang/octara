//! The app's menu bar on macOS: the app's menu (About, Hide, Quit), the app's menus
//! ([`octara::menu::set_menus`]) and the Window menu. An item of the app's runs its command in
//! the active window, enabled as the app says; picked by its keys, the keys go to the app's keymap
//! first (AppKit offers a key to the menu bar before any view). The bar is built again only when
//! its shape changes; a state change is what `validateMenuItem:` answers.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSEventType, NSMenu, NSMenuItem};
use objc2_foundation::{NSProcessInfo, NSString};
use octara::menu::{Menu, MenuItem};
use std::cell::RefCell;

/// One of the app's items: its command, its keys, whether it is enabled now.
struct Entry {
    command: String,
    chord: Option<String>,
    enabled: bool,
}

pub(crate) struct Bar {
    entries: RefCell<Vec<Entry>>,
    send: Box<dyn Fn(String, Option<String>)>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraMenuBar"]
    #[ivars = Bar]
    struct Target;

    impl Target {
        #[unsafe(method(run:))]
        fn run(&self, item: &NSMenuItem) {
            let entries = self.ivars().entries.borrow();
            let Some(e) = entries.get(item.tag() as usize) else { return };
            // Its keys picked it: they go through the keymap first.
            let chord = by_key().then(|| e.chord.clone()).flatten();
            (self.ivars().send)(e.command.clone(), chord);
        }

        #[unsafe(method(validateMenuItem:))]
        fn validate(&self, item: &NSMenuItem) -> bool {
            // While a key is matched, every item takes it, so a disabled one does not swallow it:
            // the app decides (its keymap, else the command, which does nothing then).
            by_key() || self.ivars().entries.borrow().get(item.tag() as usize).is_some_and(|e| e.enabled)
        }
    }
);

/// Whether the event being handled is a key press (a key equivalent being matched).
fn by_key() -> bool {
    MainThreadMarker::new()
        .is_some_and(|mtm| NSApplication::sharedApplication(mtm).currentEvent().is_some_and(|e| e.r#type() == NSEventType::KeyDown))
}

/// The menu bar the shell keeps.
pub(crate) struct MenuBar {
    target: Retained<Target>,
    /// The menus as built (their shape: enabled states aside).
    shape: Vec<Menu>,
}

impl MenuBar {
    /// A bar whose picks go to `send` (a command and, picked by its keys, the chord).
    pub(crate) fn new(send: impl Fn(String, Option<String>) + 'static) -> Option<MenuBar> {
        let mtm = MainThreadMarker::new()?;
        let target = Target::alloc(mtm).set_ivars(Bar { entries: RefCell::default(), send: Box::new(send) });
        let target: Retained<Target> = unsafe { msg_send![super(target), init] };
        Some(MenuBar { target, shape: Vec::new() })
    }

    /// Shows `menus`: built again when their shape changed, else their states updated.
    pub(crate) fn set(&mut self, menus: &[Menu]) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let shape = shape_of(menus);
        let mut entries = Vec::new();
        for m in menus {
            collect(m, &mut entries);
        }
        if shape == self.shape {
            *self.target.ivars().entries.borrow_mut() = entries;
            return;
        }
        *self.target.ivars().entries.borrow_mut() = entries;
        self.shape = shape;
        let app = NSApplication::sharedApplication(mtm);
        let bar = NSMenu::new(mtm);
        let name = NSProcessInfo::processInfo().processName().to_string();
        bar.addItem(&holder(mtm, &name, &app_menu(mtm, &name)));
        let mut tag = 0;
        for m in menus {
            bar.addItem(&holder(mtm, &m.name, &self.build(mtm, m, &mut tag)));
        }
        let window = window_menu(mtm);
        bar.addItem(&holder(mtm, "Window", &window));
        app.setMainMenu(Some(&bar));
        app.setWindowsMenu(Some(&window));
    }

    fn build(&self, mtm: MainThreadMarker, menu: &Menu, tag: &mut isize) -> Retained<NSMenu> {
        let out = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(&menu.name));
        for item in &menu.items {
            match item {
                MenuItem::Separator => out.addItem(&NSMenuItem::separatorItem(mtm)),
                MenuItem::Action { label, checked, shortcut, .. } => {
                    let it = item_with(mtm, label, Some(sel!(run:)), shortcut.as_deref());
                    // SAFETY: the target lives as long as the bar.
                    unsafe { it.setTarget(Some(&*self.target as &AnyObject)) };
                    it.setTag(*tag);
                    *tag += 1;
                    if let Some(c) = checked {
                        it.setState(if *c { 1 } else { 0 });
                    }
                    out.addItem(&it);
                }
                MenuItem::Submenu { menu, .. } => {
                    out.addItem(&holder(mtm, &menu.name, &self.build(mtm, menu, tag)));
                }
            }
        }
        out
    }
}

/// The menus without their states (what building depends on).
fn shape_of(menus: &[Menu]) -> Vec<Menu> {
    let mut shape = menus.to_vec();
    for m in &mut shape {
        for item in m.actions_mut() {
            if let MenuItem::Action { enabled, .. } = item {
                *enabled = true;
            }
        }
    }
    shape
}

/// The entries of `menu`'s items in order (their tags).
fn collect(menu: &Menu, out: &mut Vec<Entry>) {
    for item in &menu.items {
        match item {
            MenuItem::Action { id, enabled, shortcut, .. } => {
                out.push(Entry { command: id.clone(), chord: shortcut.clone(), enabled: *enabled })
            }
            MenuItem::Submenu { menu, .. } => collect(menu, out),
            MenuItem::Separator => {}
        }
    }
}

fn item_with(mtm: MainThreadMarker, label: &str, action: Option<Sel>, chord: Option<&str>) -> Retained<NSMenuItem> {
    let (key, mask) = chord.map_or((String::new(), NSEventModifierFlags::empty()), super::ask_mac::key_equivalent);
    // SAFETY: an item sending `action` (to its target, else the first responder).
    let it = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(label),
            action,
            &NSString::from_str(&key),
        )
    };
    it.setKeyEquivalentModifierMask(mask);
    it
}

/// An item of the bar (or a submenu's) holding `menu`.
fn holder(mtm: MainThreadMarker, title: &str, menu: &NSMenu) -> Retained<NSMenuItem> {
    let it = item_with(mtm, title, None, None);
    it.setSubmenu(Some(menu));
    it
}

/// The app's menu, as macOS apps have it (the system's actions, to the first responder).
fn app_menu(mtm: MainThreadMarker, name: &str) -> Retained<NSMenu> {
    let m = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(name));
    m.addItem(&item_with(mtm, &format!("About {name}"), Some(sel!(orderFrontStandardAboutPanel:)), None));
    m.addItem(&NSMenuItem::separatorItem(mtm));
    m.addItem(&item_with(mtm, &format!("Hide {name}"), Some(sel!(hide:)), Some("cmd+h")));
    m.addItem(&item_with(mtm, "Hide Others", Some(sel!(hideOtherApplications:)), Some("cmd+alt+h")));
    m.addItem(&item_with(mtm, "Show All", Some(sel!(unhideAllApplications:)), None));
    m.addItem(&NSMenuItem::separatorItem(mtm));
    m.addItem(&item_with(mtm, &format!("Quit {name}"), Some(sel!(terminate:)), Some("cmd+q")));
    m
}

/// The Window menu (AppKit lists the windows in it).
fn window_menu(mtm: MainThreadMarker) -> Retained<NSMenu> {
    let m = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("Window"));
    m.addItem(&item_with(mtm, "Minimize", Some(sel!(performMiniaturize:)), Some("cmd+m")));
    m.addItem(&item_with(mtm, "Zoom", Some(sel!(performZoom:)), None));
    m.addItem(&NSMenuItem::separatorItem(mtm));
    m.addItem(&item_with(mtm, "Bring All to Front", Some(sel!(arrangeInFront:)), None));
    m
}

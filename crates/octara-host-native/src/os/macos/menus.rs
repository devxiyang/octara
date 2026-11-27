//! The application menu bar and its window-independent command queue.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSEventType, NSMenu, NSMenuItem};
use objc2_foundation::{NSProcessInfo, NSString};
use octara_model::ask::{Menu, MenuItem};
use octara_model::input::Chord;
use std::cell::RefCell;

/// One of the app's items: its command, its keys, whether it is enabled now.
struct Entry {
    command: String,
    chord: Option<Chord>,
    enabled: bool,
}

pub(super) struct Bar {
    entries: RefCell<Vec<Entry>>,
    send: Box<dyn Fn(Selection)>,
}

pub(super) enum Selection {
    Command { name: String, chord: Option<Chord> },
    Paste,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraNativeMenuBar"]
    #[ivars = Bar]
    struct Target;

    impl Target {
        #[unsafe(method(run:))]
        fn run(&self, item: &NSMenuItem) {
            let entries = self.ivars().entries.borrow();
            let Some(e) = entries.get(item.tag() as usize).filter(|entry| entry.enabled) else { return };
            let chord = e.chord.filter(|_| by_key());
            (self.ivars().send)(Selection::Command { name: e.command.clone(), chord });
        }

        #[unsafe(method(runPaste:))]
        fn run_paste(&self, sender: &NSMenuItem) {
            let app = NSApplication::sharedApplication(self.mtm());
            // SAFETY: Paste is a standard responder action with one object parameter.
            if !unsafe { app.sendAction_to_from(sel!(paste:), None, Some(sender)) } {
                (self.ivars().send)(Selection::Paste);
            }
        }

        #[unsafe(method(validateMenuItem:))]
        fn validate(&self, item: &NSMenuItem) -> bool {
            if item.action() == Some(sel!(runPaste:)) {
                NSApplication::sharedApplication(self.mtm()).keyWindow().is_some()
            } else {
                self.ivars().entries.borrow().get(item.tag() as usize).is_some_and(|e| e.enabled)
            }
        }
    }
);

/// Whether the event being handled is a key press (a key equivalent being matched).
fn by_key() -> bool {
    MainThreadMarker::new()
        .is_some_and(|mtm| NSApplication::sharedApplication(mtm).currentEvent().is_some_and(|e| e.r#type() == NSEventType::KeyDown))
}

/// The menu bar the shell keeps.
pub(super) struct MenuBar {
    target: Retained<Target>,
    /// The definitions last installed in AppKit.
    shape: Option<Vec<Menu>>,
}

impl MenuBar {
    /// A bar whose picks go to `send` (a command and, picked by its keys, the chord).
    pub(super) fn new(send: impl Fn(Selection) + 'static) -> Option<MenuBar> {
        let mtm = MainThreadMarker::new()?;
        let target = Target::alloc(mtm).set_ivars(Bar { entries: RefCell::default(), send: Box::new(send) });
        let target: Retained<Target> = unsafe { msg_send![super(target), init] };
        Some(MenuBar { target, shape: None })
    }

    /// Installs menu definitions only when they change.
    pub(super) fn set(&mut self, menus: &[Menu]) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        if self.shape.as_deref() == Some(menus) {
            return;
        }
        let mut entries = Vec::new();
        for menu in menus {
            collect(menu, true, &mut entries);
        }
        *self.target.ivars().entries.borrow_mut() = entries;
        self.shape = Some(menus.to_vec());
        let app = NSApplication::sharedApplication(mtm);
        let bar = NSMenu::new(mtm);
        let name = NSProcessInfo::processInfo().processName().to_string();
        bar.addItem(&holder(mtm, &name, &app_menu(mtm, &name)));
        let mut tag = 0;
        let mut has_edit = false;
        for m in menus {
            let menu = self.build(mtm, m, &mut tag);
            if m.name.eq_ignore_ascii_case("Edit") {
                has_edit = true;
                if !m.items.iter().any(|item| matches!(item, MenuItem::Action { label, .. } if label.eq_ignore_ascii_case("Paste"))) {
                    menu.addItem(&self.paste_item(mtm));
                }
            }
            bar.addItem(&holder(mtm, &m.name, &menu));
        }
        if !has_edit {
            let edit = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("Edit"));
            edit.addItem(&self.paste_item(mtm));
            bar.addItem(&holder(mtm, "Edit", &edit));
        }
        let window = window_menu(mtm);
        bar.addItem(&holder(mtm, "Window", &window));
        app.setMainMenu(Some(&bar));
        app.setWindowsMenu(Some(&window));
    }

    fn paste_item(&self, mtm: MainThreadMarker) -> Retained<NSMenuItem> {
        let item = item_with(mtm, "Paste", Some(sel!(runPaste:)), None);
        // SAFETY: The target lives as long as the menu bar.
        unsafe { item.setTarget(Some(&*self.target)) };
        item
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
                MenuItem::Submenu { menu, enabled } => {
                    let item = holder(mtm, &menu.name, &self.build(mtm, menu, tag));
                    item.setEnabled(*enabled);
                    out.addItem(&item);
                }
            }
        }
        out
    }
}

/// The entries of `menu`'s items in order (their tags).
fn collect(menu: &Menu, parent_enabled: bool, out: &mut Vec<Entry>) {
    for item in &menu.items {
        match item {
            MenuItem::Action { id, enabled, shortcut, .. } => out.push(Entry {
                command: id.clone(),
                chord: shortcut.as_ref().and_then(|shortcut| shortcut.parse().ok()),
                enabled: parent_enabled && *enabled,
            }),
            MenuItem::Submenu { menu, enabled } => collect(menu, parent_enabled && *enabled, out),
            MenuItem::Separator => {}
        }
    }
}

fn item_with(mtm: MainThreadMarker, label: &str, action: Option<Sel>, chord: Option<&str>) -> Retained<NSMenuItem> {
    let (key, mask) = chord.map_or((String::new(), NSEventModifierFlags::empty()), super::asks::key_equivalent);
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

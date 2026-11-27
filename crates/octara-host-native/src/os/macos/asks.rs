//! AppKit popups, prompts, and file selection sheets.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSAlert, NSAlertStyle, NSEventModifierFlags, NSMenu, NSMenuItem, NSModalResponseOK, NSOpenPanel, NSSavePanel, NSView};
use objc2_core_foundation::{CFRunLoop, kCFRunLoopDefaultMode};
use objc2_foundation::{NSPoint, NSString, NSURL};
use octara_model::ask::{Answer, Ask, AskId, Menu, MenuItem, PromptLevel};
use octara_model::geometry::Point;
use octara_model::input::{Chord, Key, Modifiers};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// Which item a menu's pick was (its tag), set by the target.
pub(super) struct Picked(Cell<Option<isize>>);

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraNativePopupTarget"]
    #[ivars = Picked]
    struct Target;

    impl Target {
        #[unsafe(method(pick:))]
        fn pick(&self, item: &NSMenuItem) {
            self.ivars().0.set(Some(item.tag()));
        }
    }
);

/// AppKit may enter a nested run loop while a menu or sheet is open.
#[derive(Default)]
pub(super) struct Shown {
    pending: RefCell<HashSet<AskId>>,
    menu: RefCell<Option<(AskId, Retained<NSMenu>)>>,
    alerts: RefCell<HashMap<AskId, Retained<NSAlert>>>,
    panels: RefCell<HashMap<AskId, Retained<NSSavePanel>>>,
}

type Send = Rc<dyn Fn(AskId, Answer)>;

impl Shown {
    pub(super) fn show(self: &Rc<Self>, view: &NSView, id: AskId, ask: Ask, send: Send) {
        self.pending.borrow_mut().insert(id);
        let view = objc2::Message::retain(view);
        let shown = self.clone();
        later(move || {
            if !shown.pending.borrow().contains(&id) {
                return;
            }
            if view.window().is_none() {
                shown.complete(id, Answer::dismissed(&ask), &send);
                return;
            }
            match &ask {
                Ask::Menu { menu, at } => pop_up(&view, &shown, id, menu, *at, &send),
                Ask::Prompt { level, message, detail, answers } => {
                    alert(&view, &shown, id, *level, message, detail.as_deref(), answers, &send);
                }
                Ask::Paths(_) | Ask::NewPath { .. } => panel(&view, &shown, id, &ask, &send),
            }
        });
    }

    fn complete(&self, id: AskId, answer: Answer, send: &Send) {
        if self.pending.borrow_mut().remove(&id) {
            send(id, answer);
        }
    }

    pub(super) fn withdraw(&self, id: AskId) {
        self.pending.borrow_mut().remove(&id);
        let menu = self.menu.borrow().as_ref().filter(|(shown, _)| *shown == id).map(|(_, menu)| menu.clone());
        if let Some(menu) = menu {
            menu.cancelTracking();
        }
        let alert = self.alerts.borrow_mut().remove(&id);
        if let Some(alert) = alert {
            let sheet = alert.window();
            if let Some(parent) = sheet.sheetParent() {
                parent.endSheet(&sheet);
            }
        }
        let panel = self.panels.borrow_mut().remove(&id);
        if let Some(panel) = panel {
            // SAFETY: cancel accepts a nil action sender.
            unsafe { panel.cancel(None) };
        }
    }

    pub(super) fn close(&self) {
        let pending = self.pending.borrow().iter().copied().collect::<Vec<_>>();
        for id in pending {
            self.withdraw(id);
        }
    }
}

/// Runs `f` on the main run loop after the current callback.
pub(super) fn later(f: impl Fn() + 'static) {
    let Some(rl) = CFRunLoop::main() else { return };
    let block = block2::RcBlock::new(f);
    // SAFETY: the main run loop, its default mode and a block it copies.
    unsafe {
        rl.perform_block(kCFRunLoopDefaultMode.map(|m| &**m), Some(&block));
    }
    rl.wake_up();
}

/// The menu's `NSMenu`, its items targeting `target` and tagged by their index in `ids`.
fn build(mtm: MainThreadMarker, menu: &Menu, target: &Target, ids: &mut Vec<String>) -> Retained<NSMenu> {
    let out = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(&menu.name));
    out.setAutoenablesItems(false);
    for item in &menu.items {
        match item {
            MenuItem::Separator => out.addItem(&NSMenuItem::separatorItem(mtm)),
            MenuItem::Action { id, label, enabled, checked, shortcut } => {
                let (key, mask) = shortcut.as_deref().map_or((String::new(), NSEventModifierFlags::empty()), key_equivalent);
                // SAFETY: an item calling `pick:` on its target.
                let it = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(mtm),
                        &NSString::from_str(label),
                        Some(sel!(pick:)),
                        &NSString::from_str(&key),
                    )
                };
                it.setKeyEquivalentModifierMask(mask);
                // SAFETY: the target outlives the menu's tracking (both live in the block).
                unsafe { it.setTarget(Some(target as &AnyObject)) };
                it.setTag(ids.len() as isize);
                ids.push(id.clone());
                it.setEnabled(*enabled);
                if let Some(c) = checked {
                    it.setState(if *c { 1 } else { 0 });
                }
                out.addItem(&it);
            }
            MenuItem::Submenu { menu, enabled } => {
                // SAFETY: an item opening its submenu.
                let it = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(mtm),
                        &NSString::from_str(&menu.name),
                        None,
                        &NSString::from_str(""),
                    )
                };
                it.setSubmenu(Some(&build(mtm, menu, target, ids)));
                it.setEnabled(*enabled);
                out.addItem(&it);
            }
        }
    }
    out
}

/// A chord (`"mod+shift+c"`) as AppKit's key equivalent: its character and modifiers (keys
/// without a character show none).
pub(super) fn key_equivalent(chord: &str) -> (String, NSEventModifierFlags) {
    let Ok(Chord { key, modifiers, platform_mod }) = chord.parse::<Chord>() else { return (String::new(), NSEventModifierFlags::empty()) };
    let mut mask = NSEventModifierFlags::empty();
    for (on, flag) in [
        (platform_mod || modifiers.contains(Modifiers::META), NSEventModifierFlags::Command),
        (modifiers.contains(Modifiers::SHIFT), NSEventModifierFlags::Shift),
        (modifiers.contains(Modifiers::CTRL), NSEventModifierFlags::Control),
        (modifiers.contains(Modifiers::ALT), NSEventModifierFlags::Option),
    ] {
        if on {
            mask |= flag;
        }
    }
    let key = match key {
        Key::Char(key) => key,
        Key::Enter => '\r',
        Key::Tab => '\t',
        Key::Escape => '\u{1b}',
        Key::Backspace => '\u{8}',
        Key::Delete => '\u{f728}',
        Key::Space => ' ',
        Key::Up => '\u{f700}',
        Key::Down => '\u{f701}',
        Key::Left => '\u{f702}',
        Key::Right => '\u{f703}',
        Key::Home => '\u{f729}',
        Key::End => '\u{f72b}',
        Key::PageUp => '\u{f72c}',
        Key::PageDown => '\u{f72d}',
        Key::F(number) => char::from_u32(0xf703 + u32::from(number)).unwrap_or_default(),
        Key::Menu => return (String::new(), NSEventModifierFlags::empty()),
    };
    (key.to_string(), mask)
}

fn pop_up(view: &NSView, shown: &Rc<Shown>, id: AskId, menu: &Menu, Point { x, y }: Point, send: &Send) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let target: Retained<Target> = unsafe { msg_send![super(Target::alloc(mtm).set_ivars(Picked(Cell::new(None)))), init] };
    let mut ids = Vec::new();
    let ns = build(mtm, menu, &target, &mut ids);
    let y = if view.isFlipped() { y } else { view.bounds().size.height as f32 - y };
    if shown.menu.borrow().is_some() {
        shown.complete(id, Answer::Menu(None), send);
        return;
    }
    *shown.menu.borrow_mut() = Some((id, ns.clone()));
    // Tracks the menu until a pick or a dismissal (or `cancelTracking`).
    ns.popUpMenuPositioningItem_atLocation_inView(None, NSPoint::new(x as f64, y as f64), Some(view));
    shown.menu.borrow_mut().take();
    let picked = target.ivars().0.get().and_then(|t| ids.get(t as usize).cloned());
    shown.complete(id, Answer::Menu(picked), send);
}

#[allow(clippy::too_many_arguments)]
fn alert(
    view: &NSView,
    shown: &Rc<Shown>,
    id: AskId,
    level: PromptLevel,
    message: &str,
    detail: Option<&str>,
    answers: &[String],
    send: &Send,
) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let Some(window) = view.window() else {
        shown.complete(id, Answer::Prompt(None), send);
        return;
    };
    if window.attachedSheet().is_some() {
        shown.complete(id, Answer::Prompt(None), send);
        return;
    }
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(message));
    if let Some(d) = detail {
        alert.setInformativeText(&NSString::from_str(d));
    }
    alert.setAlertStyle(match level {
        PromptLevel::Info => NSAlertStyle::Informational,
        PromptLevel::Warning => NSAlertStyle::Warning,
        PromptLevel::Critical => NSAlertStyle::Critical,
    });
    // AppKit's first button is the default (rightmost): the last answer is added first.
    for a in answers.iter().rev() {
        let b = alert.addButtonWithTitle(&NSString::from_str(a));
        if a.eq_ignore_ascii_case("cancel") {
            b.setKeyEquivalent(&NSString::from_str("\u{1b}"));
        }
    }
    let n = answers.len();
    let (shown_by, send) = (shown.clone(), send.clone());
    let done = block2::RcBlock::new(move |response: isize| {
        shown_by.alerts.borrow_mut().remove(&id);
        // The first button returns 1000, the next 1001, …
        let k = response - 1000;
        let picked = (0..n as isize).contains(&k).then(|| n - 1 - k as usize);
        shown_by.complete(id, Answer::Prompt(picked), &send);
    });
    shown.alerts.borrow_mut().insert(id, alert.clone());
    alert.beginSheetModalForWindow_completionHandler(&window, Some(&done));
}

fn panel(view: &NSView, shown: &Rc<Shown>, id: AskId, ask: &Ask, send: &Send) {
    let Some(window) = view.window().filter(|window| window.attachedSheet().is_none()) else {
        shown.complete(id, Answer::dismissed(ask), send);
        return;
    };
    let Some(mtm) = MainThreadMarker::new() else { return };
    let (save, open) = match ask {
        Ask::Paths(options) => {
            let panel = NSOpenPanel::openPanel(mtm);
            panel.setCanChooseFiles(options.files);
            panel.setCanChooseDirectories(options.directories);
            panel.setAllowsMultipleSelection(options.multiple);
            if let Some(prompt) = &options.prompt {
                panel.setPrompt(Some(&NSString::from_str(prompt)));
            }
            (panel.clone().into_super(), Some(panel))
        }
        Ask::NewPath { directory, name } => {
            let panel = NSSavePanel::savePanel(mtm);
            panel.setDirectoryURL(NSURL::from_file_path(directory).as_deref());
            if let Some(name) = name {
                panel.setNameFieldStringValue(&NSString::from_str(name));
            }
            (panel, None)
        }
        _ => return,
    };
    let done = {
        let shown = shown.clone();
        let send = send.clone();
        let default = Answer::dismissed(ask);
        let save = save.clone();
        block2::RcBlock::new(move |response| {
            shown.panels.borrow_mut().remove(&id);
            let answer = if response != NSModalResponseOK {
                default.clone()
            } else if let Some(panel) = &open {
                let paths = panel.URLs().iter().filter_map(|url| url.to_file_path()).collect::<Vec<_>>();
                Answer::Paths((!paths.is_empty()).then_some(paths))
            } else {
                Answer::NewPath(save.URL().and_then(|url| url.to_file_path()))
            };
            save.orderOut(None);
            shown.complete(id, answer, &send);
        })
    };
    shown.panels.borrow_mut().insert(id, save.clone());
    save.beginSheetModalForWindow_completionHandler(&window, &done);
}

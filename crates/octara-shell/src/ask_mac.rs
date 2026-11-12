//! Requests a macOS window shows natively ([`octara::ask`]): a menu as an `NSMenu` popped up at
//! its point, a prompt as an `NSAlert` sheet on the window. Both open from the main run loop
//! after the loop's callback returns (AppKit tracks a menu in a loop of its own, and winit's
//! handler must not be entered again from inside it, as Rune found); the answer goes back
//! through `send`, as input. A request withdrawn (answered otherwise, the window closing) closes.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSAlert, NSAlertStyle, NSEventModifierFlags, NSMenu, NSMenuItem, NSView};
use objc2_core_foundation::{CFRunLoop, kCFRunLoopDefaultMode};
use objc2_foundation::{NSPoint, NSString};
use octara::ask::{Answer, Ask, AskId, Menu, MenuItem, PromptLevel};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

/// Which item a menu's pick was (its tag), set by the target.
pub(crate) struct Picked(Cell<Option<isize>>);

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraMenuTarget"]
    #[ivars = Picked]
    struct Target;

    impl Target {
        #[unsafe(method(pick:))]
        fn pick(&self, item: &NSMenuItem) {
            self.ivars().0.set(Some(item.tag()));
        }
    }
);

/// What a window shows of its requests now.
#[derive(Default)]
pub(crate) struct Shown {
    /// The menu tracking, and its request.
    menu: RefCell<Option<(AskId, Retained<NSMenu>)>>,
    /// The alerts on the window, by request.
    alerts: RefCell<Vec<(AskId, Retained<NSAlert>)>>,
}

/// Where answers go.
pub(crate) type Send = Rc<dyn Fn(AskId, Answer)>;

/// Whether the window shows `ask` itself.
pub(crate) fn shows(ask: &Ask) -> bool {
    matches!(ask, Ask::Menu { .. } | Ask::Prompt { .. })
}

/// Shows `ask` (from the main run loop, once the loop's callback returned).
pub(crate) fn show(window: &Window, shown: &Rc<Shown>, id: AskId, ask: Ask, send: Send) {
    let Some(view) = content_view(window) else {
        send(id, Answer::dismissed(&ask));
        return;
    };
    let shown = shown.clone();
    later(move || match &ask {
        Ask::Menu { menu, at } => pop_up(&view, &shown, id, menu, *at, &send),
        Ask::Prompt { level, message, detail, answers } => alert(&view, &shown, id, *level, message, detail.as_deref(), answers, &send),
        _ => {}
    });
}

/// Closes what shows of request `id`.
pub(crate) fn withdraw(shown: &Shown, id: AskId) {
    if let Some((_, menu)) = shown.menu.borrow().as_ref().filter(|(m, _)| *m == id) {
        menu.cancelTracking();
    }
    let alert = {
        let mut alerts = shown.alerts.borrow_mut();
        alerts.iter().position(|(a, _)| *a == id).map(|i| alerts.remove(i).1)
    };
    if let Some(alert) = alert {
        let sheet = alert.window();
        if let Some(parent) = sheet.sheetParent() {
            parent.endSheet(&sheet);
        }
    }
}

/// Runs `f` on the main run loop after the current callback.
fn later(f: impl Fn() + 'static) {
    let Some(rl) = CFRunLoop::main() else { return };
    let block = block2::RcBlock::new(f);
    // SAFETY: the main run loop, its default mode and a block it copies.
    unsafe {
        rl.perform_block(kCFRunLoopDefaultMode.map(|m| &**m), Some(&block));
    }
    rl.wake_up();
}

fn content_view(window: &Window) -> Option<Retained<NSView>> {
    let RawWindowHandle::AppKit(h) = window.window_handle().ok()?.as_raw() else { return None };
    // SAFETY: winit's content view, alive as long as the window.
    unsafe { Retained::retain(h.ns_view.as_ptr().cast()) }
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
fn key_equivalent(chord: &str) -> (String, NSEventModifierFlags) {
    let mut mask = NSEventModifierFlags::empty();
    let mut key = String::new();
    for part in chord.split('+') {
        match part {
            "mod" | "cmd" => mask |= NSEventModifierFlags::Command,
            "shift" => mask |= NSEventModifierFlags::Shift,
            "alt" => mask |= NSEventModifierFlags::Option,
            "ctrl" => mask |= NSEventModifierFlags::Control,
            k if k.chars().count() == 1 => key = k.to_lowercase(),
            "enter" => key = "\r".into(),
            "backspace" => key = "\u{8}".into(),
            "delete" => key = "\u{7f}".into(),
            "tab" => key = "\t".into(),
            "space" => key = " ".into(),
            _ => return (String::new(), NSEventModifierFlags::empty()),
        }
    }
    (key, mask)
}

fn pop_up(view: &NSView, shown: &Rc<Shown>, id: AskId, menu: &Menu, (x, y): (f32, f32), send: &Send) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let target: Retained<Target> = unsafe { msg_send![super(Target::alloc(mtm).set_ivars(Picked(Cell::new(None)))), init] };
    let mut ids = Vec::new();
    let ns = build(mtm, menu, &target, &mut ids);
    let y = if view.isFlipped() { y } else { view.bounds().size.height as f32 - y };
    *shown.menu.borrow_mut() = Some((id, ns.clone()));
    // Tracks the menu until a pick or a dismissal (or `cancelTracking`).
    ns.popUpMenuPositioningItem_atLocation_inView(None, NSPoint::new(x as f64, y as f64), Some(view));
    shown.menu.borrow_mut().take();
    let picked = target.ivars().0.get().and_then(|t| ids.get(t as usize).cloned());
    send(id, Answer::Menu(picked));
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
        send(id, Answer::Prompt(None));
        return;
    };
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
        shown_by.alerts.borrow_mut().retain(|(a, _)| *a != id);
        // The first button returns 1000, the next 1001, …
        let k = response - 1000;
        let picked = (0..n as isize).contains(&k).then(|| n - 1 - k as usize);
        send(id, Answer::Prompt(picked));
    });
    shown.alerts.borrow_mut().push((id, alert.clone()));
    alert.beginSheetModalForWindow_completionHandler(&window, Some(&done));
}

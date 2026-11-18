//! Command menus composed on the same surface as the runtime's drawn context menus.

use crate::{Model, button};
use octara::menu::menu_surface;
use octara::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// A button opening a finite command menu controlled by `open`.
///
/// The trigger is `id`, the popup `{id}-menu`, actions `{id}-action-{command}`, and submenu
/// triggers `{id}-submenu-{index}`. Each submenu extends that prefix for its own rows. Action
/// IDs must be unique throughout the snapshot. Submenus have no stable ID in [`MenuItem`], so
/// their indices identify only this opening; identical display names remain distinct.
///
/// `menu` is sampled once per opening. Changes to labels, checked states, availability or order
/// appear next time it opens. The open menu retains its snapshot and node identities. Supply
/// current `enabled` states through the menu's items; unavailable commands should be disabled.
///
/// Down opens on the first enabled item, Up on the last. Inside, Up/Down and Home/End navigate,
/// Right opens a submenu, and Left or Escape closes the innermost submenu. Typing searches
/// labels by prefix. Outside presses and Escape dismiss without invoking an action.
/// Pointer presses preserve the previous control's focus until the menu opens. A pick closes
/// the entire menu and runs its command after that focus is restored, so editor commands keep
/// their original selection and context. Keyboard opening restores focus to the trigger.
/// Component behavior opens before application `on_press`; application `on_key` receives only
/// unconsumed keys. Adding these callbacks preserves opening and navigation.
#[ui]
#[track_caller]
pub fn menu_button(id: &str, label: impl Into<String>, open: impl Model<bool>, menu: impl Fn() -> Menu + 'static) -> El {
    let id = id.to_owned();
    let open = Rc::new(open);
    let menu = Rc::new(menu);
    let pending = Rc::new(RefCell::new(None::<Menu>));
    let (expanded, press, shown, keys, dismiss, pick) = (open.clone(), open.clone(), open.clone(), open.clone(), open.clone(), open);
    let (key_menu, key_pending, key_id) = (menu.clone(), pending.clone(), id.clone());
    let press_pending = pending.clone();
    button(label.into())
        .id(id.clone())
        .preserve_focus_on_press(true)
        .expanded(move || expanded.get())
        .behavior(|b| {
            b.on_press(move || {
                press_pending.borrow_mut().take();
                press.set(!press.get());
            })
            .on_key(move |key| {
                if !key.modifiers.is_empty() || !matches!(key.key, Key::Up | Key::Down) {
                    return false;
                }
                let snapshot = key_menu();
                let mut enabled = snapshot.items.iter().enumerate().filter_map(|(index, item)| match item {
                    MenuItem::Action { id, enabled: true, .. } => Some(format!("{key_id}-action-{id}")),
                    MenuItem::Submenu { enabled: true, .. } => Some(format!("{key_id}-submenu-{index}")),
                    _ => None,
                });
                let first = if key.key == Key::Up { enabled.next_back() } else { enabled.next() };
                *key_pending.borrow_mut() = Some(snapshot);
                keys.set(true);
                if let Some(first) = first {
                    focus(&first);
                }
                true
            })
        })
        .overlay(
            move || shown.get(),
            Placement::BELOW,
            move || {
                let snapshot = pending.borrow_mut().take().unwrap_or_else(|| untrack(|| menu()));
                let (pick, dismiss) = (pick.clone(), dismiss.clone());
                menu_surface(
                    &id,
                    snapshot,
                    move |command| {
                        pick.set(false);
                        // Command dispatch must follow the modal's focus restoration.
                        invoke_after_update(&command);
                    },
                    move || dismiss.set(false),
                )
            },
        )
}

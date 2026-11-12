//! Menus ([windows](../../../docs/design/windows.md)): what a context menu shows
//! ([`popup_menu`](crate::ask::popup_menu)) and the app's menu bar ([`set_menus`]), whose items
//! run commands.

use serde::{Deserialize, Serialize};

/// A menu: its name (a menu bar's title) and its items in order (GPUI's `Menu`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Menu {
    pub name: String,
    pub items: Vec<MenuItem>,
}

impl Menu {
    pub fn new(name: impl Into<String>) -> Menu {
        Menu { name: name.into(), items: Vec::new() }
    }
    pub fn item(mut self, item: MenuItem) -> Menu {
        self.items.push(item);
        self
    }
    pub fn items(mut self, items: impl IntoIterator<Item = MenuItem>) -> Menu {
        self.items.extend(items);
        self
    }
    /// The item with `id`, in it or its submenus.
    pub fn find(&self, id: &str) -> Option<&MenuItem> {
        self.items.iter().find_map(|i| match i {
            MenuItem::Action { id: own, .. } if own == id => Some(i),
            MenuItem::Submenu { menu, .. } => menu.find(id),
            _ => None,
        })
    }
}

/// One entry of a menu.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MenuItem {
    /// An item a person picks: what it answers (`id`; in a menu bar, the command it runs), its
    /// label, whether it can be picked, its check mark, and the keys shown for it.
    Action {
        id: String,
        label: String,
        enabled: bool,
        checked: Option<bool>,
        shortcut: Option<String>,
    },
    Separator,
    Submenu {
        menu: Menu,
        enabled: bool,
    },
}

impl MenuItem {
    /// An item labeled `label` answering `id`.
    pub fn action(label: impl Into<String>, id: impl Into<String>) -> MenuItem {
        MenuItem::Action { id: id.into(), label: label.into(), enabled: true, checked: None, shortcut: None }
    }
    pub fn separator() -> MenuItem {
        MenuItem::Separator
    }
    pub fn submenu(menu: Menu) -> MenuItem {
        MenuItem::Submenu { menu, enabled: true }
    }
    /// It cannot be picked (shown dimmed).
    pub fn disabled(mut self) -> MenuItem {
        match &mut self {
            MenuItem::Action { enabled, .. } | MenuItem::Submenu { enabled, .. } => *enabled = false,
            MenuItem::Separator => {}
        }
        self
    }
    /// With a check mark, on or off.
    pub fn checked(mut self, on: bool) -> MenuItem {
        if let MenuItem::Action { checked, .. } = &mut self {
            *checked = Some(on);
        }
        self
    }
    /// The keys shown next to it (`"mod+c"`), as a hint: a menu bar's item takes its keys from
    /// the keymap instead.
    pub fn shortcut(mut self, chord: impl Into<String>) -> MenuItem {
        if let MenuItem::Action { shortcut, .. } = &mut self {
            *shortcut = Some(chord.into());
        }
        self
    }
}

impl Menu {
    /// The standard Edit menu: undo, redo, cut, copy, paste, select all (the `edit.*` commands).
    pub fn edit() -> Menu {
        Menu::new("Edit").items([
            MenuItem::action("Undo", "edit.undo"),
            MenuItem::action("Redo", "edit.redo"),
            MenuItem::separator(),
            MenuItem::action("Cut", "edit.cut"),
            MenuItem::action("Copy", "edit.copy"),
            MenuItem::action("Paste", "edit.paste"),
            MenuItem::action("Select All", "edit.select_all"),
        ])
    }

    /// Every action item, its submenus' included.
    pub fn actions_mut(&mut self) -> Vec<&mut MenuItem> {
        let mut out = Vec::new();
        for item in &mut self.items {
            match item {
                MenuItem::Action { .. } => out.push(item),
                MenuItem::Submenu { menu, .. } => out.extend(menu.actions_mut()),
                MenuItem::Separator => {}
            }
        }
        out
    }
}

/// Sets the app's menus (GPUI's `set_menus`): on macOS the menu bar shows them between the app's
/// menu and the Window menu (without a call, [`Menu::edit`] alone). An item's id is the command
/// it runs in the active window, enabled while something there handles it, with the keys the
/// keymap binds to it. Callable from handlers.
pub fn set_menus(menus: impl IntoIterator<Item = Menu>) {
    crate::app::app_state().set_menus(menus.into_iter().collect());
}

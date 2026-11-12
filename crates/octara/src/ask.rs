//! What an app asks a person through the system ([windows](../../../docs/design/windows.md)): a
//! menu, a prompt, paths to open, where to save. A request is the window's state: the snapshot
//! lists it, the window presents it to its platform window, which shows it natively when it can
//! (else the app draws menus and prompts in the window), and the answer comes back as input
//! ([`InputEvent::Answer`](crate::app::InputEvent::Answer)), so it is recorded and replayed, and
//! agents and tests answer it the same way. A task awaits it.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A request's number in its window.
pub type AskId = u64;

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

/// How serious a prompt is (GPUI's `PromptLevel`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptLevel {
    #[default]
    Info,
    Warning,
    Critical,
}

/// What a dialog for paths to open lets a person pick (GPUI's `PathPromptOptions`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PathPromptOptions {
    pub files: bool,
    pub directories: bool,
    pub multiple: bool,
    /// The dialog's prompt (its title or button).
    pub prompt: Option<String>,
}

/// A request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ask {
    /// A menu at a point of the window (logical pixels).
    Menu {
        menu: Menu,
        at: (f32, f32),
    },
    /// A message with answers to pick from (buttons, the last the default).
    Prompt {
        level: PromptLevel,
        message: String,
        detail: Option<String>,
        answers: Vec<String>,
    },
    Paths(PathPromptOptions),
    NewPath {
        directory: PathBuf,
        name: Option<String>,
    },
}

/// A request's answer (`None`: dismissed).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    /// The id of the item picked.
    Menu(Option<String>),
    /// The index of the answer picked.
    Prompt(Option<usize>),
    Paths(Option<Vec<PathBuf>>),
    NewPath(Option<PathBuf>),
}

impl Answer {
    /// The answer that dismisses `ask`.
    pub fn dismissed(ask: &Ask) -> Answer {
        match ask {
            Ask::Menu { .. } => Answer::Menu(None),
            Ask::Prompt { .. } => Answer::Prompt(None),
            Ask::Paths(_) => Answer::Paths(None),
            Ask::NewPath { .. } => Answer::NewPath(None),
        }
    }
    /// Whether it answers `ask` (its kind, and an item or answer it has).
    pub fn fits(&self, ask: &Ask) -> bool {
        match (self, ask) {
            (Answer::Menu(id), Ask::Menu { menu, .. }) => {
                id.as_ref().is_none_or(|id| menu.find(id).is_some_and(|i| matches!(i, MenuItem::Action { enabled: true, .. })))
            }
            (Answer::Prompt(i), Ask::Prompt { answers, .. }) => i.is_none_or(|i| i < answers.len()),
            (Answer::Paths(_), Ask::Paths(_)) | (Answer::NewPath(_), Ask::NewPath { .. }) => true,
            _ => false,
        }
    }
}

/// Shows `menu` at `at` in the current window (else at the pointer, or under the focused
/// element: a context menu from the keyboard) and gives the id of the item picked, `None` when
/// dismissed (Rune's `popup_menu`). Await it in a task.
pub fn popup_menu(menu: Menu, at: Option<(f32, f32)>) -> impl Future<Output = Option<String>> {
    let w = crate::app::current();
    let at = at.unwrap_or_else(|| w.menu_point());
    let answer = w.ask(Ask::Menu { menu, at });
    async move {
        match answer.await {
            Answer::Menu(id) => id,
            _ => None,
        }
    }
}

/// Asks with a message and answers to pick from (GPUI's `prompt`): the index of the one picked,
/// `None` when dismissed. Await it in a task.
pub fn prompt(level: PromptLevel, message: &str, detail: Option<&str>, answers: &[&str]) -> impl Future<Output = Option<usize>> {
    let ask = Ask::Prompt {
        level,
        message: message.to_owned(),
        detail: detail.map(str::to_owned),
        answers: answers.iter().map(|a| (*a).to_owned()).collect(),
    };
    let answer = crate::app::current().ask(ask);
    async move {
        match answer.await {
            Answer::Prompt(i) => i,
            _ => None,
        }
    }
}

/// Asks for files or directories to open (GPUI's `prompt_for_paths`): `None` when cancelled.
/// Await it in a task.
pub fn prompt_for_paths(options: PathPromptOptions) -> impl Future<Output = Option<Vec<PathBuf>>> {
    let answer = crate::app::current().ask(Ask::Paths(options));
    async move {
        match answer.await {
            Answer::Paths(p) => p,
            _ => None,
        }
    }
}

/// Asks where to save, in `directory`, suggesting `name` (GPUI's `prompt_for_new_path`): `None`
/// when cancelled. Await it in a task.
pub fn prompt_for_new_path(directory: &Path, name: Option<&str>) -> impl Future<Output = Option<PathBuf>> {
    let answer = crate::app::current().ask(Ask::NewPath { directory: directory.to_owned(), name: name.map(str::to_owned) });
    async move {
        match answer.await {
            Answer::NewPath(p) => p,
            _ => None,
        }
    }
}

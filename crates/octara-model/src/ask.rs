//! Requests and replies for platform prompts and services.

use crate::geometry::Point;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

/// Identifies a pending ask within its window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AskId(pub u64);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Menu {
    pub name: String,
    pub items: Vec<MenuItem>,
}

impl Menu {
    fn accepts(&self, id: &str) -> bool {
        self.items.iter().any(|item| match item {
            MenuItem::Action { id: own, enabled, .. } => own == id && *enabled,
            MenuItem::Submenu { menu, enabled } => *enabled && menu.accepts(id),
            MenuItem::Separator => false,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MenuItem {
    Action { id: String, label: String, enabled: bool, checked: Option<bool>, shortcut: Option<String> },
    Separator,
    Submenu { menu: Menu, enabled: bool },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptLevel {
    #[default]
    Info,
    Warning,
    Critical,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathPromptOptions {
    pub files: bool,
    pub directories: bool,
    pub multiple: bool,
    pub prompt: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ask {
    Menu { menu: Menu, at: Point },
    Prompt { level: PromptLevel, message: String, detail: Option<String>, answers: Vec<String> },
    Paths(PathPromptOptions),
    NewPath { directory: PathBuf, name: Option<String> },
}

impl Ask {
    /// Prompts for a single native directory path.
    pub fn pick_directory(prompt: Option<String>) -> Self {
        Self::Paths(PathPromptOptions { directories: true, prompt, ..Default::default() })
    }

    /// Prompts for native file paths, with optional multiple selection.
    pub fn pick_files(prompt: Option<String>, multiple: bool) -> Self {
        Self::Paths(PathPromptOptions { files: true, multiple, prompt, ..Default::default() })
    }
}

/// Who presents an ask for a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskRoute {
    Platform,
    App,
    Unavailable,
}

/// An answer to a pending ask. For each answer kind, `None` indicates dismissal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    Menu(Option<String>),
    Prompt(Option<usize>),
    Paths(Option<Vec<PathBuf>>),
    NewPath(Option<PathBuf>),
}

impl Answer {
    pub fn dismissed(ask: &Ask) -> Self {
        match ask {
            Ask::Menu { .. } => Self::Menu(None),
            Ask::Prompt { .. } => Self::Prompt(None),
            Ask::Paths(_) => Self::Paths(None),
            Ask::NewPath { .. } => Self::NewPath(None),
        }
    }

    /// Whether this reply is compatible with the pending ask and its offered options.
    pub fn fits(&self, ask: &Ask) -> bool {
        match (self, ask) {
            (Self::Menu(id), Ask::Menu { menu, .. }) => id.as_deref().is_none_or(|id| menu.accepts(id)),
            (Self::Prompt(index), Ask::Prompt { answers, .. }) => index.is_none_or(|index| index < answers.len()),
            (Self::Paths(paths), Ask::Paths(options)) => {
                paths.as_ref().is_none_or(|paths| !paths.is_empty() && (options.multiple || paths.len() == 1))
            }
            (Self::NewPath(_), Ask::NewPath { .. }) => true,
            _ => false,
        }
    }
}

/// Identifies a pending service request within its window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ServiceId(pub u64);

/// Data offered by the system clipboard. Debug formatting omits private contents.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardItem {
    Text(String),
    Image { media_type: String, bytes: Vec<u8> },
    Files(Vec<PathBuf>),
}

impl fmt::Debug for ClipboardItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::Text(_) => "Text",
            Self::Image { .. } => "Image",
            Self::Files(_) => "Files",
        };
        f.debug_tuple(kind).field(&"<redacted>").finish()
    }
}

bitflags::bitflags! {
    /// Clipboard formats a caller accepts.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct ClipboardKinds: u8 {
        const TEXT = 1 << 0;
        const IMAGE = 1 << 1;
        const FILES = 1 << 2;
    }
}

/// An operation requested from the host. Recorders must omit private transfer data.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceRequest {
    PickFile { max_bytes: usize, accept: String },
    ReadClipboard { accept: ClipboardKinds },
    WriteClipboardText { text: String },
}

impl fmt::Debug for ServiceRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PickFile { max_bytes, accept } => {
                f.debug_struct("PickFile").field("max_bytes", max_bytes).field("accept", accept).finish()
            }
            Self::ReadClipboard { accept } => f.debug_struct("ReadClipboard").field("accept", accept).finish(),
            Self::WriteClipboardText { .. } => f.debug_struct("WriteClipboardText").field("text", &"<redacted>").finish(),
        }
    }
}

/// Bytes from a file explicitly selected in the host's file chooser.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickedFile {
    pub name: String,
    pub media_type: String,
    pub bytes: Vec<u8>,
}

/// Terminal result of a platform service request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceOutcome {
    FilePicked(Option<PickedFile>),
    Clipboard(Vec<ClipboardItem>),
    ClipboardWritten,
    Error(ServiceError),
}

impl ServiceOutcome {
    /// The first text representation returned by a clipboard read.
    pub fn clipboard_text(&self) -> Option<&str> {
        let Self::Clipboard(items) = self else { return None };
        items.iter().find_map(|item| match item {
            ClipboardItem::Text(text) => Some(text.as_str()),
            _ => None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceError {
    Unavailable,
    ActivationRequired,
    Denied,
    Busy,
    Cancelled,
    TooLarge { bytes: u64, max_bytes: usize },
    Failed,
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => f.write_str("Unavailable in this host"),
            Self::ActivationRequired => f.write_str("A user gesture is required"),
            Self::Denied => f.write_str("Permission denied"),
            Self::Busy => f.write_str("The host is already serving a request"),
            Self::Cancelled => f.write_str("Request cancelled"),
            Self::TooLarge { bytes, max_bytes } => write!(f, "File exceeds the {max_bytes}-byte limit ({bytes} bytes)"),
            Self::Failed => f.write_str("The host could not complete the request"),
        }
    }
}

impl std::error::Error for ServiceError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_helpers_and_clipboard_text_preserve_payload_types() {
        assert_eq!(
            Ask::pick_directory(Some("Choose a folder".into())),
            Ask::Paths(PathPromptOptions { directories: true, prompt: Some("Choose a folder".into()), ..Default::default() }),
        );
        assert_eq!(Ask::pick_files(None, true), Ask::Paths(PathPromptOptions { files: true, multiple: true, ..Default::default() }));
        let images = vec![ClipboardItem::Image { media_type: "image/jpeg".into(), bytes: vec![1, 2, 3] }];
        assert_eq!(ServiceOutcome::Clipboard(images.clone()).clipboard_text(), None);
        let contents = ServiceOutcome::Clipboard([images, vec![ClipboardItem::Text("text".into())]].concat());
        assert_eq!(contents.clipboard_text(), Some("text"));
    }

    #[test]
    fn answers_respect_disabled_submenus_and_prompt_bounds() {
        let menu = Menu {
            name: "File".into(),
            items: vec![MenuItem::Submenu {
                enabled: false,
                menu: Menu {
                    name: "New".into(),
                    items: vec![MenuItem::Action { id: "new".into(), label: "File".into(), enabled: true, checked: None, shortcut: None }],
                },
            }],
        };
        let ask = Ask::Menu { menu, at: Point::new(0.0, 0.0) };
        assert!(!Answer::Menu(Some("new".into())).fits(&ask));
        assert!(Answer::dismissed(&ask).fits(&ask));

        let ask = Ask::Prompt { level: PromptLevel::Info, message: "Continue?".into(), detail: None, answers: vec!["Continue".into()] };
        assert!(Answer::Prompt(Some(0)).fits(&ask));
        assert!(!Answer::Prompt(Some(1)).fits(&ask));
        assert!(!Answer::Menu(None).fits(&ask));
    }
}

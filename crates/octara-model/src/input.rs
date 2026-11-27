//! Serializable interaction, editor, and host-reply channels.
//!
//! A recorder must redact secure editor input, dropped files, and private service data before persisting events.

use crate::ask::{Answer, AskId, ClipboardItem, ServiceId, ServiceOutcome};
use crate::geometry::{Point, Size, Vec2, WindowViewport};
use crate::identity::{EditorSessionId, ExitTicket, TargetGuard};
use crate::native::{NativeEvent, NativeMeasureTicket, NativeSnapshot, NativeSnapshotRequest};
use crate::semantics::A11yAction;
pub use crate::text::{CommitKind, TextEdit};
use crate::window::{SystemPreferences, WindowChrome, WindowGeometry};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::Range;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

/// How a logical rich-text link was targeted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkEventKind {
    Press,
    SecondaryPress,
    Hover(bool),
}

#[derive(Clone, Debug, PartialEq)]
pub struct LinkEvent {
    pub href: Arc<str>,
    pub text: Arc<str>,
    pub kind: LinkEventKind,
    /// Modifier keys held when the pointer activated this link.
    pub modifiers: Modifiers,
    pub rect: crate::geometry::Rect,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CopyFormat {
    Plain,
    Markdown,
    Custom(Arc<str>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContextMenuItem {
    Copy { format: CopyFormat, text: String },
    CopyLink(Arc<str>),
    OpenLink(Arc<str>),
    Custom { id: Arc<str>, label: Arc<str> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContextMenuEvent {
    pub target: crate::identity::NodeId,
    pub position: Option<Point>,
    pub rect: crate::geometry::Rect,
    pub items: Vec<ContextMenuItem>,
}

/// Interaction dispatched through the same retained-tree arbiter on all hosts.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionEvent {
    PointerDown { id: PointerId, kind: PointerKind, target: ContactTarget, pos: Point, button: Button, mods: Modifiers },
    PointerMove { id: PointerId, pos: Point, mods: Modifiers },
    PointerUp { id: PointerId, target: Option<ContactTarget>, pos: Point, mods: Modifiers },
    PointerCancel { id: PointerId },
    PointerLeave,
    Wheel { pos: Point, delta: Vec2, phase: WheelPhase, mods: Modifiers },
    Key(KeyEvent),
    BackGesture { id: u64, phase: BackPhase },
    WindowActive(bool),
}

/// The guarded hit and the action-owning allocation chosen by the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContactTarget {
    pub hit: TargetGuard,
    pub action: Option<TargetGuard>,
}

/// Authored data carried for the lifetime of an in-app drag.
#[derive(Clone, Debug, PartialEq)]
pub struct DragPayload {
    pub kind: Arc<str>,
    pub data: Arc<serde_json::Value>,
    pub preview_label: Option<Arc<str>>,
}

impl DragPayload {
    pub fn new(kind: impl Into<Arc<str>>, data: impl Into<serde_json::Value>) -> Self {
        Self { kind: kind.into(), data: Arc::new(data.into()), preview_label: None }
    }

    pub fn preview_label(mut self, label: impl Into<Arc<str>>) -> Self {
        self.preview_label = Some(label.into());
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropPosition {
    Before,
    Inside,
    After,
}

/// A change in a receiver's drag hover. A missing position indicates departure.
#[derive(Clone, Debug, PartialEq)]
pub struct DropHover {
    pub source: TargetGuard,
    pub payload: DragPayload,
    pub position: Option<DropPosition>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DropEvent {
    pub source: TargetGuard,
    pub kind: Arc<str>,
    pub data: Arc<serde_json::Value>,
    pub position: DropPosition,
    pub modifiers: Modifiers,
}

/// Data offered to the focused node by a system paste gesture.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PasteEvent {
    pub items: Vec<ClipboardItem>,
}

/// A pointer's identity within this window for the duration of a contact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PointerId(pub u64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PointerKind {
    #[default]
    Mouse,
    Touch,
    Pen,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    #[default]
    Primary,
    Secondary,
    Middle,
}

bitflags::bitflags! {
    /// Modifier keys held during an input.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct Modifiers: u8 {
        const SHIFT = 1 << 0;
        const CTRL = 1 << 1;
        const ALT = 1 << 2;
        const META = 1 << 3;
    }
}

/// A key as interpreted by the current keyboard layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Key {
    Tab,
    Enter,
    Escape,
    Backspace,
    Delete,
    Space,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Menu,
    F(u8),
    Char(char),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyEvent {
    pub key: Key,
    /// The printable character on the physical key in a US layout, when available.
    pub code: Option<char>,
    pub modifiers: Modifiers,
    pub repeat: bool,
}

impl KeyEvent {
    pub fn new(key: Key, modifiers: Modifiers) -> Self {
        let code = match key {
            Key::Char(c) if c.is_ascii_alphanumeric() => Some(c.to_ascii_lowercase()),
            _ => None,
        };
        Self { key, code, modifiers, repeat: false }
    }

    pub fn requests_context_menu(self) -> bool {
        (self.key == Key::Menu && self.modifiers.is_empty()) || (self.key == Key::F(10) && self.modifiers == Modifiers::SHIFT)
    }
}

/// A key and its shortcut modifiers, written for example as `mod+shift+k`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Chord {
    pub key: Key,
    pub modifiers: Modifiers,
    /// Include the host's primary shortcut modifier, usually Command or Control.
    pub platform_mod: bool,
}

impl Chord {
    /// Match a logical key or its reported physical character, with the host's
    /// primary modifier substituted for `mod`.
    pub fn matches(self, event: KeyEvent, primary: Modifiers) -> bool {
        let modifiers = self.modifiers | if self.platform_mod { primary } else { Modifiers::empty() };
        modifiers == event.modifiers
            && match (self.key, event.key) {
                (Key::Char(expected), Key::Char(actual)) => {
                    expected.eq_ignore_ascii_case(&actual) || event.code.is_some_and(|code| expected.eq_ignore_ascii_case(&code))
                }
                (expected, actual) => expected == actual,
            }
    }
}

impl FromStr for Chord {
    type Err = String;

    fn from_str(chord: &str) -> Result<Self, Self::Err> {
        let chord = chord.trim();
        let (mods, key) = match chord.strip_suffix("++") {
            Some(mods) => (mods, "+"),
            None if chord == "+" => ("", "+"),
            None => chord.rsplit_once('+').unwrap_or(("", chord)),
        };
        let mut modifiers = Modifiers::empty();
        let mut platform_mod = false;
        if !mods.is_empty() {
            for name in mods.split('+') {
                match name.to_ascii_lowercase().as_str() {
                    "mod" => platform_mod = true,
                    "cmd" | "command" | "meta" | "super" | "win" => modifiers |= Modifiers::META,
                    "ctrl" | "control" => modifiers |= Modifiers::CTRL,
                    "alt" | "option" | "opt" => modifiers |= Modifiers::ALT,
                    "shift" => modifiers |= Modifiers::SHIFT,
                    _ => return Err(format!("unknown modifier `{name}` in `{chord}`")),
                }
            }
        }

        let lower = key.to_ascii_lowercase();
        let key = match lower.as_str() {
            "tab" => Key::Tab,
            "enter" | "return" => Key::Enter,
            "escape" | "esc" => Key::Escape,
            "backspace" => Key::Backspace,
            "delete" | "del" => Key::Delete,
            "space" => Key::Space,
            "up" => Key::Up,
            "down" => Key::Down,
            "left" => Key::Left,
            "right" => Key::Right,
            "home" => Key::Home,
            "end" => Key::End,
            "pageup" => Key::PageUp,
            "pagedown" => Key::PageDown,
            "menu" => Key::Menu,
            _ => {
                if let Some(index) = lower.strip_prefix('f').and_then(|suffix| suffix.parse::<u8>().ok())
                    && (1..=24).contains(&index)
                {
                    Key::F(index)
                } else {
                    let mut chars = lower.chars();
                    match (chars.next(), chars.next()) {
                        (Some(c), None) => Key::Char(c),
                        _ => return Err(format!("unknown key `{key}` in `{chord}`")),
                    }
                }
            }
        };
        Ok(Self { key, modifiers, platform_mod })
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.platform_mod {
            f.write_str("mod+")?;
        }
        for (modifier, name) in
            [(Modifiers::META, "cmd+"), (Modifiers::CTRL, "ctrl+"), (Modifiers::ALT, "alt+"), (Modifiers::SHIFT, "shift+")]
        {
            if self.modifiers.contains(modifier) {
                f.write_str(name)?;
            }
        }
        match self.key {
            Key::Char(c) => c.fmt(f),
            Key::F(index) => write!(f, "f{index}"),
            key => f.write_str(&format!("{key:?}").to_ascii_lowercase()),
        }
    }
}

/// The phase of a wheel delta. Direct input and momentum have separate lifecycles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WheelPhase {
    #[default]
    Discrete,
    Direct(WheelStage),
    Momentum(WheelStage),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WheelStage {
    Start,
    Move,
    End,
    Cancel,
}

/// Phases of a platform-owned interactive back gesture.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackPhase {
    Started,
    /// Transition progress reported as a fraction from zero to one.
    Progress(f32),
    Cancelled,
    Completed,
}

/// Proposals from the host editor against one session and a known document revision.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditorEvent {
    ProposeEdit {
        session: EditorSessionId,
        expected_rev: u64,
        edit: TextEdit,
    },
    /// A directional anchor and focus in UTF-8 bytes of the displayed document.
    ProposeSelection {
        session: EditorSessionId,
        expected_rev: u64,
        selection: (usize, usize),
    },
    /// Preedit text and an optional UTF-8 selection within that text.
    Composition {
        session: EditorSessionId,
        expected_rev: u64,
        text: String,
        selection: Option<(usize, usize)>,
        replacement: Option<Range<usize>>,
    },
    /// Commit the host's text as the active composition's replacement.
    Commit {
        session: EditorSessionId,
        expected_rev: u64,
        text: String,
    },
    CancelComposition {
        session: EditorSessionId,
        expected_rev: u64,
    },
    /// Offer an editor key to app handlers before the host performs its default behavior.
    KeyIntercept {
        session: EditorSessionId,
        expected_rev: u64,
        key: KeyEvent,
    },
    /// Apply the engine default after a host declines its own editor command.
    KeyFallback {
        session: EditorSessionId,
        expected_rev: u64,
        key: KeyEvent,
    },
}

impl fmt::Debug for EditorEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::ProposeEdit { .. } => "ProposeEdit",
            Self::ProposeSelection { .. } => "ProposeSelection",
            Self::Composition { .. } => "Composition",
            Self::Commit { .. } => "Commit",
            Self::CancelComposition { .. } => "CancelComposition",
            Self::KeyIntercept { .. } => "KeyIntercept",
            Self::KeyFallback { .. } => "KeyFallback",
        };
        f.debug_struct(kind).field("session", &self.session()).field("expected_rev", &self.expected_rev()).finish_non_exhaustive()
    }
}

impl EditorEvent {
    pub fn session(&self) -> EditorSessionId {
        match *self {
            Self::ProposeEdit { session, .. }
            | Self::ProposeSelection { session, .. }
            | Self::Composition { session, .. }
            | Self::Commit { session, .. }
            | Self::CancelComposition { session, .. }
            | Self::KeyIntercept { session, .. }
            | Self::KeyFallback { session, .. } => session,
        }
    }

    pub fn expected_rev(&self) -> u64 {
        match *self {
            Self::ProposeEdit { expected_rev, .. }
            | Self::ProposeSelection { expected_rev, .. }
            | Self::Composition { expected_rev, .. }
            | Self::Commit { expected_rev, .. }
            | Self::CancelComposition { expected_rev, .. }
            | Self::KeyIntercept { expected_rev, .. }
            | Self::KeyFallback { expected_rev, .. } => expected_rev,
        }
    }
}

/// A file reference delivered by a host drop. The payload carries no file contents.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DroppedFile {
    /// A local path supplied by a native host.
    Path(PathBuf),
    /// A display name supplied by a browser.
    Name(String),
}

impl fmt::Debug for DroppedFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::Path(_) => "Path",
            Self::Name(_) => "Name",
        };
        f.debug_tuple(kind).field(&"<redacted>").finish()
    }
}

/// Window notifications and replies to requests, already associated with a receiving window.
///
/// `FilesDropped` targets the guarded node that registered a file-drop handler.
/// `Paste` offers clipboard data to the current focus and its logical ancestors.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostReply {
    ViewportChanged(WindowViewport),
    ChromeChanged(WindowChrome),
    GeometryChanged(WindowGeometry),
    SystemPreferencesChanged(SystemPreferences),
    CloseRequested,
    FocusChanged { target: Option<TargetGuard>, visible: bool },
    DocumentSelection { selection: Option<(TargetGuard, [crate::semantics::TextPosition; 2])> },
    ScrollChanged { target: TargetGuard, offset: Vec2, at_end: bool },
    FilesDropped { target: TargetGuard, files: Vec<DroppedFile> },
    Paste(PasteEvent),
    MenuCommand { name: String, chord: Option<Chord> },
    AskAnswer { id: AskId, answer: Answer },
    ServiceReply { id: ServiceId, outcome: ServiceOutcome },
    NativeEvent { target: TargetGuard, event: NativeEvent },
    NativeLive { target: TargetGuard, live: bool },
    NativeMeasurement { ticket: NativeMeasureTicket, size: Size },
    NativeSnapshot { request: NativeSnapshotRequest, snapshot: Result<NativeSnapshot, String> },
    ExitFinished(ExitTicket),
    AccessibilityAction { target: TargetGuard, action: A11yAction },
}

/// The cursor requested by the active target.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorStyle {
    #[default]
    Arrow,
    IBeam,
    PointingHand,
    Crosshair,
    OpenHand,
    ClosedHand,
    ResizeLeftRight,
    ResizeUpDown,
    ResizeColumn,
    ResizeRow,
    OperationNotAllowed,
    Auto,
    Wait,
    Move,
    Help,
    None,
    ContextMenu,
    Progress,
    Cell,
    VerticalText,
    Alias,
    Copy,
    NoDrop,
    AllScroll,
    ResizeNorth,
    ResizeEast,
    ResizeSouth,
    ResizeWest,
    ResizeNorthEast,
    ResizeNorthWest,
    ResizeSouthEast,
    ResizeSouthWest,
    ResizeNorthEastSouthWest,
    ResizeNorthWestSouthEast,
    ZoomIn,
    ZoomOut,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{NativeId, NodeId, TreeId};
    use crate::paint::Picture;
    use serde::de::DeserializeOwned;
    use slotmap::SlotMap;

    fn round_trip<T: fmt::Debug + PartialEq + Serialize + DeserializeOwned>(value: T) {
        assert_eq!(serde_json::from_str::<T>(&serde_json::to_string(&value).unwrap()).unwrap(), value);
    }

    #[test]
    fn all_channels_round_trip_with_versioned_targets() {
        let tree = TreeId::new([1, 2], 3);
        let node = SlotMap::<NodeId, ()>::with_key().insert(());
        let target = TargetGuard::for_node(tree, node);
        round_trip(InteractionEvent::PointerDown {
            id: PointerId(4),
            kind: PointerKind::Touch,
            target: ContactTarget { hit: target, action: Some(target) },
            pos: Point::new(5.0, 6.0),
            button: Button::Primary,
            mods: Modifiers::SHIFT,
        });
        let session = EditorSessionId { tree, node, session_gen: 7 };
        let event =
            EditorEvent::Composition { session, expected_rev: 8, text: "漢字".into(), selection: Some((6, 3)), replacement: Some(0..3) };
        assert_eq!(event.session(), session);
        assert_eq!(event.expected_rev(), 8);
        assert!(!format!("{event:?}").contains("漢字"));
        round_trip(event);
        round_trip(HostReply::NativeSnapshot {
            request: NativeSnapshotRequest { tree, id: NativeId::from(node), request: 9 },
            snapshot: Ok(NativeSnapshot { picture: Picture::new(1, 1, vec![50, 100, 150, 255]).unwrap(), misses: vec![] }),
        });
    }

    #[test]
    fn chord_preserves_the_literal_plus_key() {
        let chord: Chord = "mod+shift++".parse().unwrap();
        assert_eq!(chord.key, Key::Char('+'));
        assert!(chord.platform_mod);
        assert_eq!(chord.modifiers, Modifiers::SHIFT);
        assert_eq!(chord.to_string().parse::<Chord>().unwrap(), chord);
    }
}

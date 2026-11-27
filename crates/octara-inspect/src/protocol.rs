//! The octara.inspect/2 command and receipt schema.

use octara::runtime::RestartState;
use octara_model::{
    audit::AuditFinding,
    capture::{CaptureSource, PaintReceipt, PaintSample},
    diagnostics::{Code, Diagnostic},
    geometry::{LayoutBox, Point, Vec2, WindowViewport},
    identity::{EditorSessionId, TargetGuard, WindowId},
    input::{Button, EditorEvent, KeyEvent, Modifiers, PointerId, PointerKind},
    paint::Picture,
    semantics::{A11yAction, Checked, Relations, Role},
};
use serde::{Deserialize, Serialize};
use slotmap::Key as _;

pub const PROTOCOL: &str = "octara.inspect/2";
pub type TargetRef = TargetGuard;

/// The full allocation identity, including the generation of a reused window slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WindowRef(pub u64);

impl From<WindowId> for WindowRef {
    fn from(id: WindowId) -> Self {
        Self(id.data().as_ffi())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentRequest {
    pub protocol: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowRef>,
    #[serde(flatten)]
    pub command: AgentCommand,
}

impl AgentRequest {
    pub fn new(window: Option<WindowId>, command: AgentCommand) -> Self {
        Self { protocol: PROTOCOL.into(), window: window.map(WindowRef::from), command }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum AgentCommand {
    Observe,
    Audit,
    Act {
        target: TargetRef,
        action: AgentAction,
    },
    Capture {
        #[serde(default)]
        source: CaptureSource,
        #[serde(default)]
        settle_ms: u64,
    },
    AdvanceClock {
        ms: u64,
    },
    SetPicking {
        enabled: bool,
    },
    ListWindows,
    /// Export the application's explicitly persisted state from a dev host.
    ExportRestartState,
    /// Ask a dev host to exit through its normal shutdown path.
    Shutdown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentAction {
    Tap {
        pos: Point,
        #[serde(default)]
        modifiers: Modifiers,
    },
    PointerDown {
        id: PointerId,
        #[serde(default)]
        pointer: PointerKind,
        pos: Point,
        #[serde(default)]
        button: Button,
        #[serde(default)]
        modifiers: Modifiers,
    },
    PointerMove {
        id: PointerId,
        pos: Point,
        #[serde(default)]
        modifiers: Modifiers,
    },
    PointerUp {
        id: PointerId,
        pos: Point,
        #[serde(default)]
        modifiers: Modifiers,
    },
    PointerCancel {
        id: PointerId,
    },
    Wheel {
        pos: Point,
        delta: Vec2,
        #[serde(default)]
        modifiers: Modifiers,
    },
    Key {
        event: KeyEvent,
    },
    Editor {
        event: EditorEvent,
    },
    Accessibility {
        action: A11yAction,
    },
    MenuCommand {
        name: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    Delivered,
    Refused,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    pub observation_rev: u64,
    pub tree_rev: u64,
    pub settled: bool,
    pub committed_frame: Option<u64>,
    pub presented_frame: Option<u64>,
}

/// The executable identity published by a local launch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildIdentity {
    pub digest: String,
    pub pid: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentReceipt {
    pub protocol: String,
    pub ok: bool,
    pub delivery: Delivery,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowRef>,
    pub progress: Progress,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<BuildIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<TreeSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub findings: Option<Vec<AuditFinding>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture: Option<Capture>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub windows: Option<Vec<WindowInfo>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_state: Option<RestartState>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

impl AgentReceipt {
    /// Report a refusal before dispatch, such as a client-side build precondition.
    pub fn refused(code: &Code, message: impl Into<String>) -> Self {
        let mut receipt = Self::success(None);
        receipt.fail(code, message, false);
        receipt
    }

    pub(crate) fn success(window: Option<WindowRef>) -> Self {
        Self {
            protocol: PROTOCOL.into(),
            ok: true,
            delivery: Delivery::Delivered,
            window,
            progress: Progress::default(),
            build: None,
            code: None,
            message: None,
            handled: None,
            snapshot: None,
            findings: None,
            capture: None,
            windows: None,
            restart_state: None,
            diagnostics: Vec::new(),
        }
    }

    pub(crate) fn fail(&mut self, code: &Code, message: impl Into<String>, delivered: bool) {
        self.ok = false;
        self.delivery = if delivered { Delivery::Delivered } else { Delivery::Refused };
        self.code = Some(code.id.into());
        self.message = Some(message.into());
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowInfo {
    pub id: WindowRef,
    pub target: TargetRef,
    pub title: String,
    pub viewport: WindowViewport,
    pub progress: Progress,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TreeSnapshot {
    pub viewport: WindowViewport,
    pub target: TargetRef,
    pub focus: Option<TargetRef>,
    pub modal: Option<TargetRef>,
    pub picking: bool,
    pub nodes: Vec<NodeSnapshot>,
    pub editor: Option<EditorSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeSnapshot {
    pub target: TargetRef,
    pub parent: Option<TargetRef>,
    pub test_id: Option<String>,
    pub role: Option<Role>,
    pub label: Option<String>,
    pub checked: Option<Checked>,
    pub relations: Relations<TargetRef>,
    pub value: Option<String>,
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_props: Option<serde_json::Value>,
    pub geometry: Option<LayoutBox>,
    pub scroll_offset: Option<Vec2>,
    pub shown: bool,
    pub interactive: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorSnapshot {
    pub session: EditorSessionId,
    pub document_rev: u64,
    pub secure: bool,
    pub value: Option<String>,
    pub selection: Option<(usize, usize)>,
    pub marked: Option<std::ops::Range<usize>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capture {
    pub source: CaptureSource,
    pub sample: PaintSample,
    pub picture: Picture,
    pub effects: Vec<PaintReceipt>,
}

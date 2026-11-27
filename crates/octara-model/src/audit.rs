//! Host-independent inputs and findings for an inspection audit.

use crate::{
    diagnostics::Severity,
    geometry::LayoutBox,
    identity::TargetGuard,
    semantics::{Role, TextClip},
};
use serde::{Deserialize, Serialize};
use std::{fmt, panic::Location};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditProvenance {
    pub file: String,
    pub line: u32,
    pub column: u32,
}

impl From<&Location<'_>> for AuditProvenance {
    fn from(location: &Location<'_>) -> Self {
        Self { file: location.file().into(), line: location.line(), column: location.column() }
    }
}

impl fmt::Display for AuditProvenance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.file, self.line)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditFinding {
    /// A built-in diagnostic ID or a code chosen by an external rule.
    pub code: String,
    pub severity: Severity,
    pub node: TargetGuard,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<AuditProvenance>,
    pub message: String,
}

/// Settled, read-only facts for a live node. Editor contents are excluded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AuditNode<'a> {
    pub node: TargetGuard,
    pub parent: Option<TargetGuard>,
    pub provenance: Option<&'static Location<'static>>,
    pub test_id: Option<&'a str>,
    pub role: Option<Role>,
    pub accessible_name: Option<&'a str>,
    pub text: Option<&'a str>,
    pub geometry: Option<LayoutBox>,
    pub text_clip: Option<TextClip>,
    pub shown: bool,
    pub hidden_from_accessibility: bool,
    pub inert: bool,
    pub in_modal_scope: bool,
    pub authored_interactive: bool,
    pub authored_focusable: bool,
    pub focused: bool,
}

impl AuditNode<'_> {
    pub fn finding(&self, code: impl Into<String>, severity: Severity, message: impl Into<String>) -> AuditFinding {
        AuditFinding {
            code: code.into(),
            severity,
            node: self.node,
            provenance: self.provenance.map(AuditProvenance::from),
            message: message.into(),
        }
    }
}

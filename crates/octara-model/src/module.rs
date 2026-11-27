//! Platform-independent values for Octara modules and host lifecycle events.

use crate::identity::WindowId;
use serde::{Deserialize, Serialize};
use std::{fmt, path::PathBuf};

/// An identifier declared by a module, such as `octara.webview`.
///
/// Deserialization borrows from static input. For dynamic input, decode a
/// string and resolve it against the application's registered module IDs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ModuleId(pub &'static str);

/// Identifies a pending module call within an application.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CallTicket(pub u64);

/// Identifies a module event subscription within an application.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SubscriptionId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostSurfaceKind {
    MacOs,
    Ios,
    Windows,
    Linux,
    Web,
    Headless,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionStatus {
    Granted,
    Denied,
    NotDetermined,
    Restricted,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleError {
    Unavailable,
    PermissionDenied,
    ActivationRequired,
    Busy,
    Cancelled,
    InvalidArgs(String),
    Failed(String),
}

impl fmt::Display for ModuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => f.write_str("module is unavailable"),
            Self::PermissionDenied => f.write_str("permission denied"),
            Self::ActivationRequired => f.write_str("a user gesture is required"),
            Self::Busy => f.write_str("module is busy"),
            Self::Cancelled => f.write_str("module call cancelled"),
            Self::InvalidArgs(message) => write!(f, "invalid module arguments: {message}"),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ModuleError {}

/// Controls how native views compose with Octara overlays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AirspacePolicy {
    /// Mount below transparent Octara pixels and route covered hits to Octara.
    #[default]
    UnderlayPreferred,
    /// Use a snapshot while an Octara overlay covers the live native view.
    SnapshotOnOcclude,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostLifecycleEvent {
    AppLaunched { urls: Vec<String>, files: Vec<PathBuf> },
    AppForeground,
    AppBackground,
    MemoryWarning,
    OpenUrls(Vec<String>),
    OpenFiles(Vec<PathBuf>),
    DockReopen { has_visible_windows: bool },
    WindowAttached { window: WindowId },
    WindowDetached { window: WindowId },
    WillTerminate,
}

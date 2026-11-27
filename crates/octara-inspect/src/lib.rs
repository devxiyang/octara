//! Guarded agent input, deterministic replay, and host-independent inspection.

pub mod audit;
pub mod conformance;
pub mod protocol;
pub mod replay;
pub mod transport;

pub use audit::{AuditFinding, AuditNode, AuditProvenance, AuditRule, ClippedText, DeadFocusTarget, MissingSemantics};
pub use replay::TraceRecorder;
pub use transport::codec::{InspectTransport, JsonLines};

mod inspector;
pub use inspector::{InspectHost, Inspector};

#[cfg(all(feature = "native", not(target_arch = "wasm32")))]
pub mod driver;
#[cfg(all(feature = "native", not(target_arch = "wasm32")))]
pub mod dst;
#[cfg(all(feature = "native", not(target_arch = "wasm32")))]
mod module;
#[cfg(all(feature = "native", not(target_arch = "wasm32")))]
pub use driver::HeadlessDriver;

pub use protocol::{AgentAction, AgentCommand, AgentReceipt, AgentRequest, TargetRef};

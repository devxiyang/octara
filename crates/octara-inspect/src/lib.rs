//! Guarded agent input, deterministic replay, and host-independent inspection.

pub mod conformance;
pub mod protocol;
pub mod replay;
pub mod transport;

mod inspector;
pub use inspector::{InspectHost, Inspector};

#[cfg(all(feature = "native", not(target_arch = "wasm32")))]
pub mod driver;
#[cfg(all(feature = "native", not(target_arch = "wasm32")))]
pub mod dst;
#[cfg(all(feature = "native", not(target_arch = "wasm32")))]
pub use driver::HeadlessDriver;

pub use protocol::{AgentAction, AgentCommand, AgentReceipt, AgentRequest, TargetRef};

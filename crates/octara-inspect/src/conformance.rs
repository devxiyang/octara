//! Assertions clients can share across headless, native, and browser inspection hosts.

use crate::protocol::{AgentCommand, AgentReceipt, AgentRequest, Delivery, PROTOCOL};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContractError(pub &'static str);

impl fmt::Display for ContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for ContractError {}

/// Validate a receipt's wire contract without assuming a specific presentation backend.
pub fn verify_receipt(request: &AgentRequest, receipt: &AgentReceipt) -> Result<(), ContractError> {
    if receipt.protocol != PROTOCOL {
        return Err(ContractError("unexpected receipt protocol"));
    }
    if request.window.is_some() && request.window != receipt.window {
        return Err(ContractError("receipt identifies a different window"));
    }
    if let (Some(committed), Some(presented)) = (receipt.progress.committed_frame, receipt.progress.presented_frame)
        && presented > committed
    {
        return Err(ContractError("presented frame is newer than committed frame"));
    }
    if !receipt.ok {
        if receipt.code.as_ref().is_none_or(String::is_empty) {
            return Err(ContractError("refused or failed command has no diagnostic code"));
        }
        return Ok(());
    }
    if receipt.delivery != Delivery::Delivered || receipt.code.is_some() {
        return Err(ContractError("successful command has an inconsistent delivery or code"));
    }
    match &request.command {
        AgentCommand::Observe if !receipt.progress.settled || receipt.snapshot.is_none() => {
            Err(ContractError("observe succeeded without a settled snapshot"))
        }
        AgentCommand::Audit if !receipt.progress.settled || receipt.findings.is_none() => {
            Err(ContractError("audit succeeded without settled findings"))
        }
        AgentCommand::ListWindows if receipt.windows.is_none() => Err(ContractError("list_windows succeeded without a window list")),
        AgentCommand::ExportRestartState if receipt.restart_state.is_none() => {
            Err(ContractError("export_restart_state succeeded without persisted state"))
        }
        AgentCommand::Act { .. } if receipt.handled.is_none() => Err(ContractError("act succeeded without a handling result")),
        AgentCommand::Capture { source, .. } if receipt.capture.as_ref().is_none_or(|capture| capture.source != *source) => {
            Err(ContractError("capture succeeded without the requested source"))
        }
        _ => Ok(()),
    }
}

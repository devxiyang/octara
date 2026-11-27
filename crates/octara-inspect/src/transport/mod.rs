//! JSON-lines framing shared by the terminal and browser bridges.

use crate::protocol::{AgentReceipt, AgentRequest, Delivery};
use octara_model::diagnostics::codes;

#[cfg(unix)]
pub mod unix;
#[cfg(target_arch = "wasm32")]
pub mod web;

const REQUEST_LIMIT: usize = 1024 * 1024;
const RESPONSE_LIMIT: usize = 32 * 1024 * 1024;

/// Exchange one request line with the application on its owning thread.
pub trait InspectTransport {
    fn exchange(&mut self, line: &[u8]) -> String;
}

impl<F: FnMut(AgentRequest) -> AgentReceipt> InspectTransport for F {
    fn exchange(&mut self, line: &[u8]) -> String {
        exchange(line, self)
    }
}

fn exchange(line: &[u8], execute: &mut dyn FnMut(AgentRequest) -> AgentReceipt) -> String {
    if line.len() > REQUEST_LIMIT {
        return malformed("request exceeds the line size limit");
    }
    let request = match serde_json::from_slice(line) {
        Ok(request) => request,
        Err(_) => return malformed("expected one JSON request per line"),
    };
    let mut receipt = execute(request);
    let reason = match serde_json::to_string(&receipt) {
        Ok(reply) if reply.len() < RESPONSE_LIMIT => return reply + "\n",
        Ok(_) => "reply exceeds the line size limit",
        Err(_) => "reply cannot be encoded",
    };
    receipt.fail(&codes::INSPECT_RESPONSE, reason, receipt.delivery == Delivery::Delivered);
    receipt.snapshot = None;
    receipt.capture = None;
    receipt.windows = None;
    receipt.restart_state = None;
    receipt.diagnostics.clear();
    serde_json::to_string(&receipt).expect("receipt without payloads is serializable") + "\n"
}

fn malformed(message: &str) -> String {
    let mut receipt = AgentReceipt::success(None);
    receipt.fail(&codes::INSPECT_REQUEST, message, false);
    serde_json::to_string(&receipt).expect("a transport diagnostic is serializable") + "\n"
}

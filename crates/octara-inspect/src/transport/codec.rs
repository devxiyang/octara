//! Wire codecs for executing typed inspection requests on the owning thread.

use super::REQUEST_LIMIT;
use crate::{AgentReceipt, AgentRequest, protocol::Delivery};
use octara_model::diagnostics::codes;

const RESPONSE_LIMIT: usize = 32 * 1024 * 1024;

/// Decode a complete payload, dispatch typed requests, and encode the resulting receipts.
/// The host continues to own framing across reads and scheduling on the app thread.
pub trait InspectTransport {
    fn exchange(&mut self, payload: &[u8], execute: &mut dyn FnMut(AgentRequest) -> AgentReceipt) -> Vec<u8>;
}

/// A single JSON request and newline-terminated JSON receipt, with bounded payloads.
#[derive(Clone, Copy, Debug, Default)]
pub struct JsonLines;

impl InspectTransport for JsonLines {
    fn exchange(&mut self, payload: &[u8], execute: &mut dyn FnMut(AgentRequest) -> AgentReceipt) -> Vec<u8> {
        Self::exchange_line(payload, execute).into_bytes()
    }
}

impl JsonLines {
    pub(super) fn exchange_line(line: &[u8], execute: &mut dyn FnMut(AgentRequest) -> AgentReceipt) -> String {
        if line.len() > REQUEST_LIMIT {
            return Self::malformed("request exceeds the line size limit");
        }
        let request = match serde_json::from_slice(line) {
            Ok(request) => request,
            Err(_) => return Self::malformed("expected one JSON request per line"),
        };
        let mut receipt = execute(request);
        let reason = match serde_json::to_string(&receipt) {
            Ok(reply) if reply.len() < RESPONSE_LIMIT => return reply + "\n",
            Ok(_) => "reply exceeds the line size limit",
            Err(_) => "reply cannot be encoded",
        };
        receipt.fail(&codes::INSPECT_RESPONSE, reason, receipt.delivery == Delivery::Delivered);
        receipt.snapshot = None;
        receipt.findings = None;
        receipt.capture = None;
        receipt.windows = None;
        receipt.restart_state = None;
        receipt.diagnostics.clear();
        serde_json::to_string(&receipt).expect("receipt without payloads is serializable") + "\n"
    }

    pub(super) fn malformed(message: &str) -> String {
        let receipt = AgentReceipt::refused(&codes::INSPECT_REQUEST, message);
        serde_json::to_string(&receipt).expect("a transport diagnostic is serializable") + "\n"
    }
}

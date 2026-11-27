//! JSON-lines framing shared by the terminal and browser bridges.

use crate::protocol::{AgentReceipt, AgentRequest};

pub mod codec;

#[cfg(unix)]
pub mod unix;
#[cfg(target_arch = "wasm32")]
pub mod web;

const REQUEST_LIMIT: usize = 1024 * 1024;
/// Exchange one request line with the application on its owning thread.
/// To plug a byte-oriented codec into an Inspector, use [`crate::InspectTransport`].
pub trait InspectTransport {
    fn exchange(&mut self, line: &[u8]) -> String;
}

impl<F: FnMut(AgentRequest) -> AgentReceipt> InspectTransport for F {
    fn exchange(&mut self, line: &[u8]) -> String {
        codec::JsonLines::exchange_line(line, self)
    }
}

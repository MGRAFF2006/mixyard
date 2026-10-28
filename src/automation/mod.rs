//! Pure, bounded situation decisions. Observations are supplied, never discovered.
//! A selection is a proposal; profile conflict checks, consent and the planner remain authoritative.
mod controller;
mod evaluate;
mod model;
mod proposal;
mod validate;

pub use controller::{
    Controller, ControllerDecision, ManualOverride, TransitionCandidate, TransitionPolicy,
};
pub use evaluate::evaluate;
pub use model::*;
pub use proposal::{preview_proposal, Proposal};
pub use validate::validate;

/// Fixed format-v1 limits also apply to Rust callers (not only JSON inputs).
pub const MAX_NODES: usize = 256;
pub const MAX_DEPTH: usize = 32;
pub const MAX_WORK: usize = 4096;
pub const MAX_FACTS: usize = 512;
pub const MAX_ITEMS: usize = 256;
pub const MAX_TEXT_BYTES: usize = 1024;
pub const MAX_TRACE: usize = 256;
pub const MAX_TRACE_BYTES: usize = 256 * 1024;
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Error {
    pub code: &'static str,
    pub message: String,
}
impl Error {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Error {}

/// Bounded strict JSON entry point. Serde's recursion limit stays enabled.
pub fn parse_request(bytes: &[u8]) -> Result<Request, Error> {
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(Error::new("resource-limit", "request exceeds 1 MiB"));
    }
    serde_json::from_slice(bytes).map_err(|e| Error::new("invalid-json", e.to_string()))
}

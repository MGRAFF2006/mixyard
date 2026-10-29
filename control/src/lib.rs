//! Explicitly opted-in encrypted management. No audio transport or native activation.
mod identity;
mod server;
#[cfg(not(windows))]
#[path = "../../src/store_fs.rs"]
mod store_fs;
#[path = "../../src/web/mod.rs"]
// Retain the root web-service's existing allowance for bounded HTTP replies.
#[allow(dead_code, clippy::result_large_err)]
mod web;

pub use identity::{AudioIdentityMaterial, IdentityStore, Permission, Trust};
pub use server::{Control, Options};

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Failure {
    pub code: &'static str,
    pub message: String,
    #[serde(skip)]
    pub status: u16,
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Failure {}
#[cfg(not(windows))]
impl From<store_fs::Error> for Failure {
    fn from(e: store_fs::Error) -> Self {
        fail(503, "identity-store-unavailable", e.to_string())
    }
}
impl From<std::io::Error> for Failure {
    fn from(e: std::io::Error) -> Self {
        fail(503, "identity-store-unavailable", e.to_string())
    }
}
pub(crate) fn fail(status: u16, code: &'static str, message: impl Into<String>) -> Failure {
    Failure {
        status,
        code,
        message: message.into(),
    }
}
pub(crate) type Result<T> = std::result::Result<T, Failure>;

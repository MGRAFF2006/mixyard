//! Platform-neutral read-only provider boundary. No caller can submit observations over IPC.
use crate::Inventory;
use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, &'static str>;
pub const MAX_ENTRIES: usize = 1024;
pub const MAX_ID_BYTES: usize = 4096;

/// Trusted local launch configuration, never populated from HTTP or IPC fields.
#[derive(Clone, Default)]
pub struct Options {
    /// An owned test executable. Its observations always retain simulated evidence.
    pub fixture_helper: Option<PathBuf>,
}

/// One bounded discovery. Implementations must cancel and reap owned children on drop.
/// Polling must not wait for subprocess I/O. Concrete formats and selectors are
/// translated by the provider; unknown formats stay absent. Revision starts at zero.
pub trait Capture {
    fn poll(&mut self) -> Result<Option<Inventory>>;
}

/// Shared admission for all platform translators and typed IPC consumers.
pub(super) fn validate(inventory: &Inventory) -> Result<()> {
    if [
        inventory.sources.len(),
        inventory.outputs.len(),
        inventory.media_sessions.len(),
    ]
    .iter()
    .any(|n| *n > MAX_ENTRIES)
    {
        return Err("inventory-exceeds-1024-entries-per-category-reduce-visible-graph");
    }
    let value = serde_json::to_value(inventory).map_err(|_| "inventory-serialization-failed")?;
    fn text(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::String(s) => {
                s.len() <= MAX_ID_BYTES && !s.chars().any(char::is_control)
            }
            serde_json::Value::Array(a) => a.iter().all(text),
            serde_json::Value::Object(o) => o.values().all(text),
            _ => true,
        }
    }
    if !text(&value) {
        return Err("inventory-identity-exceeds-4096-bytes-or-invalid");
    }
    let empty = crate::Config {
        schema_version: 2,
        revision: 0,
        sources: vec![],
        mixes: vec![],
        sends: vec![],
        outputs: vec![],
        media_controls: vec![],
    };
    crate::plan(&empty, inventory).map_err(|_| "inventory-invalid-schema-or-observations")?;
    Ok(())
}

pub(super) fn start(options: &Options) -> Result<Box<dyn Capture>> {
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(super::discovery::Job::start(options)?))
    }
    #[cfg(any(target_os = "macos", windows))]
    {
        let platform = if cfg!(windows) {
            crate::Platform::Windows
        } else {
            crate::Platform::Macos
        };
        Ok(Box::new(super::native_inventory::Job::start(
            options, platform,
        )?))
    }
}

pub(super) fn complete(instance: &str, inventory: &mut Inventory) -> Result<()> {
    validate(inventory)?;
    #[cfg(target_os = "linux")]
    {
        let endpoint = super::endpoint(instance).map_err(|_| "inventory-invalid-namespace")?;
        super::discovery::revision(inventory, std::path::Path::new(&endpoint).parent().unwrap())
    }
    #[cfg(any(target_os = "macos", windows))]
    {
        let directory = super::inventory_os::state_directory(instance)?;
        super::inventory_state::revision(inventory, &directory)
    }
}

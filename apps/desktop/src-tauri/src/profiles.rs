//! Read-only current-user local drafts. No frontend path or mutation command.
use mixyard::{
    profiles::{self, Snapshot, Store},
    Config, Inventory,
};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

pub const MAX_STORE_BYTES: usize = 262_144;
const MAX_INPUT_BYTES: usize = 131_072;
const MAX_REQUEST_BYTES: usize = 262_144;
const MAX_RESPONSE_BYTES: usize = 2_097_152;
const MAX_SAFE_REVISION: u64 = 9_007_199_254_740_991;
static BUSY: AtomicBool = AtomicBool::new(false);

#[derive(Deserialize)]
#[serde(tag = "command", rename_all = "camelCase", deny_unknown_fields)]
pub enum Request {
    List {},
    #[serde(rename_all = "camelCase")]
    Preview {
        store_revision: u64,
        profile_id: String,
        inventory_json: String,
    },
}

pub struct Admission;
impl Drop for Admission {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}
pub fn acquire() -> Result<Admission, String> {
    BUSY.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .map(|_| Admission)
        .map_err(|_| {
            "profile-busy: A profile request is already running. Wait, then Refresh.".into()
        })
}

struct Reply(Vec<u8>);
impl Write for Reply {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_RESPONSE_BYTES - self.0.len() {
            return Err(io::Error::other(
                "Profile reply exceeds 2 MiB; nothing was truncated. Use the profiles CLI.",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn encode(value: &impl Serialize) -> Result<String, String> {
    let mut reply = Reply(Vec::new());
    serde_json::to_writer(&mut reply, value).map_err(|e| format!("profile-reply-rejected: {e}"))?;
    String::from_utf8(reply.0).map_err(|e| format!("profile-encoding: {e}"))
}
fn exact(revision: u64) -> Result<(), String> {
    if revision > MAX_SAFE_REVISION {
        return Err("profile-revision-range: Revision exceeds the desktop's exact integer range (0–9007199254740991). Use the profiles CLI.".into());
    }
    Ok(())
}
fn config_bounds(c: &Config) -> Result<(), String> {
    exact(c.revision)?;
    if c.sources.len() > 32
        || c.mixes.len() > 32
        || c.outputs.len() > 32
        || c.sends.len() > 256
        || c.media_controls.len() > 64
    {
        return Err("profile-work-limit: Config exceeds desktop bounds (32 sources/mixes/outputs, 256 sends, 64 policies). Nothing was truncated; use the profiles CLI.".into());
    }
    Ok(())
}
fn snapshot_bounds(s: &Snapshot) -> Result<(), String> {
    exact(s.store_revision)?;
    // Includes the private core high-water mark needed by preview's next revision.
    let value = serde_json::to_value(s).map_err(|e| e.to_string())?;
    exact(
        value["configRevision"]
            .as_u64()
            .ok_or("Missing config revision context")?,
    )?;
    if s.profiles.len() > 64 {
        return Err("profile-work-limit: Store exceeds 64 desktop profiles. Nothing was truncated; use the profiles CLI.".into());
    }
    for c in s
        .profiles
        .iter()
        .map(|p| &p.config)
        .chain(s.draft.iter())
        .chain(s.previous_draft.iter())
    {
        config_bounds(c)?;
    }
    Ok(())
}

pub fn run(request: Request) -> Result<String, String> {
    // The environment belongs to the native process/current user, never the webview.
    run_at(
        request,
        &profiles::default_directory().map_err(|e| e.to_string())?,
    )
}
fn run_at(request: Request, directory: &Path) -> Result<String, String> {
    if let Request::Preview {
        store_revision,
        profile_id,
        inventory_json,
    } = &request
    {
        exact(*store_revision)?;
        if profile_id.is_empty()
            || profile_id.len().saturating_add(inventory_json.len()) > MAX_REQUEST_BYTES
            || inventory_json.len() > MAX_INPUT_BYTES
        {
            return Err("profile-input-limit: Expected a stable profile ID and inventory of at most 128 KiB.".into());
        }
    }
    let snapshot = Store::new(directory)
        .and_then(|s| s.read_bounded(MAX_STORE_BYTES))
        .map_err(|e| e.to_string())?;
    snapshot_bounds(&snapshot)?;
    // Core reads allow a valid prior snapshot during a cooperative write. The
    // desktop conservatively asks for Refresh after writer/recovery markers clear.
    for name in ["write.lock", "pending.json"] {
        match std::fs::symlink_metadata(directory.join(name)) {
            Ok(_) => return Err("store-busy: Writer or interrupted-write marker exists. Preserve the store; use the documented CLI recovery procedure. Nothing was repaired.".into()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {},
            Err(e) => return Err(format!("io-error: {e}")),
        }
    }
    let mut result = serde_json::json!({
        "schemaVersion": 1, "mode": "local-profile-preview", "command": "list",
        "provenance": "current-user-local-store", "freshness": "read-snapshot",
        "storeRevision": snapshot.store_revision, "audioApplied": false,
        "confirmedWorkingSetup": null, "storedConsentIsFreshNativeConsent": false,
    });
    match request {
        Request::List {} => {
            let present = match std::fs::symlink_metadata(directory.join("profiles.json")) {
                Ok(_) => true,
                Err(e) if e.kind() == io::ErrorKind::NotFound => false,
                Err(e) => return Err(format!("io-error: {e}")),
            };
            result["storeState"] = serde_json::json!(if present { "present" } else { "missing" });
            result["profiles"] =
                serde_json::to_value(&snapshot.profiles).map_err(|e| e.to_string())?;
            result["draft"] = serde_json::to_value(&snapshot.draft).map_err(|e| e.to_string())?;
        }
        Request::Preview {
            store_revision,
            profile_id,
            inventory_json,
        } => {
            if store_revision != snapshot.store_revision {
                return Err(format!("store-conflict: Selected store revision {store_revision}, now {}. Refresh and select the profile again.", snapshot.store_revision));
            }
            let profile = snapshot.profile(&profile_id).map_err(|e| e.to_string())?;
            let inventory: Inventory = serde_json::from_str(&inventory_json)
                .map_err(|e| format!("inventory-json-rejected: {e}"))?;
            if inventory.schema_version != 2 {
                return Err("inventory-schema: Import schema-2 inventory; discovery envelope 1 is not an inventory.".into());
            }
            exact(inventory.revision)?;
            if inventory.sources.len() > 64
                || inventory.outputs.len() > 64
                || inventory.media_sessions.len() > 128
            {
                return Err("profile-work-limit: Inventory exceeds 64 source/output observations or 128 sessions. Nothing was truncated; use the profiles CLI.".into());
            }
            let preview = profiles::preview(&snapshot, &profile.config, &inventory)
                .map_err(|e| e.to_string())?;
            exact(preview.plan.config_revision)?;
            result["command"] = serde_json::json!("preview");
            result["profileId"] = serde_json::json!(profile_id);
            result["inventoryRevision"] = serde_json::json!(inventory.revision);
            result["draftRevision"] =
                serde_json::json!(snapshot.draft.as_ref().map(|c| c.revision));
            result["preview"] = serde_json::to_value(preview).map_err(|e| e.to_string())?;
        }
    }
    encode(&result)
}

#[cfg(test)]
#[path = "profiles_tests.rs"]
mod tests;

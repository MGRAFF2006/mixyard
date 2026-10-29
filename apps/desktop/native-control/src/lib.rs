//! Native paired management. Credentials never cross the webview boundary.
mod tls;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
pub use tls::Reply;

const SERVICE: &str = "org.mixyard.desktop.paired.v1";
const INDEX: &str = "paired-device-index";
const MAX_DEVICES: usize = 16;
const SAFE_REVISION: u64 = (1u64 << 53) - 1;

pub trait Vault: Send + Sync {
    fn read(&self, key: &str) -> Result<Option<String>, String>;
    fn write(&self, key: &str, value: &str) -> Result<(), String>;
    fn delete(&self, key: &str) -> Result<(), String>;
}
pub struct PlatformVault {
    // No secret is stored here. The OS releases this advisory owner on process exit.
    owner: Result<std::fs::File, String>,
}
impl PlatformVault {
    pub fn open(directory: Result<std::path::PathBuf, String>) -> Self {
        let owner = (|| {
            let directory = directory?;
            std::fs::create_dir_all(&directory)
                .map_err(|_| "Cannot create paired-management owner directory")?;
            let path = directory.join("paired-management.lock");
            if std::fs::symlink_metadata(&path)
                .is_ok_and(|m| !m.is_file() || m.file_type().is_symlink())
            {
                return Err("Paired-management owner path is not a regular file".into());
            }
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(path)
                .map_err(|_| "Cannot open paired-management owner lock")?;
            file.try_lock().map_err(|_| "Another desktop process owns paired management. Close it before using saved devices here.")?;
            Ok(file)
        })();
        Self { owner }
    }
    fn available(&self) -> Result<(), String> {
        self.owner.as_ref().map(|_| ()).map_err(Clone::clone)
    }
}
impl Vault for PlatformVault {
    fn read(&self, key: &str) -> Result<Option<String>, String> {
        self.available()?;
        match keyring::Entry::new(SERVICE, key)
            .map_err(vault_error)?
            .get_password()
        {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(vault_error(e)),
        }
    }
    fn write(&self, key: &str, value: &str) -> Result<(), String> {
        self.available()?;
        keyring::Entry::new(SERVICE, key)
            .map_err(vault_error)?
            .set_password(value)
            .map_err(vault_error)
    }
    fn delete(&self, key: &str) -> Result<(), String> {
        self.available()?;
        match keyring::Entry::new(SERVICE, key)
            .map_err(vault_error)?
            .delete_credential()
        {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(vault_error(e)),
        }
    }
}
fn vault_error(_: keyring::Error) -> String {
    "Secure credential vault unavailable or locked. Unlock Secret Service / Windows Credential Manager / macOS Keychain and retry. No plaintext fallback is used.".into()
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Invitation {
    pub device_id: String,
    pub certificate_pem: String,
    pub certificate_sha256: String,
    pub invitation: String,
    pub permissions: Vec<String>,
    // Public provisioning fields emitted by the control CLI.
    pub server_name: Option<String>,
    pub expires_at_ms: Option<u64>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Target {
    pub device_id: String,
    pub display_name: String,
    pub endpoint: String,
    pub certificate_sha256: String,
    pub credential_id: String,
    pub permissions: Vec<String>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Stored {
    target: Target,
    certificate_pem: String,
    credential: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Session {
    device_id: String,
    authority_id: String,
    revision: u64,
    permissions: Vec<String>,
    backend_available: bool,
    audio_applied: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PairReply {
    device_id: String,
    credential_id: String,
    credential: String,
    permissions: Vec<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Envelope {
    device_id: String,
    authority_id: String,
    revision: u64,
    path: String,
    #[allow(dead_code)]
    body: Box<RawValue>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScopedReply {
    device_id: String,
    authority_id: String,
    revision: u64,
    status: u16,
    #[allow(dead_code)]
    body: Box<RawValue>,
}
#[derive(Serialize)]
pub struct OpenReply {
    pub generation: u64,
    pub status: u16,
    pub body: String,
}
struct Host {
    generation: AtomicU64,
    operation: Mutex<()>,
    session: Mutex<Option<Session>>,
}
impl Host {
    fn new() -> Self {
        Self {
            generation: AtomicU64::new(0),
            operation: Mutex::new(()),
            session: Mutex::new(None),
        }
    }
}

pub struct Paired<V: Vault> {
    vault: V,
    storage: Mutex<()>,
    hosts: Mutex<BTreeMap<String, Arc<Host>>>,
    active: AtomicUsize,
}
impl<V: Vault> Paired<V> {
    pub fn new(vault: V) -> Self {
        Self {
            vault,
            storage: Mutex::new(()),
            hosts: Mutex::new(BTreeMap::new()),
            active: AtomicUsize::new(0),
        }
    }
    fn host(&self, id: &str) -> Result<Arc<Host>, String> {
        hex_id(id)?;
        let mut hosts = self.hosts.lock().map_err(|_| "Paired state unavailable")?;
        if !hosts.contains_key(id) && hosts.len() >= MAX_DEVICES {
            return Err(
                "16 paired device sessions maximum; restart after removing unused devices".into(),
            );
        }
        Ok(hosts
            .entry(id.into())
            .or_insert_with(|| Arc::new(Host::new()))
            .clone())
    }
    fn ids(&self) -> Result<Vec<String>, String> {
        let ids: Vec<String> = match self.vault.read(INDEX)? {
            None => Vec::new(),
            Some(text) => {
                if text.len() > 4096 {
                    return Err("Secure paired-device index exceeds its bound".into());
                }
                decode(&text)?
            }
        };
        if ids.len() > MAX_DEVICES
            || ids.iter().any(|id| hex_id(id).is_err())
            || ids.iter().collect::<std::collections::BTreeSet<_>>().len() != ids.len()
        {
            return Err("Secure paired-device index is corrupt; no records were changed".into());
        }
        // A failed index update after successful credential deletion can leave an
        // index tombstone. Missing records grant no authority and do not hide siblings.
        let mut present = Vec::new();
        for id in ids {
            if self.vault.read(&id)?.is_some() {
                present.push(id);
            }
        }
        Ok(present)
    }
    fn stored(&self, id: &str) -> Result<Stored, String> {
        hex_id(id)?;
        let text = self
            .vault
            .read(id)?
            .ok_or("Paired credential missing; pair again with a new invitation")?;
        if text.len() > 2400 {
            return Err("Secure paired-device record exceeds its bound".into());
        }
        let stored: Stored = decode(&text)?;
        if stored.target.device_id != id {
            return Err("Vault device identity mismatch".into());
        }
        tls::endpoint(&stored.target.endpoint)?;
        hex_id(&stored.credential)?;
        roles(&stored.target.permissions)?;
        Ok(stored)
    }
    pub fn list(&self) -> Result<Vec<Target>, String> {
        let _admission = self.admit()?;
        let _storage = self.storage.lock().map_err(|_| "Vault operation failed")?;
        self.ids()?
            .iter()
            .map(|id| self.stored(id).map(|record| record.target))
            .collect()
    }
    pub fn pair(
        &self,
        endpoint: String,
        display_name: String,
        invite: Invitation,
    ) -> Result<Target, String> {
        let _admission = self.admit()?;
        let address = tls::endpoint(&endpoint)?;
        label(&display_name)?;
        hex_id(&invite.device_id)?;
        hex_id(&invite.invitation)?;
        roles(&invite.permissions)?;
        if invite.certificate_pem.len() > 4096
            || invite
                .server_name
                .as_deref()
                .is_some_and(|v| v != "mixyard-control")
        {
            return Err("Unsupported provisioning certificate or server name".into());
        }
        let config = tls::configuration(&invite.certificate_pem, &invite.certificate_sha256)?;
        let _storage = self.storage.lock().map_err(|_| "Vault operation failed")?;
        let mut ids = self.ids()?;
        if ids.contains(&invite.device_id) {
            return Err(
                "Device already paired; disconnect and remove it before pairing again".into(),
            );
        }
        if ids.len() >= MAX_DEVICES {
            return Err("Remove an unused paired device first (limit 16)".into());
        }
        let placeholder = Stored {
            target: Target {
                device_id: invite.device_id.clone(),
                display_name: display_name.clone(),
                endpoint: endpoint.clone(),
                certificate_sha256: invite.certificate_sha256.clone(),
                credential_id: "0".repeat(64),
                permissions: invite.permissions.clone(),
            },
            certificate_pem: invite.certificate_pem.clone(),
            credential: "0".repeat(64),
        };
        if serde_json::to_string(&placeholder)
            .map_err(|_| "Invalid provisioning record")?
            .len()
            > 2400
        {
            return Err(
                "Provisioning record exceeds secure vault limits; invitation was not consumed"
                    .into(),
            );
        }
        // A secure write/read/delete probe precedes one-use invitation consumption.
        // Use a fixed, nonsecret sentinel; a platform vault error never becomes memory-only pairing.
        self.vault
            .write("pairing-storage-probe", "mixyard-secure-storage-probe")?;
        if self.vault.read("pairing-storage-probe")?.as_deref()
            != Some("mixyard-secure-storage-probe")
        {
            return Err("Secure vault readback failed; invitation was not used".into());
        }
        self.vault.delete("pairing-storage-probe")?;
        let body = serde_json::json!({"deviceId":invite.device_id,"invitation":invite.invitation,"name":display_name}).to_string();
        let reply = tls::post(address, config, "/control/pair", &body, None)?;
        if reply.status != 200 {
            return Err(format!(
                "Pairing rejected (HTTP {}); check invitation expiry and trusted host",
                reply.status
            ));
        }
        let paired: PairReply = decode(&reply.body)?;
        if paired.device_id != invite.device_id || paired.permissions != invite.permissions {
            return Err("Pairing identity or permissions do not match the invitation; revoke this invitation/controller on the owning service".into());
        }
        hex_id(&paired.credential)?;
        hex_id(&paired.credential_id)?;
        roles(&paired.permissions)?;
        let target = Target {
            device_id: paired.device_id,
            display_name,
            endpoint,
            certificate_sha256: invite.certificate_sha256,
            credential_id: paired.credential_id,
            permissions: paired.permissions,
        };
        let stored = Stored {
            target: target.clone(),
            certificate_pem: invite.certificate_pem,
            credential: paired.credential,
        };
        let encoded =
            serde_json::to_string(&stored).map_err(|_| "Unable to encode vault record")?;
        // Windows generic credential blobs have a small byte limit. Bound all platforms equally.
        if encoded.len() > 2400 {
            return Err("Provisioning record exceeds the secure vault record limit; revoke the new controller on the owning service".into());
        }
        self.vault.write(&target.device_id, &encoded).map_err(|e| format!("{e} Pairing was consumed; revoke the new controller on the owning service before retrying."))?;
        if self.vault.read(&target.device_id)?.as_deref() != Some(encoded.as_str()) {
            return Err("Secure credential readback failed after pairing; revoke this controller on the owning service".into());
        }
        ids.push(target.device_id.clone());
        if let Err(error) = self.vault.write(
            INDEX,
            &serde_json::to_string(&ids).map_err(|_| "Vault index encoding failed")?,
        ) {
            let cleanup = self.vault.delete(&target.device_id);
            return Err(format!("{error} Pairing consumed: revoke the new controller on the owning service. Native record cleanup {}.", if cleanup.is_ok() { "completed" } else { "failed; vault repair required" }));
        }
        Ok(target)
    }
    pub fn disconnect(&self, id: &str, expected: Option<u64>) -> Result<(), String> {
        let host = self.host(id)?;
        let current = host.generation.load(Ordering::SeqCst);
        if expected.is_none() || expected == Some(current) {
            if host
                .generation
                .compare_exchange(current, current + 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                *host
                    .session
                    .lock()
                    .map_err(|_| "Paired session unavailable")? = None;
            }
        }
        Ok(())
    }
    pub fn open(&self, id: &str) -> Result<OpenReply, String> {
        let _admission = self.admit()?;
        let host = self.host(id)?;
        let generation = host.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *host.session.lock().map_err(|_| "Session unavailable")? = None;
        let _operation = host.operation.lock().map_err(|_| "Session unavailable")?;
        self.fresh(&host, generation)?;
        let result = (|| {
            let record = self.stored(id)?;
            let reply = self.send(&record, "/control/session", "{}")?;
            if reply.status == 200 {
                let session: Session = decode(&reply.body)?;
                if session.device_id != id
                    || session.backend_available
                    || session.audio_applied
                    || session.revision > SAFE_REVISION
                {
                    return Err("Untrusted session identity/capabilities".into());
                }
                hex_id(&session.authority_id)?;
                roles(&session.permissions)?;
                if session.permissions != record.target.permissions {
                    return Err(
                        "Permissions changed; verify the owning service and pair again".into(),
                    );
                }
                self.fresh(&host, generation)?;
                *host.session.lock().map_err(|_| "Session unavailable")? = Some(session);
            }
            self.fresh(&host, generation)?;
            Ok(OpenReply {
                generation,
                status: reply.status,
                body: reply.body,
            })
        })();
        if result.is_err() {
            let _ = self.disconnect(id, Some(generation));
        }
        result
    }
    pub fn request(&self, id: &str, generation: u64, raw: &str) -> Result<Reply, String> {
        let _admission = self.admit()?;
        if raw.len() > tls::MAX_BODY {
            return Err("Control request exceeds 1 MiB".into());
        }
        let host = self.host(id)?;
        let _operation = host.operation.lock().map_err(|_| "Session unavailable")?;
        self.fresh(&host, generation)?;
        let scope = host
            .session
            .lock()
            .map_err(|_| "Session unavailable")?
            .clone()
            .ok_or("Read a fresh paired session before requesting")?;
        let envelope: Envelope = decode(raw)?;
        if envelope.device_id != id
            || envelope.authority_id != scope.authority_id
            || envelope.revision != scope.revision
        {
            return Err(
                "Request has stale device/authority/revision; refresh and review, never replay"
                    .into(),
            );
        }
        if !allowed_route(&envelope.path) {
            return Err("Canonical route is not supported remotely".into());
        }
        let result = (|| {
            let record = self.stored(id)?;
            // Preserve the entire original envelope including -0 and finite float bit patterns.
            let reply = self.send(&record, "/control/request", raw)?;
            self.fresh(&host, generation)?;
            if reply.status == 401 || reply.status == 409 {
                self.disconnect(id, Some(generation))?;
                return Ok(reply);
            }
            let value: serde_json::Value = decode(&reply.body)?;
            if value.get("body").is_some() {
                let response: ScopedReply = decode(&reply.body)?;
                if response.device_id != id
                    || response.authority_id != scope.authority_id
                    || response.status != reply.status
                    || response.revision < scope.revision
                    || response.revision > SAFE_REVISION
                {
                    return Err("Control response identity/status/revision mismatch".into());
                }
                self.fresh(&host, generation)?;
                let mut session = host.session.lock().map_err(|_| "Session unavailable")?;
                if let Some(session) = session.as_mut() {
                    session.revision = response.revision;
                }
            } else if reply.status < 400 || value.get("error").is_none() {
                return Err("Invalid control response envelope".into());
            }
            Ok(reply)
        })();
        if result.is_err() {
            let _ = self.disconnect(id, Some(generation));
        }
        result
    }
    pub fn forget(&self, id: &str) -> Result<(), String> {
        let _admission = self.admit()?;
        self.disconnect(id, None)?;
        let host = self.host(id)?;
        let _operation = host.operation.lock().map_err(|_| "Session unavailable")?;
        let _storage = self.storage.lock().map_err(|_| "Vault unavailable")?;
        let ids: Vec<_> = self.ids()?.into_iter().filter(|v| v != id).collect();
        // Credential deletion first: failure keeps the indexed record visible for recovery.
        self.vault.delete(id)?;
        self.vault.write(
            INDEX,
            &serde_json::to_string(&ids).map_err(|_| "Vault index encoding failed")?,
        )?;
        Ok(())
    }
    pub fn revoke_self(&self, id: &str, generation: u64) -> Result<Reply, String> {
        let _admission = self.admit()?;
        let host = self.host(id)?;
        let _operation = host.operation.lock().map_err(|_| "Session unavailable")?;
        self.fresh(&host, generation)?;
        let record = self.stored(id)?;
        let scope = host
            .session
            .lock()
            .map_err(|_| "Session unavailable")?
            .clone()
            .ok_or("Refresh before revoking")?;
        if !scope.permissions.iter().any(|v| v == "settings") {
            return Err("Revocation needs settings rights. Revoke this controller locally on the owning service; removing it here only deletes this app's credential.".into());
        }
        let body = serde_json::json!({"deviceId":id,"authorityId":scope.authority_id,"revision":scope.revision,"body":{"credentialId":record.target.credential_id}}).to_string();
        let result = (|| {
            let reply = self.send(&record, "/control/revoke", &body)?;
            self.fresh(&host, generation)?;
            if reply.status == 200 {
                let response: ScopedReply = decode(&reply.body)?;
                let outcome: serde_json::Value = decode(response.body.get())?;
                if response.device_id != id
                    || response.authority_id != scope.authority_id
                    || response.revision != scope.revision
                    || response.status != 200
                    || outcome.get("revoked").and_then(|v| v.as_bool()) != Some(true)
                {
                    return Err("Revocation response identity or outcome mismatch; reconcile on the owning service".into());
                }
            }
            if matches!(reply.status, 200 | 401 | 409) {
                self.disconnect(id, Some(generation))?;
            }
            Ok(reply)
        })();
        if result.is_err() {
            let _ = self.disconnect(id, Some(generation));
        }
        result
    }

    fn send(&self, record: &Stored, route: &str, body: &str) -> Result<Reply, String> {
        tls::post(
            tls::endpoint(&record.target.endpoint)?,
            tls::configuration(&record.certificate_pem, &record.target.certificate_sha256)?,
            route,
            body,
            Some(&record.credential),
        )
    }
    fn fresh(&self, host: &Host, generation: u64) -> Result<(), String> {
        if host.generation.load(Ordering::SeqCst) != generation {
            Err("Paired session changed; old completion discarded. Refresh and review.".into())
        } else {
            Ok(())
        }
    }
    fn admit(&self) -> Result<Admission<'_>, String> {
        self.active
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < 4).then_some(n + 1)
            })
            .map_err(|_| "Four paired operations already pending; refresh before retrying")?;
        Ok(Admission(&self.active))
    }
}
struct Admission<'a>(&'a AtomicUsize);
impl Drop for Admission<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
fn decode<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, String> {
    serde_json::from_str(text)
        .map_err(|_| "Invalid native control JSON; refresh before another write".into())
}
fn hex_id(value: &str) -> Result<(), String> {
    if value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err("Invalid provisioned identity or native credential".into())
    }
}
fn roles(values: &[String]) -> Result<(), String> {
    if values.is_empty()
        || values
            .iter()
            .any(|v| !matches!(v.as_str(), "view" | "control" | "settings"))
        || values
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != values.len()
    {
        Err("Invalid independent permissions".into())
    } else {
        Ok(())
    }
}
fn label(value: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        Err("Device name must be 1–128 UTF-8 bytes without control characters".into())
    } else {
        Ok(())
    }
}
fn allowed_route(path: &str) -> bool {
    matches!(
        path,
        "/api/status"
            | "/api/validate"
            | "/api/inspect"
            | "/api/profiles/read"
            | "/api/profiles/edit"
            | "/api/dsp/preview"
            | "/api/dsp/validate"
            | "/api/dsp/delay"
            | "/api/automation/status"
            | "/api/automation/settings/read"
            | "/api/automation/settings/write"
            | "/api/automation/validate"
            | "/api/automation/evaluate"
            | "/api/automation/context"
            | "/api/automation/disarm"
            | "/api/automation/manual"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct MemoryVault(Mutex<BTreeMap<String, String>>);
    impl Vault for MemoryVault {
        fn read(&self, key: &str) -> Result<Option<String>, String> {
            Ok(self.0.lock().unwrap().get(key).cloned())
        }
        fn write(&self, key: &str, value: &str) -> Result<(), String> {
            self.0.lock().unwrap().insert(key.into(), value.into());
            Ok(())
        }
        fn delete(&self, key: &str) -> Result<(), String> {
            self.0.lock().unwrap().remove(key);
            Ok(())
        }
    }
    #[test]
    fn missing_deleted_index_record_does_not_hide_other_paired_devices() {
        let deleted = "a".repeat(64);
        let retained = "b".repeat(64);
        let vault = MemoryVault::default();
        vault
            .write(
                INDEX,
                &serde_json::to_string(&vec![&deleted, &retained]).unwrap(),
            )
            .unwrap();
        vault.write(&retained, "existing-record").unwrap();
        let client = Paired::new(vault);
        assert_eq!(client.ids().unwrap(), vec![retained]);
    }
    #[test]
    fn disconnect_invalidates_queued_native_authority_without_touching_another_host() {
        let client = Paired::new(MemoryVault::default());
        let a = "a".repeat(64);
        let b = "b".repeat(64);
        let host_a = client.host(&a).unwrap();
        let host_b = client.host(&b).unwrap();
        client.disconnect(&a, Some(0)).unwrap();
        assert!(client.fresh(&host_a, 0).is_err());
        assert!(client.fresh(&host_b, 0).is_ok());
        // A delayed abort cannot disconnect a later session.
        client.disconnect(&a, Some(0)).unwrap();
        assert!(client.fresh(&host_a, 1).is_ok());
    }
    #[test]
    fn native_owner_conflict_is_explicit_and_owner_is_released_on_exit() {
        let directory =
            std::env::temp_dir().join(format!("mixyard-paired-owner-test-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let first = PlatformVault::open(Ok(directory.clone()));
        assert!(first.available().is_ok());
        let second = PlatformVault::open(Ok(directory.clone()));
        assert!(second
            .available()
            .unwrap_err()
            .contains("Another desktop process"));
        drop(first);
        drop(second);
        let reopened = PlatformVault::open(Ok(directory.clone()));
        assert!(reopened.available().is_ok());
        drop(reopened);
        std::fs::remove_dir_all(directory).unwrap();
    }
}

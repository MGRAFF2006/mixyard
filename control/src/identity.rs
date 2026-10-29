#[cfg(not(windows))]
use crate::store_fs;
use crate::{fail, Result};
#[cfg(windows)]
#[path = "identity_windows.rs"]
mod windows_store;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    net::IpAddr,
    path::{Component, Path},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(not(windows))]
use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
};

const LIMIT: u64 = 1024 * 1024;
const NAMES: &[&str] = &["state.json", "owner.lock", "pending.json"];

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    View,
    Control,
    Settings,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Trust {
    pub device_id: String,
    pub server_name: &'static str,
    pub certificate_pem: String,
    pub certificate_sha256: String,
}

/// Local owner-only key material for an independently consented audio endpoint.
/// This is never serialized or reachable through a management HTTP operation.
/// The caller must separately supply exact directional audio grants.
pub struct AudioIdentityMaterial {
    pub device_id: String,
    pub certificate: CertificateDer<'static>,
    pub private_key: PrivateKeyDer<'static>,
    pub server_name: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Credential {
    pub credential_id: String,
    pub name: String,
    pub permissions: Vec<Permission>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Invitation {
    expires_at_ms: u64,
    permissions: Vec<Permission>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Data {
    format: String,
    device_id: String,
    bind_ip: IpAddr,
    certificate_der: Vec<u8>,
    certificate_pem: String,
    private_key_pkcs8: Vec<u8>,
    invitations: BTreeMap<String, Invitation>,
    credentials: BTreeMap<String, Credential>,
}

/// Private cooperating-writer store; a crash leaves a manual-recovery sentinel.
/// Holds the sentinel for its entire lifetime, including while HTTP work is active.
pub struct IdentityStore {
    #[cfg(not(windows))]
    path: PathBuf,
    #[cfg(windows)]
    windows: windows_store::Store,
    data: Data,
    poisoned: bool,
}

pub(crate) fn random() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|_| fail(503, "entropy-unavailable", "OS entropy unavailable"))?;
    Ok(hex(&bytes))
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn hash(secret: &str) -> String {
    hex(&Sha256::digest(secret.as_bytes()))
}
pub(crate) fn now_ms() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .ok_or_else(|| fail(503, "clock-unavailable", "Valid wall clock required"))
}
pub fn allowed_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_loopback()
                || ip.is_private()
                || (ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]))
        }
        IpAddr::V6(ip) => ip.is_loopback() || ip.is_unique_local(),
    }
}
fn validate_permissions(p: &[Permission]) -> Result<()> {
    if p.is_empty()
        || p.len() > 3
        || p.iter().collect::<std::collections::BTreeSet<_>>().len() != p.len()
    {
        return Err(fail(
            400,
            "invalid-permissions",
            "Choose a nonempty set of view,control,settings",
        ));
    }
    Ok(())
}

impl IdentityStore {
    pub fn open(path: &Path, initialize: Option<IpAddr>) -> Result<Self> {
        if path.as_os_str().is_empty()
            || path.components().any(|p| matches!(p, Component::ParentDir))
        {
            return Err(fail(
                400,
                "unsafe-state-path",
                "Choose a dedicated private path without '..'",
            ));
        }
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            std::env::current_dir()?.join(path)
        };
        #[cfg(windows)]
        let windows = windows_store::Store::open(&path, initialize.is_some())?;
        #[cfg(not(windows))]
        let lock = path.join("owner.lock");
        #[cfg(not(windows))]
        let owner = {
            let exists = store_fs::check_directory(&path, NAMES)?;
            if !exists {
                if initialize.is_none() {
                    return Err(fail(
                        503,
                        "identity-missing",
                        "Run init locally before starting control",
                    ));
                }
                store_fs::create_directory(&path)?;
            }
            // Never reclaim an existing owner's lock.
            store_fs::create_file(&lock).map_err(|_| fail(409,"identity-busy","Owner lock exists or cannot be created; stop owner and inspect before manual recovery"))?
        };
        let read = (|| -> Result<Data> {
            #[cfg(windows)]
            let pending = windows.pending()?;
            #[cfg(not(windows))]
            let pending = store_fs::check_file(&path.join("pending.json"))?;
            if pending {
                return Err(fail(
                    409,
                    "recovery-required",
                    "Pending identity write requires manual inspection",
                ));
            }
            #[cfg(windows)]
            let bytes = windows.read()?;
            #[cfg(not(windows))]
            let bytes = if store_fs::check_file(&path.join("state.json"))? {
                let mut bytes = Vec::new();
                fs::File::open(path.join("state.json"))?
                    .take(LIMIT + 1)
                    .read_to_end(&mut bytes)?;
                Some(bytes)
            } else {
                None
            };
            if let Some(bytes) = bytes {
                if initialize.is_some() {
                    return Err(fail(
                        409,
                        "identity-exists",
                        "Identity already exists; never replace implicitly",
                    ));
                }
                if bytes.len() as u64 > LIMIT {
                    return Err(fail(
                        503,
                        "identity-too-large",
                        "Identity store exceeds 1 MiB",
                    ));
                }
                let data: Data = serde_json::from_slice(&bytes).map_err(|_| {
                    fail(
                        503,
                        "identity-corrupt",
                        "Identity JSON invalid; preserve and recover manually",
                    )
                })?;
                if data.format != "mixyard-control-v1"
                    || data.device_id.len() != 64
                    || !allowed_ip(data.bind_ip)
                    || data.credentials.len() > 128
                    || data.invitations.len() > 32
                {
                    return Err(fail(503, "identity-corrupt", "Invalid identity context"));
                }
                for c in data.credentials.values() {
                    validate_permissions(&c.permissions)?;
                }
                for i in data.invitations.values() {
                    validate_permissions(&i.permissions)?;
                }
                Ok(data)
            } else {
                let ip =
                    initialize.ok_or_else(|| fail(503, "identity-missing", "Run init first"))?;
                if !allowed_ip(ip) {
                    return Err(fail(
                        400,
                        "unsafe-bind",
                        "Only exact loopback/private LAN/Tailscale addresses allowed",
                    ));
                }
                let generated = rcgen::generate_simple_self_signed(vec![
                    "mixyard-control".into(),
                    "localhost".into(),
                    "127.0.0.1".into(),
                    ip.to_string(),
                ])
                .map_err(|_| {
                    fail(
                        503,
                        "identity-generation-failed",
                        "Could not generate TLS identity",
                    )
                })?;
                Ok(Data {
                    format: "mixyard-control-v1".into(),
                    device_id: random()?,
                    bind_ip: ip,
                    certificate_der: generated.cert.der().to_vec(),
                    certificate_pem: generated.cert.pem(),
                    private_key_pkcs8: generated.signing_key.serialize_der(),
                    invitations: BTreeMap::new(),
                    credentials: BTreeMap::new(),
                })
            }
        })();
        #[cfg(not(windows))]
        drop(owner);
        let data = match read {
            Ok(d) => d,
            Err(e) => {
                #[cfg(not(windows))]
                fs::remove_file(lock)?;
                return Err(e);
            }
        };
        let mut store = Self {
            #[cfg(not(windows))]
            path,
            #[cfg(windows)]
            windows,
            data,
            poisoned: false,
        };
        if initialize.is_some() {
            store.persist(store.data.clone())?;
        }
        // Validate key/certificate correspondence before exposing any endpoint.
        store.tls()?;
        Ok(store)
    }
    pub fn device_id(&self) -> &str {
        &self.data.device_id
    }
    pub fn bind_ip(&self) -> IpAddr {
        self.data.bind_ip
    }
    pub fn audio_identity_material(
        &self,
        expected_device_id: &str,
    ) -> Result<AudioIdentityMaterial> {
        self.ensure()?;
        if expected_device_id != self.device_id() {
            return Err(fail(
                409,
                "target-mismatch",
                "Audio provisioning belongs to another stable device identity",
            ));
        }
        Ok(AudioIdentityMaterial {
            device_id: self.device_id().into(),
            certificate: CertificateDer::from(self.data.certificate_der.clone()),
            private_key: PrivatePkcs8KeyDer::from(self.data.private_key_pkcs8.clone()).into(),
            server_name: "mixyard-control".into(),
        })
    }
    pub fn trust(&self) -> Trust {
        Trust {
            device_id: self.data.device_id.clone(),
            server_name: "mixyard-control",
            certificate_pem: self.data.certificate_pem.clone(),
            certificate_sha256: hex(&Sha256::digest(&self.data.certificate_der)),
        }
    }
    pub(crate) fn tls(&self) -> Result<rustls::ServerConfig> {
        let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
        let mut config = rustls::ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|_| fail(503, "tls-unavailable", "TLS1.3 unavailable"))?
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(self.data.certificate_der.clone())],
                PrivatePkcs8KeyDer::from(self.data.private_key_pkcs8.clone()).into(),
            )
            .map_err(|_| fail(503, "identity-corrupt", "Invalid stored TLS identity"))?;
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        config.max_early_data_size = 0;
        Ok(config)
    }
    fn ensure(&self) -> Result<()> {
        if self.poisoned {
            Err(fail(
                503,
                "identity-store-unavailable",
                "Identity write uncertain; stop service and inspect state",
            ))
        } else {
            Ok(())
        }
    }
    fn persist(&mut self, data: Data) -> Result<()> {
        self.ensure()?;
        #[cfg(not(windows))]
        store_fs::check_directory(&self.path, NAMES)?;
        #[cfg(windows)]
        self.windows.check()?;
        let bytes = serde_json::to_vec(&data)
            .map_err(|_| fail(503, "identity-corrupt", "Cannot serialize identity"))?;
        if bytes.len() as u64 > LIMIT {
            return Err(fail(
                503,
                "identity-too-large",
                "Identity store exceeds 1 MiB",
            ));
        }
        self.poisoned = true;
        #[cfg(windows)]
        {
            self.windows.retain_lock(true);
            self.windows.write(&bytes)?;
            self.windows.retain_lock(false);
        }
        #[cfg(not(windows))]
        {
            let mut pending = store_fs::create_file(&self.path.join("pending.json"))?;
            pending.write_all(&bytes)?;
            pending.sync_all()?;
            drop(pending);
            fs::rename(self.path.join("pending.json"), self.path.join("state.json"))?;
            #[cfg(unix)]
            fs::File::open(&self.path)?.sync_all()?;
        }
        self.data = data;
        self.poisoned = false;
        Ok(())
    }
    pub fn invite(
        &mut self,
        permissions: Vec<Permission>,
        ttl_seconds: u64,
    ) -> Result<serde_json::Value> {
        validate_permissions(&permissions)?;
        if ttl_seconds == 0 || ttl_seconds > 300 {
            return Err(fail(
                400,
                "invalid-expiry",
                "Invitation lifetime must be 1..300 seconds",
            ));
        }
        let now = now_ms()?;
        let invitation = random()?;
        let mut data = self.data.clone();
        data.invitations.retain(|_, i| i.expires_at_ms > now);
        if data.invitations.len() >= 32 {
            return Err(fail(
                503,
                "invitation-limit",
                "32 outstanding invitations; wait for expiry",
            ));
        }
        let expires = now
            .checked_add(ttl_seconds * 1000)
            .ok_or_else(|| fail(503, "clock-unavailable", "Expiry overflow"))?;
        data.invitations.insert(
            hash(&invitation),
            Invitation {
                expires_at_ms: expires,
                permissions: permissions.clone(),
            },
        );
        self.persist(data)?;
        let mut result = serde_json::to_value(self.trust()).expect("trust JSON");
        result["invitation"] = invitation.into();
        result["permissions"] = serde_json::to_value(permissions).unwrap();
        result["expiresAtMs"] = expires.into();
        Ok(result)
    }
    pub(crate) fn pair(&mut self, invitation: &str, name: String) -> Result<serde_json::Value> {
        self.ensure()?;
        if name.trim().is_empty() || name.len() > 128 {
            return Err(fail(
                400,
                "invalid-name",
                "Controller name must be nonblank, at most 128 UTF-8 bytes",
            ));
        }
        let now = now_ms()?;
        let invitation_hash = hash(invitation);
        let invite = self
            .data
            .invitations
            .get(&invitation_hash)
            .filter(|i| i.expires_at_ms > now)
            .ok_or_else(|| {
                fail(
                    401,
                    "unauthorized",
                    "Invalid, expired or consumed pairing invitation",
                )
            })?;
        if self.data.credentials.len() >= 128 {
            return Err(fail(
                503,
                "credential-limit",
                "128 credentials; revoke unused controllers",
            ));
        }
        let credential = random()?;
        let entry = Credential {
            credential_id: random()?,
            name,
            permissions: invite.permissions.clone(),
        };
        let mut data = self.data.clone();
        data.invitations.remove(&invitation_hash);
        data.credentials.insert(hash(&credential), entry.clone());
        self.persist(data)?;
        Ok(
            serde_json::json!({"deviceId":self.device_id(),"credentialId":entry.credential_id,"credential":credential,"permissions":entry.permissions}),
        )
    }
    pub(crate) fn authenticate(&self, secret: &str) -> Result<Credential> {
        self.ensure()?;
        self.data
            .credentials
            .get(&hash(secret))
            .cloned()
            .ok_or_else(|| fail(401, "unauthorized", "Credential absent, revoked or rotated"))
    }
    pub(crate) fn credentials(&self) -> Vec<Credential> {
        self.data.credentials.values().cloned().collect()
    }
    pub fn revoke(&mut self, id: &str) -> Result<()> {
        let mut data = self.data.clone();
        if !data.credentials.values().any(|c| c.credential_id == id) {
            return Err(fail(404, "credential-not-found", "Unknown credential"));
        }
        data.credentials.retain(|_, c| c.credential_id != id);
        self.persist(data)
    }
    pub(crate) fn rotate(&mut self, secret: &str) -> Result<serde_json::Value> {
        let entry = self.authenticate(secret)?;
        let credential = random()?;
        let mut data = self.data.clone();
        data.credentials.remove(&hash(secret));
        data.credentials.insert(hash(&credential), entry.clone());
        self.persist(data)?;
        Ok(
            serde_json::json!({"deviceId":self.device_id(),"credentialId":entry.credential_id,"credential":credential,"permissions":entry.permissions}),
        )
    }
}
impl Drop for IdentityStore {
    fn drop(&mut self) {
        // On uncertain commit retain the lock as an explicit recovery barrier.
        #[cfg(not(windows))]
        if !self.poisoned {
            let _ = fs::remove_file(self.path.join("owner.lock"));
        }
    }
}

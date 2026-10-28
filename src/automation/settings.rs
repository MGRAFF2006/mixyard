//! Durable user intent only. Reading settings never creates files or arms automation.
//! This dedicated store must be a sibling of, never inside, a profile store.
use super::{validate_policy, FactDefinition, FactKey, Graph, MAX_FACTS};
pub use crate::store_fs::Error;
use crate::store_fs::{
    check_ancestors, check_directory, check_file, create_directory, create_file,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    fs::File,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

pub const FORMAT: &str = "mixyard-situation-settings";
pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_STORE_BYTES: usize = 1024 * 1024;
pub const MAX_DURATION_MS: u64 = 86_400_000;
const DATA: &str = "settings.json";
const LOCK: &str = "write.lock";
const PENDING: &str = "pending.json";

/// Monotonic duration, serialized as integer milliseconds; valid range 0..=86400000.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct Milliseconds(pub u64);

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransitionPolicy {
    pub settle_ms: Milliseconds,
    pub minimum_hold_ms: Milliseconds,
    pub cooldown_ms: Milliseconds,
}
impl Default for TransitionPolicy {
    fn default() -> Self {
        Self {
            settle_ms: Milliseconds(1000),
            minimum_hold_ms: Milliseconds(2000),
            cooldown_ms: Milliseconds(2000),
        }
    }
}

/// A known boolean explicitly declared by the user for one exact entity.
/// No timestamps, provider/evidence labels, imported facts or native claims are stored.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Declaration {
    pub key: FactKey,
    pub value: bool,
}

/// Matches the configurable subset of `context::Options`. Trusted installation
/// paths belong to the local adapter and cannot be supplied through this type.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObserverSettings {
    #[serde(deserialize_with = "application_map")]
    pub applications: BTreeMap<String, String>,
    pub host_label: Option<String>,
    pub timezone: String,
}
// A repeated executable key is ambiguous user intent, not last-writer-wins.
fn application_map<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, String>, D::Error> {
    struct Applications;
    impl<'de> serde::de::Visitor<'de> for Applications {
        type Value = BTreeMap<String, String>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("at most 32 unique executable paths mapped to application IDs")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut map: M,
        ) -> Result<Self::Value, M::Error> {
            let mut applications = BTreeMap::new();
            while let Some((path, id)) = map.next_entry::<String, String>()? {
                if applications.len() == 32 || applications.insert(path, id).is_some() {
                    return Err(serde::de::Error::custom(
                        "too many or duplicate executable mappings",
                    ));
                }
            }
            Ok(applications)
        }
    }
    deserializer.deserialize_map(Applications)
}
impl Default for ObserverSettings {
    fn default() -> Self {
        Self {
            applications: BTreeMap::new(),
            host_label: None,
            timezone: "Europe/Berlin".into(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub graph: Option<Graph>,
    pub catalog: Vec<FactDefinition>,
    pub declarations: Vec<Declaration>,
    pub transition_policy: TransitionPolicy,
    pub observer: ObserverSettings,
}
impl Settings {
    /// Validate borrowed data before scans, serialization or cloning.
    pub fn validate(&self) -> Result<(), Error> {
        match &self.graph {
            Some(graph) => validate_policy(graph, &self.catalog)
                .map_err(|e| Error::new(e.code, e.message, 3))?,
            None if !self.catalog.is_empty() => {
                return Err(invalid(
                    "A catalog requires a graph; replace both as one policy",
                ));
            }
            None => {}
        }
        if self.declarations.len() > MAX_FACTS {
            return Err(invalid("At most 512 declarations are supported"));
        }
        // Admission before duplicate scans, including direct Rust inputs.
        for declaration in &self.declarations {
            if !matches!(
                declaration.key.name.as_str(),
                "device.hardware-monitor-declared" | "manual.flag"
            ) || !declaration.key.subject.as_deref().is_some_and(text_ok)
            {
                return Err(invalid("Declarations require a known boolean name and exact nonblank entity ID (at most 1024 bytes)"));
            }
        }
        let mut keys = BTreeSet::new();
        for declaration in &self.declarations {
            if !keys.insert(&declaration.key) {
                return Err(invalid("Duplicate declaration identity"));
            }
        }
        for duration in [
            self.transition_policy.settle_ms,
            self.transition_policy.minimum_hold_ms,
            self.transition_policy.cooldown_ms,
        ] {
            if duration.0 > MAX_DURATION_MS {
                return Err(invalid("Transition duration exceeds 86400000 milliseconds"));
            }
        }
        self.observer.validate()
    }
}
fn text_ok(s: &str) -> bool {
    !s.trim().is_empty() && s.len() <= super::MAX_TEXT_BYTES
}
impl ObserverSettings {
    pub fn validate(&self) -> Result<(), Error> {
        if !text_ok(&self.timezone)
            || self.host_label.as_deref().is_some_and(|s| !text_ok(s))
            || self.applications.len() > 32
            || self
                .applications
                .iter()
                .any(|(path, id)| !Path::new(path).is_absolute() || !text_ok(path) || !text_ok(id))
        {
            return Err(invalid("invalid-observer-options"));
        }
        // Timezone existence/DST support is established by the observer, not by
        // executing a helper while validating settings.
        Ok(())
    }
}
fn invalid(message: impl Into<String>) -> Error {
    Error::new("invalid-settings", message, 3)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Snapshot {
    pub format: String,
    pub schema_version: u32,
    pub settings_revision: u64,
    pub settings: Settings,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            format: FORMAT.into(),
            schema_version: SCHEMA_VERSION,
            settings_revision: 0,
            settings: Settings::default(),
        }
    }
}
impl Snapshot {
    fn validate(&self) -> Result<(), Error> {
        if self.format != FORMAT || self.schema_version != SCHEMA_VERSION {
            return Err(Error::new("unsupported-store", "Expected mixyard-situation-settings schemaVersion 1; no migration or overwrite attempted", 3));
        }
        self.settings.validate()
    }
}

/// Bounded strict JSON input for adapters; serde's recursion limit stays enabled.
pub fn parse_settings(bytes: &[u8]) -> Result<Settings, Error> {
    check_size(bytes.len())?;
    let settings: Settings = serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string()))?;
    settings.validate()?;
    Ok(settings)
}
fn check_size(size: usize) -> Result<(), Error> {
    if size > MAX_STORE_BYTES {
        return Err(Error::new(
            "store-too-large",
            "Settings snapshot exceeds 1 MiB; nothing was truncated or overwritten",
            3,
        ));
    }
    Ok(())
}

/// Trusted local filesystem, cooperating writers only. A crashed writer's
/// sentinel/pending file requires manual recovery; it is never reclaimed.
#[derive(Debug)]
pub struct Store {
    directory: PathBuf,
}
impl Store {
    pub fn new(directory: impl AsRef<Path>) -> Result<Self, Error> {
        let path = directory.as_ref();
        if path.as_os_str().is_empty()
            || path.components().any(|c| matches!(c, Component::ParentDir))
        {
            return Err(Error::new(
                "unsafe-store-path",
                "Store path must be nonempty and contain no '..' components",
                4,
            ));
        }
        let directory = if path.is_absolute() {
            path.to_owned()
        } else {
            env::current_dir().map_err(Error::io)?.join(path)
        };
        check_ancestors(&directory)?;
        Ok(Self { directory })
    }

    /// `profiles` maps to sibling `situation-settings`; a custom basename maps
    /// to `<basename>.situation-settings`. Never places files in the profile store.
    pub fn sibling_of_profiles(profile_directory: impl AsRef<Path>) -> Result<Self, Error> {
        let profiles = Self::new(profile_directory)?;
        let name = profiles
            .directory
            .file_name()
            .ok_or_else(|| invalid("Profile directory needs a basename"))?;
        let sibling = if name == "profiles" {
            "situation-settings".into()
        } else {
            let mut sibling = name.to_os_string();
            sibling.push(".situation-settings");
            sibling
        };
        Self::new(profiles.directory.with_file_name(sibling))
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn read(&self) -> Result<Snapshot, Error> {
        if !self.check_directory()? || !check_file(&self.directory.join(DATA))? {
            return Ok(Snapshot::default());
        }
        let mut bytes = Vec::new();
        File::open(self.directory.join(DATA))
            .map_err(Error::io)?
            .take((MAX_STORE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(Error::io)?;
        check_size(bytes.len())?;
        let snapshot: Snapshot = serde_json::from_slice(&bytes).map_err(|e| {
            Error::new(
                "corrupt-store",
                format!("{e}; preserve settings.json and recover from a verified backup"),
                3,
            )
        })?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn write(
        &self,
        expected_settings_revision: u64,
        settings: &Settings,
    ) -> Result<Snapshot, Error> {
        settings.validate()?;
        let next = self.next_revision(expected_settings_revision)?;
        // Serialize borrowed input with a byte bound before cloning or touching disk.
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Record<'a> {
            format: &'static str,
            schema_version: u32,
            settings_revision: u64,
            settings: &'a Settings,
        }
        let mut bytes = BoundedBytes(Vec::new());
        serde_json::to_writer(
            &mut bytes,
            &Record {
                format: FORMAT,
                schema_version: SCHEMA_VERSION,
                settings_revision: next,
                settings,
            },
        )
        .map_err(|_| {
            Error::new(
                "store-too-large",
                "Settings snapshot exceeds 1 MiB or cannot be serialized; no write attempted",
                3,
            )
        })?;
        if !self.check_directory()? {
            create_directory(&self.directory)?;
        }
        self.check_directory()?;
        let lock_path = self.directory.join(LOCK);
        drop(create_file(&lock_path).map_err(|e| {
            if e.kind() == io::ErrorKind::AlreadyExists {
                Error::new("store-busy", "write.lock exists; another writer or interrupted write requires manual inspection, never automatic reclamation", 5)
            } else { Error::io(e) }
        })?);
        // A revision race is safe to retry after rereading. Other failures retain
        // our sentinel because disk state may require operator inspection.
        if let Err(error) = self.next_revision(expected_settings_revision) {
            if error.code == "store-conflict" || error.code == "revision-exhausted" {
                fs::remove_file(&lock_path).map_err(Error::io)?;
            }
            return Err(error);
        }
        let pending_path = self.directory.join(PENDING);
        if check_file(&pending_path)? {
            return Err(Error::new("recovery-required", "pending.json exists; preserve settings.json and inspect the interrupted write manually", 5));
        }
        let mut pending = create_file(&pending_path).map_err(Error::io)?;
        pending.write_all(&bytes.0).map_err(Error::io)?;
        pending.sync_all().map_err(Error::io)?;
        drop(pending);
        self.check_directory()?;
        check_file(&self.directory.join(DATA))?;
        // Never delete the destination to work around platform rename failures.
        fs::rename(&pending_path, self.directory.join(DATA)).map_err(Error::io)?;
        #[cfg(unix)]
        File::open(&self.directory).and_then(|f| f.sync_all()).map_err(|e| Error::new("commit-uncertain", format!("Replacement occurred but directory sync failed: {e}; reread before retrying"), 4))?;
        fs::remove_file(&lock_path).map_err(|e| {
            Error::new(
                "commit-uncertain",
                format!(
                    "Replacement occurred but lock cleanup failed: {e}; reread before retrying"
                ),
                4,
            )
        })?;
        Ok(Snapshot {
            format: FORMAT.into(),
            schema_version: SCHEMA_VERSION,
            settings_revision: next,
            settings: settings.clone(),
        })
    }
    fn next_revision(&self, expected: u64) -> Result<u64, Error> {
        let current = self.read()?.settings_revision;
        if current != expected {
            return Err(Error::new("store-conflict", format!("Expected settings revision {expected}, found {current}; reread and review before retrying"), 5));
        }
        current.checked_add(1).ok_or_else(|| {
            Error::new(
                "revision-exhausted",
                "Settings revision exhausted; no wraparound or write attempted",
                3,
            )
        })
    }
    fn check_directory(&self) -> Result<bool, Error> {
        check_directory(&self.directory, &[DATA, LOCK, PENDING])
    }
}
struct BoundedBytes(Vec<u8>);
impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_STORE_BYTES - self.0.len() {
            return Err(io::Error::other("settings byte limit exceeded"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Resolve a sibling of the default profile store without touching disk.
pub fn default_directory() -> Result<PathBuf, Error> {
    Ok(Store::sibling_of_profiles(crate::profiles::default_directory()?)?.directory)
}

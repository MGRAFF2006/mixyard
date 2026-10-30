//! Local desired drafts only. No backend, native consent, or media resume state.
use crate::{plan, Config, Inventory, Plan, ResolutionStatus, Selector, Target};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    fs::File,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

const FORMAT: &str = "mixyard-local-drafts";
const DATA: &str = "profiles.json";
const LOCK: &str = "write.lock";
const PENDING: &str = "pending.json";

pub use crate::store_fs::Error;
use crate::store_fs::{
    check_ancestors, check_directory, check_file, create_directory, create_file,
};
impl Error {
    fn policy(error: impl Into<String>) -> Self {
        Self::new("invalid-policy", error, 3)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub config: Config,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Snapshot {
    format: String,
    schema_version: u32,
    pub store_revision: u64,
    config_revision: u64,
    pub profiles: Vec<Profile>,
    pub draft: Option<Config>,
    pub previous_draft: Option<Config>,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            format: FORMAT.into(),
            schema_version: 1,
            store_revision: 0,
            config_revision: 0,
            profiles: vec![],
            draft: None,
            previous_draft: None,
        }
    }
}
impl Snapshot {
    fn validate(&self) -> Result<(), Error> {
        if self.format != FORMAT || self.schema_version != 1 {
            return Err(Error::new("unsupported-store", "Expected mixyard-local-drafts store schemaVersion 1; no migration or overwrite attempted", 3));
        }
        let mut ids = BTreeSet::new();
        for profile in &self.profiles {
            if profile.id.trim().is_empty()
                || !ids.insert(&profile.id)
                || profile.name.trim().is_empty()
            {
                return Err(Error::new(
                    "corrupt-store",
                    "Blank/duplicate profile identity or blank name",
                    3,
                ));
            }
        }
        if self.previous_draft.is_some() && self.draft.is_none() {
            return Err(Error::new(
                "corrupt-store",
                "Undo draft without current draft",
                3,
            ));
        }
        for config in self
            .profiles
            .iter()
            .map(|p| &p.config)
            .chain(self.draft.iter())
            .chain(self.previous_draft.iter())
        {
            config
                .validate()
                .map_err(|e| Error::new("corrupt-store", e, 3))?;
            if config.revision > self.config_revision {
                return Err(Error::new(
                    "corrupt-store",
                    "Config revision exceeds stored revision context",
                    3,
                ));
            }
        }
        Ok(())
    }
    fn replace_draft(&mut self, mut config: Config) -> Result<(), Error> {
        config.validate().map_err(Error::policy)?;
        self.config_revision = increment(self.config_revision.max(config.revision))?;
        config.revision = self.config_revision;
        self.previous_draft = self.draft.replace(config);
        Ok(())
    }
    pub fn profile(&self, id: &str) -> Result<&Profile, Error> {
        self.profiles.iter().find(|p| p.id == id).ok_or_else(|| {
            Error::new(
                "profile-not-found",
                format!("Unknown stable profile ID: {id}"),
                3,
            )
        })
    }
}
fn increment(value: u64) -> Result<u64, Error> {
    value.checked_add(1).ok_or_else(|| {
        Error::new(
            "revision-exhausted",
            "Revision exhausted; no wraparound or write attempted",
            3,
        )
    })
}

pub enum Edit {
    Save {
        id: Option<String>,
        name: String,
        config: Config,
    },
    Rename {
        id: String,
        name: String,
    },
    Draft(Config),
    Load {
        id: String,
    },
    Undo,
}

/// The directory is dedicated to this store, on a trusted local filesystem.
/// A create-new sentinel serializes cooperating writers. It is never reclaimed.
#[derive(Debug)]
pub struct Store {
    directory: PathBuf,
}
impl Store {
    /// Read the canonical desired draft without acquiring the writer sentinel.
    #[cfg(all(feature = "desktop", target_os = "linux"))]
    pub fn current_snapshot(&self, max_bytes: usize) -> Result<CurrentSnapshot, Error> {
        let snapshot = self.read_bounded(max_bytes)?;
        let config = snapshot
            .draft
            .ok_or_else(|| Error::new("no-draft", "No canonical desired draft is stored", 3))?;
        use sha2::{Digest, Sha256};
        let bytes = serde_json::to_vec(&config).map_err(Error::io)?;
        Ok(CurrentSnapshot {
            store_revision: snapshot.store_revision,
            config_revision: config.revision,
            config_sha256: format!("{:x}", Sha256::digest(bytes)),
            config,
        })
    }

    /// Acquire the existing writer exclusion only after native compilation.
    /// A successful acquisition owns the sentinel until explicit safe release.
    #[cfg(all(feature = "desktop", target_os = "linux"))]
    pub fn guard_current_snapshot(
        &self,
        expected: &CurrentSnapshot,
        max_bytes: usize,
    ) -> Result<CurrentSnapshotGuard, Error> {
        self.guard_current_snapshot_with(expected, max_bytes, || {})
    }
    #[cfg(all(feature = "desktop", target_os = "linux"))]
    fn guard_current_snapshot_with(
        &self,
        expected: &CurrentSnapshot,
        max_bytes: usize,
        before_lock: impl FnOnce(),
    ) -> Result<CurrentSnapshotGuard, Error> {
        self.current_snapshot(max_bytes)?.matches(expected)?;
        before_lock();
        self.check_directory()?;
        use std::os::unix::fs::MetadataExt;
        let directory = fs::symlink_metadata(&self.directory).map_err(Error::io)?;
        if directory.uid() != unsafe { libc::geteuid() } {
            return Err(Error::new(
                "unsafe-store-path",
                "Canonical store directory belongs to another user",
                4,
            ));
        }
        let path = self.directory.join(LOCK);
        let lock = create_file(&path).map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                Error::new(
                    "store-busy",
                    "write.lock exists; it is never reclaimed automatically",
                    5,
                )
            } else {
                Error::io_at("create current-snapshot writer lock", &path, error)
            }
        })?;
        let metadata = lock.metadata().map_err(Error::io)?;
        let mut guard = CurrentSnapshotGuard {
            directory: self.directory.clone(),
            directory_identity: (directory.dev(), directory.ino()),
            lock_identity: (metadata.dev(), metadata.ino()),
            lock: Some(lock),
        };
        // No native ticket has been queued, so a stale recheck may safely
        // release only this lock. Other I/O uncertainty retains the sentinel.
        let current_directory = fs::symlink_metadata(&self.directory).map_err(Error::io)?;
        if (current_directory.dev(), current_directory.ino()) != guard.directory_identity {
            return Err(Error::new(
                "store-lock-changed",
                "Canonical store directory changed during lock acquisition",
                5,
            ));
        }
        let current = self.current_snapshot(max_bytes)?;
        if let Err(error) = current.matches(expected) {
            guard.release()?;
            return Err(error);
        }
        Ok(guard)
    }

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
    pub fn read(&self) -> Result<Snapshot, Error> {
        self.read_with_limit(None)
    }
    /// Opt-in byte bound for desktop work. Uses the same path checks, parser and
    /// full snapshot validation as `read`; never truncates or repairs a store.
    pub fn read_bounded(&self, max_bytes: usize) -> Result<Snapshot, Error> {
        self.read_with_limit(Some(max_bytes))
    }
    fn read_with_limit(&self, max_bytes: Option<usize>) -> Result<Snapshot, Error> {
        if !self.check_directory()? {
            return Ok(Snapshot::default());
        }
        let path = self.directory.join(DATA);
        if !check_file(&path)? {
            return Ok(Snapshot::default());
        }
        let file = File::open(&path).map_err(|e| Error::io_at("open snapshot", &path, e))?;
        let parsed = if let Some(limit) = max_bytes {
            let mut bytes = Vec::new();
            file.take((limit as u64).saturating_add(1))
                .read_to_end(&mut bytes)
                .map_err(|e| Error::io_at("read snapshot", &path, e))?;
            if bytes.len() > limit {
                return Err(Error::new("store-too-large", format!("Store exceeds the {limit}-byte desktop read limit; nothing was truncated. Use the profiles CLI for larger stores."), 3));
            }
            serde_json::from_slice(&bytes)
        } else {
            serde_json::from_reader(file)
        };
        let snapshot: Snapshot = parsed.map_err(|e| {
            if e.is_io() {
                Error::io(format!("read snapshot {}: {e}", path.display()))
            } else {
                Error::new(
                    "corrupt-store",
                    format!(
                        "{}: {e}; preserve the store and recover from a verified backup",
                        path.display()
                    ),
                    3,
                )
            }
        })?;
        snapshot.validate()?;
        Ok(snapshot)
    }
    pub fn edit(&self, expected_revision: u64, edit: Edit) -> Result<Snapshot, Error> {
        // Validate before creating files, and again inside the writer lock.
        let mut snapshot = self.read()?;
        self.prepare(&mut snapshot, expected_revision, &edit)?;
        if !self.check_directory()? {
            create_directory(&self.directory)?;
        }
        self.check_directory()?;
        let lock_path = self.directory.join(LOCK);
        let lock = create_file(&lock_path).map_err(|e| {
            if e.kind() == io::ErrorKind::AlreadyExists {
                Error::new("store-busy", "write.lock exists: another writer or interrupted write. Verify no writer is running, back up profiles.json, then inspect/remove write.lock and pending.json manually; never reclaimed automatically", 5)
            } else { Error::io_at("create writer lock", &lock_path, e) }
        })?;
        drop(lock);
        // Errors retain our sentinel: a failed/uncertain write requires review.
        let mut snapshot = self.read()?;
        if let Err(error) = self.prepare(&mut snapshot, expected_revision, &edit) {
            // No write has started: a revision race does not abandon our lock.
            fs::remove_file(&lock_path)
                .map_err(|e| Error::io_at("remove unused writer lock", &lock_path, e))?;
            return Err(error);
        }
        let pending_path = self.directory.join(PENDING);
        if check_file(&pending_path)? {
            return Err(Error::new("recovery-required", "pending.json exists; preserve prior profiles.json and inspect interrupted write manually", 5));
        }
        let bytes = serde_json::to_vec_pretty(&snapshot).map_err(Error::io)?;
        let mut pending = create_file(&pending_path)
            .map_err(|e| Error::io_at("create pending snapshot", &pending_path, e))?;
        pending
            .write_all(&bytes)
            .map_err(|e| Error::io_at("write pending snapshot", &pending_path, e))?;
        pending
            .sync_all()
            .map_err(|e| Error::io_at("sync pending snapshot", &pending_path, e))?;
        // Close before rename, including on Windows.
        drop(pending);
        // Recheck paths; never delete the destination to work around rename errors.
        self.check_directory()?;
        check_file(&self.directory.join(DATA))?;
        fs::rename(&pending_path, self.directory.join(DATA)).map_err(|e| {
            Error::io(format!(
                "replace snapshot {} -> {}: {e}",
                pending_path.display(),
                self.directory.join(DATA).display()
            ))
        })?;
        #[cfg(unix)]
        File::open(&self.directory).and_then(|f| f.sync_all()).map_err(|e| {
            Error::new("commit-uncertain", format!("Replacement occurred but directory sync failed: {e}; reread store before retrying"), 4)
        })?;
        fs::remove_file(&lock_path).map_err(|e| {
            Error::new(
                "commit-uncertain",
                format!(
                    "Store replaced but lock cleanup failed: {e}; reread store before retrying"
                ),
                4,
            )
        })?;
        Ok(snapshot)
    }
    fn prepare(&self, snapshot: &mut Snapshot, expected: u64, edit: &Edit) -> Result<(), Error> {
        if snapshot.store_revision != expected {
            return Err(Error::new("store-conflict", format!("Expected store revision {expected}, found {}; reread and review before retrying", snapshot.store_revision), 5));
        }
        let next = increment(snapshot.store_revision)?;
        match edit {
            Edit::Save { id, name, config } => {
                validate_name(name)?;
                config.validate().map_err(Error::policy)?;
                // Keep a future revision available for load/restore.
                increment(snapshot.config_revision.max(config.revision))?;
                snapshot.config_revision = snapshot.config_revision.max(config.revision);
                if let Some(id) = id {
                    let profile = snapshot
                        .profiles
                        .iter_mut()
                        .find(|p| &p.id == id)
                        .ok_or_else(|| {
                            Error::new(
                                "profile-not-found",
                                format!("Unknown stable profile ID: {id}"),
                                3,
                            )
                        })?;
                    profile.name = name.clone();
                    profile.config = config.clone();
                } else {
                    let id = format!("profile-{next}");
                    if snapshot.profiles.iter().any(|p| p.id == id) {
                        return Err(Error::new(
                            "corrupt-store",
                            "Generated profile ID already exists",
                            3,
                        ));
                    }
                    snapshot.profiles.push(Profile {
                        id,
                        name: name.clone(),
                        config: config.clone(),
                    });
                }
            }
            Edit::Rename { id, name } => {
                validate_name(name)?;
                let profile = snapshot
                    .profiles
                    .iter_mut()
                    .find(|p| &p.id == id)
                    .ok_or_else(|| {
                        Error::new(
                            "profile-not-found",
                            format!("Unknown stable profile ID: {id}"),
                            3,
                        )
                    })?;
                profile.name = name.clone();
            }
            Edit::Draft(config) => snapshot.replace_draft(config.clone())?,
            Edit::Load { id } => snapshot.replace_draft(snapshot.profile(id)?.config.clone())?,
            Edit::Undo => {
                let previous = snapshot
                    .previous_draft
                    .clone()
                    .ok_or_else(|| Error::new("no-undo", "No previous validated draft", 3))?;
                snapshot.replace_draft(previous)?;
            }
        }
        snapshot.store_revision = next;
        snapshot.validate()
    }
    fn check_directory(&self) -> Result<bool, Error> {
        check_directory(&self.directory, &[DATA, LOCK, PENDING])
    }
}

#[cfg(all(feature = "desktop", target_os = "linux"))]
#[derive(Clone, Debug)]
pub struct CurrentSnapshot {
    pub store_revision: u64,
    pub config_revision: u64,
    pub config_sha256: String,
    pub config: Config,
}

#[cfg(all(feature = "desktop", target_os = "linux"))]
impl CurrentSnapshot {
    fn matches(&self, expected: &Self) -> Result<(), Error> {
        if self.store_revision != expected.store_revision {
            return Err(Error::new(
                "stale-store",
                "Canonical store revision changed",
                5,
            ));
        }
        if self.config_revision != expected.config_revision
            || self.config_sha256 != expected.config_sha256
        {
            return Err(Error::new(
                "stale-config",
                "Canonical desired draft changed",
                5,
            ));
        }
        Ok(())
    }
}

/// Drop deliberately retains write.lock. Timeout, disconnect, or loss of this
/// value proves nothing about whether a native install can still take effect.
#[cfg(all(feature = "desktop", target_os = "linux"))]
#[derive(Debug)]
pub struct CurrentSnapshotGuard {
    directory: PathBuf,
    directory_identity: (u64, u64),
    lock_identity: (u64, u64),
    lock: Option<File>,
}

#[cfg(all(feature = "desktop", target_os = "linux"))]
impl CurrentSnapshotGuard {
    /// The caller must establish actual native owner completion/barrier proof,
    /// or reap/disconnect both owned children, before calling this method.
    pub fn release(&mut self) -> Result<(), Error> {
        use std::os::unix::fs::MetadataExt;
        if self.lock.is_none() {
            return Ok(());
        }
        check_directory(&self.directory, &[DATA, LOCK, PENDING])?;
        let directory = fs::symlink_metadata(&self.directory).map_err(Error::io)?;
        let path = self.directory.join(LOCK);
        check_file(&path)?;
        let metadata = fs::symlink_metadata(&path).map_err(Error::io)?;
        if (directory.dev(), directory.ino()) != self.directory_identity
            || (metadata.dev(), metadata.ino()) != self.lock_identity
        {
            return Err(Error::new(
                "store-lock-changed",
                "Owned writer lock identity changed; no path removed",
                5,
            ));
        }
        fs::remove_file(&path).map_err(|e| Error::io_at("release owned writer lock", &path, e))?;
        self.lock = None;
        Ok(())
    }
}

fn validate_name(name: &str) -> Result<(), Error> {
    if name.trim().is_empty() {
        return Err(Error::new(
            "invalid-name",
            "Profile name must not be blank",
            3,
        ));
    }
    Ok(())
}
/// Resolve without touching disk. An unset/nonabsolute base is an error.
pub fn default_directory() -> Result<PathBuf, Error> {
    let base = if cfg!(windows) {
        env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        env::var_os("XDG_DATA_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    }
    .filter(|p| p.is_absolute())
    .ok_or_else(|| {
        Error::new(
            "store-path-unavailable",
            "Set an absolute per-user data directory or pass --store DIRECTORY explicitly",
            4,
        )
    })?;
    Ok(base.join("mixyard/profiles"))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub identity: Value,
    pub before: Option<Value>,
    pub after: Option<Value>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Changes {
    pub sources: Vec<Change>,
    pub sends: Vec<Change>,
    pub mixes: Vec<Change>,
    pub masters: Vec<Change>,
    pub outputs: Vec<Change>,
    pub media_controls: Vec<Change>,
}
#[derive(Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub struct SensitivePath {
    pub source: Option<String>,
    pub selector: Option<Selector>,
    pub mix: String,
    pub output: String,
    pub target: Target,
    pub reason: &'static str,
}
/// Conservative desired-path review, including muted/zero and unavailable paths.
/// Virtual inputs may feed recording; arbitrary labels carry no policy meaning.
pub fn sensitive_paths(config: &Config) -> Vec<SensitivePath> {
    let mut paths = BTreeSet::new();
    for output in &config.outputs {
        let recording = matches!(output.target, Target::VirtualInput { .. });
        let sends: Vec<_> = config
            .sends
            .iter()
            .filter(|s| s.mix == output.mix)
            .collect();
        for send in &sends {
            if let Some(source) = config.sources.iter().find(|s| s.id == send.source) {
                if recording || matches!(source.selector, Selector::Mic { .. }) {
                    paths.insert(SensitivePath {
                        source: Some(source.id.clone()),
                        selector: Some(source.selector.clone()),
                        mix: output.mix.clone(),
                        output: output.id.clone(),
                        target: output.target.clone(),
                        reason: if recording {
                            "potential-recording"
                        } else {
                            "microphone-monitoring"
                        },
                    });
                }
            }
        }
        if recording && sends.is_empty() {
            paths.insert(SensitivePath {
                source: None,
                selector: None,
                mix: output.mix.clone(),
                output: output.id.clone(),
                target: output.target.clone(),
                reason: "potential-recording",
            });
        }
    }
    paths.into_iter().collect()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub changes: Changes,
    pub affected_sensitive_paths_before: Vec<SensitivePath>,
    pub affected_sensitive_paths_after: Vec<SensitivePath>,
    pub sensitive_paths: Vec<SensitivePath>,
    pub unavailable_identities: Value,
    pub plan: Plan,
}
fn diff<T: Serialize>(before: &[T], after: &[T], keys: &[&str]) -> Result<Vec<Change>, Error> {
    let index = |items: &[T]| -> Result<BTreeMap<String, (Value, Value)>, Error> {
        items
            .iter()
            .map(|item| {
                let value = serde_json::to_value(item).map_err(Error::io)?;
                let identity = Value::Array(keys.iter().map(|key| value[key].clone()).collect());
                Ok((identity.to_string(), (identity, value)))
            })
            .collect()
    };
    let before = index(before)?;
    let after = index(after)?;
    let keys: BTreeSet<_> = before.keys().chain(after.keys()).collect();
    Ok(keys
        .into_iter()
        .filter_map(|key| {
            let b = before.get(key);
            let a = after.get(key);
            if b == a {
                return None;
            }
            Some(Change {
                identity: b.or(a).unwrap().0.clone(),
                before: b.map(|x| x.1.clone()),
                after: a.map(|x| x.1.clone()),
            })
        })
        .collect())
}
pub fn preview(
    snapshot: &Snapshot,
    candidate: &Config,
    inventory: &Inventory,
) -> Result<Preview, Error> {
    snapshot.validate()?;
    candidate.validate().map_err(Error::policy)?;
    let mut candidate = candidate.clone();
    candidate.revision = increment(snapshot.config_revision.max(candidate.revision))?;
    let routing = plan(&candidate, inventory).map_err(Error::policy)?;
    let empty = Config {
        schema_version: 2,
        revision: 0,
        sources: vec![],
        mixes: vec![],
        sends: vec![],
        outputs: vec![],
        media_controls: vec![],
    };
    let before = snapshot.draft.as_ref().unwrap_or(&empty);
    let changes = Changes {
        sources: diff(&before.sources, &candidate.sources, &["id"])?,
        sends: diff(&before.sends, &candidate.sends, &["source", "mix"])?,
        mixes: diff(&before.mixes, &candidate.mixes, &["id"])?,
        masters: diff(
            &before
                .mixes
                .iter()
                .map(|m| serde_json::json!({"id": m.id, "master": m.master}))
                .collect::<Vec<_>>(),
            &candidate
                .mixes
                .iter()
                .map(|m| serde_json::json!({"id": m.id, "master": m.master}))
                .collect::<Vec<_>>(),
            &["id"],
        )?,
        outputs: diff(&before.outputs, &candidate.outputs, &["id"])?,
        media_controls: diff(
            &before.media_controls,
            &candidate.media_controls,
            &["deviceUid"],
        )?,
    };
    let affected = |p: &SensitivePath| {
        changes
            .sources
            .iter()
            .any(|c| p.source.as_ref().is_some_and(|s| c.identity[0] == *s))
            || changes.sends.iter().any(|c| c.identity[1] == p.mix)
            || changes.mixes.iter().any(|c| c.identity[0] == p.mix)
            || changes.outputs.iter().any(|c| c.identity[0] == p.output)
    };
    let paths = sensitive_paths(&candidate);
    let unavailable_sources: Vec<_> = routing
        .sources
        .iter()
        .filter(|r| r.status != ResolutionStatus::Selected)
        .collect();
    let unavailable_outputs: Vec<_> = routing
        .outputs
        .iter()
        .filter(|r| r.status != ResolutionStatus::Selected)
        .collect();
    Ok(Preview {
        affected_sensitive_paths_before: sensitive_paths(before)
            .into_iter()
            .filter(&affected)
            .collect(),
        affected_sensitive_paths_after: sensitive_paths(&candidate)
            .into_iter()
            .filter(&affected)
            .collect(),
        changes,
        sensitive_paths: paths,
        unavailable_identities: serde_json::json!({"sources": unavailable_sources, "outputs": unavailable_outputs}),
        plan: routing,
    })
}

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "desktop", target_os = "linux"))]
mod private_guard_tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let mut nonce = [0; 16];
            getrandom::fill(&mut nonce).unwrap();
            let path = PathBuf::from("/dev/shm").join(format!(
                "mixyard-private-guard-{}-{:x}",
                std::process::id(),
                u128::from_ne_bytes(nonce)
            ));
            create_directory(&path).unwrap();
            Self(path)
        }
        fn store(&self) -> Store {
            Store::new(self.0.join("profiles")).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn config() -> Config {
        serde_json::from_str(include_str!("../examples/config.json")).unwrap()
    }
    #[test]
    fn guard_excludes_canonical_writers_until_explicit_owner_completion() {
        let fixture = Fixture::new();
        let store = fixture.store();
        store.edit(0, Edit::Draft(config())).unwrap();
        let snapshot = store.current_snapshot(65536).unwrap();
        let mut guard = store.guard_current_snapshot(&snapshot, 65536).unwrap();
        assert_eq!(store.current_snapshot(65536).unwrap().store_revision, 1); // Status/read does not lock.
        assert_eq!(
            store.edit(1, Edit::Draft(config())).unwrap_err().code,
            "store-busy"
        );
        guard.release().unwrap();
        store.edit(1, Edit::Draft(config())).unwrap();
        assert_eq!(
            store
                .guard_current_snapshot(&snapshot, 65536)
                .unwrap_err()
                .code,
            "stale-store"
        );
        assert!(!fixture.0.join("profiles/write.lock").exists());
        let current = store.current_snapshot(65536).unwrap();
        drop(store.guard_current_snapshot(&current, 65536).unwrap());
        assert_eq!(
            store.edit(2, Edit::Draft(config())).unwrap_err().code,
            "store-busy"
        );
    }
    #[test]
    fn snapshot_changed_between_precheck_and_writer_acquisition_is_rejected_under_guard() {
        let fixture = Fixture::new();
        let store = fixture.store();
        store.edit(0, Edit::Draft(config())).unwrap();
        let expected = store.current_snapshot(65536).unwrap();
        let error = store
            .guard_current_snapshot_with(&expected, 65536, || {
                store.edit(1, Edit::Draft(config())).unwrap();
            })
            .unwrap_err();
        assert_eq!(error.code, "stale-store");
        assert!(!fixture.0.join("profiles/write.lock").exists());
        store.edit(2, Edit::Draft(config())).unwrap();
    }
    #[test]
    fn digest_mismatch_and_replaced_sentinel_never_delete_unknown_lock() {
        let fixture = Fixture::new();
        let store = fixture.store();
        store.edit(0, Edit::Draft(config())).unwrap();
        let mut expected = store.current_snapshot(65536).unwrap();
        expected.config_sha256 = "0".repeat(64);
        assert_eq!(
            store
                .guard_current_snapshot(&expected, 65536)
                .unwrap_err()
                .code,
            "stale-config"
        );
        let current = store.current_snapshot(65536).unwrap();
        let mut guard = store.guard_current_snapshot(&current, 65536).unwrap();
        let path = fixture.0.join("profiles/write.lock");
        fs::remove_file(&path).unwrap();
        let replacement = create_file(&path).unwrap();
        assert_eq!(guard.release().unwrap_err().code, "store-lock-changed");
        assert!(path.exists());
        drop(replacement);
    }
}

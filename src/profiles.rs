//! Local desired drafts only. No backend, native consent, or media resume state.
use crate::{plan, Config, Inventory, Plan, ResolutionStatus, Selector, Target};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

const FORMAT: &str = "mixyard-local-drafts";
const DATA: &str = "profiles.json";
const LOCK: &str = "write.lock";
const PENDING: &str = "pending.json";

#[derive(Debug)]
pub struct Error {
    pub code: &'static str,
    pub message: String,
    pub exit_code: u8,
}
impl Error {
    fn new(code: &'static str, message: impl Into<String>, exit_code: u8) -> Self {
        Self {
            code,
            message: message.into(),
            exit_code,
        }
    }
    fn io(error: impl std::fmt::Display) -> Self {
        Self::new("io-error", error.to_string(), 4)
    }
    fn policy(error: impl Into<String>) -> Self {
        Self::new("invalid-policy", error, 3)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Error {}

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
        let file = File::open(&path).map_err(Error::io)?;
        let parsed = if let Some(limit) = max_bytes {
            let mut bytes = Vec::new();
            file.take((limit as u64).saturating_add(1))
                .read_to_end(&mut bytes)
                .map_err(Error::io)?;
            if bytes.len() > limit {
                return Err(Error::new("store-too-large", format!("Store exceeds the {limit}-byte desktop read limit; nothing was truncated. Use the profiles CLI for larger stores."), 3));
            }
            serde_json::from_slice(&bytes)
        } else {
            serde_json::from_reader(file)
        };
        let snapshot: Snapshot = parsed.map_err(|e| {
            if e.is_io() {
                Error::io(e)
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
            } else { Error::io(e) }
        })?;
        drop(lock);
        // Errors retain our sentinel: a failed/uncertain write requires review.
        let mut snapshot = self.read()?;
        if let Err(error) = self.prepare(&mut snapshot, expected_revision, &edit) {
            // No write has started: a revision race does not abandon our lock.
            fs::remove_file(&lock_path).map_err(Error::io)?;
            return Err(error);
        }
        let pending_path = self.directory.join(PENDING);
        if check_file(&pending_path)? {
            return Err(Error::new("recovery-required", "pending.json exists; preserve prior profiles.json and inspect interrupted write manually", 5));
        }
        let bytes = serde_json::to_vec_pretty(&snapshot).map_err(Error::io)?;
        let mut pending = create_file(&pending_path).map_err(Error::io)?;
        pending.write_all(&bytes).map_err(Error::io)?;
        pending.sync_all().map_err(Error::io)?;
        // Close before rename, including on Windows.
        drop(pending);
        // Recheck paths; never delete the destination to work around rename errors.
        self.check_directory()?;
        check_file(&self.directory.join(DATA))?;
        fs::rename(&pending_path, self.directory.join(DATA)).map_err(Error::io)?;
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
        check_ancestors(&self.directory)?;
        let meta = match fs::symlink_metadata(&self.directory) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(Error::io(e)),
        };
        if !meta.is_dir() {
            return Err(unsafe_path(&self.directory));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o077 != 0 {
                return Err(unsafe_path(&self.directory));
            }
        }
        for entry in fs::read_dir(&self.directory).map_err(Error::io)? {
            let entry = entry.map_err(Error::io)?;
            if ![DATA, LOCK, PENDING]
                .iter()
                .any(|name| entry.file_name() == *name)
            {
                return Err(Error::new(
                    "foreign-store-path",
                    "Store directory contains unrelated files; use a dedicated empty directory",
                    4,
                ));
            }
            check_file(&entry.path())?;
        }
        Ok(true)
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
fn unsafe_path(path: &Path) -> Error {
    Error::new("unsafe-store-path", format!("{}: expected a private directory or regular single-link file, without symlinks/reparse points", path.display()), 4)
}
fn check_ancestors(path: &Path) -> Result<(), Error> {
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) => {
                if is_link(&meta) || !meta.is_dir() {
                    return Err(unsafe_path(ancestor));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::io(e)),
        }
    }
    Ok(())
}
fn is_link(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return true;
        } // FILE_ATTRIBUTE_REPARSE_POINT
    }
    meta.file_type().is_symlink()
}
fn check_file(path: &Path) -> Result<bool, Error> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(Error::io(e)),
    };
    if is_link(&meta) || !meta.is_file() {
        return Err(unsafe_path(path));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if meta.nlink() != 1 || meta.permissions().mode() & 0o077 != 0 {
            return Err(unsafe_path(path));
        }
    }
    Ok(true)
}
fn create_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}
fn create_directory(path: &Path) -> Result<(), Error> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(Error::io)
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

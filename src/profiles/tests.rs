use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        // Canonicalize the system temp root (macOS /var is a system symlink).
        let root = env::temp_dir().canonicalize().unwrap();
        let path = root.join(format!(
            "mixyard-profiles-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        create_directory(&path).unwrap();
        Self(path)
    }
    fn store(&self) -> Store {
        Store::new(self.0.join("store")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn config() -> Config {
    serde_json::from_str(include_str!("../../examples/config.json")).unwrap()
}
fn inventory() -> Inventory {
    serde_json::from_str(include_str!("../../examples/inventory.json")).unwrap()
}
fn save(name: &str, config: Config) -> Edit {
    Edit::Save {
        id: None,
        name: name.into(),
        config,
    }
}
fn bytes(store: &Store) -> Vec<u8> {
    fs::read(store.directory.join(DATA)).unwrap()
}
fn put(path: &Path, bytes: &[u8]) {
    let mut f = create_file(path).unwrap();
    f.write_all(bytes).unwrap();
}

#[test]
fn arbitrary_names_stable_ids_repeated_atomic_saves_and_no_count_limit() {
    let fixture = Fixture::new();
    let store = fixture.store();
    assert_eq!(store.read().unwrap().store_revision, 0);
    assert!(!store.directory.exists());
    let mut snapshot = store.edit(0, save("../夜 / Record ♥", config())).unwrap();
    let id = snapshot.profiles[0].id.clone();
    for revision in 1..35 {
        snapshot = store
            .edit(revision, save("same display name", config()))
            .unwrap();
        assert!(!store.directory.join(PENDING).exists());
        assert!(!store.directory.join(LOCK).exists());
        assert_eq!(store.read().unwrap().store_revision, revision + 1);
    }
    assert_eq!(snapshot.profiles.len(), 35);
    snapshot = store
        .edit(
            35,
            Edit::Rename {
                id: id.clone(),
                name: "custom new name".into(),
            },
        )
        .unwrap();
    assert_eq!(snapshot.profiles[0].id, id);
    assert_eq!(snapshot.profiles[0].name, "custom new name");
    let before = serde_json::to_value(&snapshot.profiles[0].config).unwrap();
    let after = serde_json::to_value(config()).unwrap();
    assert_eq!(before, after); // Gains, exclusions, media policy and consent preserved.
    snapshot = store
        .edit(
            36,
            Edit::Save {
                id: Some(id.clone()),
                name: "overwrite only by ID and revision".into(),
                config: config(),
            },
        )
        .unwrap();
    assert_eq!(snapshot.profiles.len(), 35);
    assert_eq!(snapshot.profiles[0].id, id);
}

#[test]
fn undo_validated_only_restores_fresh_revision_and_bounds_overflow() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let first = store.edit(0, Edit::Draft(config())).unwrap();
    let initial = first.draft.as_ref().unwrap().revision;
    let original = bytes(&store);
    let mut invalid = config();
    invalid.sends[0].gain = 2.0;
    assert_eq!(
        store.edit(1, Edit::Draft(invalid)).unwrap_err().code,
        "invalid-policy"
    );
    assert_eq!(bytes(&store), original);
    assert_eq!(store.edit(1, Edit::Undo).unwrap_err().code, "no-undo");
    let mut modified = config();
    modified.mixes[0].name = "arbitrary renamed mix".into();
    modified.mixes[0].master.gain = 0.5;
    let second = store.edit(1, Edit::Draft(modified)).unwrap();
    assert!(second.draft.as_ref().unwrap().revision > initial);
    let restored = store.edit(2, Edit::Undo).unwrap();
    assert!(restored.draft.as_ref().unwrap().revision > second.draft.unwrap().revision);
    let mut restored_config = restored.draft.unwrap();
    restored_config.revision = 1;
    assert_eq!(
        serde_json::to_value(restored_config).unwrap(),
        serde_json::to_value(config()).unwrap()
    );
    let again = store.edit(3, Edit::Undo).unwrap();
    assert_eq!(again.draft.unwrap().mixes[0].master.gain, 0.5);
    let original = bytes(&store);
    let mut huge = config();
    huge.revision = u64::MAX;
    assert_eq!(
        store.edit(4, Edit::Draft(huge)).unwrap_err().code,
        "revision-exhausted"
    );
    assert_eq!(bytes(&store), original);
    let mut snapshot = store.read().unwrap();
    snapshot.store_revision = u64::MAX;
    assert_eq!(
        store
            .prepare(&mut snapshot, u64::MAX, &Edit::Undo)
            .unwrap_err()
            .code,
        "revision-exhausted"
    );
}

#[test]
fn load_missing_devices_preserves_config_and_returns_sensitive_review() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let saved = store
        .edit(0, save("No names imply policy", config()))
        .unwrap();
    let loaded = store
        .edit(
            1,
            Edit::Load {
                id: saved.profiles[0].id.clone(),
            },
        )
        .unwrap();
    let mut inventory = inventory();
    inventory.sources.clear();
    inventory.outputs.clear();
    let preview = preview(&loaded, loaded.draft.as_ref().unwrap(), &inventory).unwrap();
    assert!(preview.changes.sources.is_empty());
    assert!(preview.changes.outputs.is_empty());
    assert_eq!(
        preview.unavailable_identities["sources"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        preview.unavailable_identities["outputs"]
            .as_array()
            .unwrap()
            .len(),
        7
    );
    assert!(preview
        .sensitive_paths
        .iter()
        .any(|p| p.reason == "microphone-monitoring"));
    assert!(preview
        .sensitive_paths
        .iter()
        .any(|p| p.reason == "potential-recording" && p.source.as_deref() == Some("spotify")));
    assert_eq!(preview.plan.mode, "dry-run");
    assert!(
        loaded
            .draft
            .unwrap()
            .outputs
            .iter()
            .find(|o| o.id == "headphones")
            .unwrap()
            .allow_mic_monitor
    );
}

#[test]
fn preview_reports_all_changes_and_affected_removed_paths_without_writes() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let snapshot = store.edit(0, Edit::Draft(config())).unwrap();
    let before = bytes(&store);
    let mut candidate = config();
    candidate.sources[0].selector = Selector::Mic {
        device_uid: "missing-new-mic".into(),
    };
    candidate.sends[0].gain = 0.25;
    candidate.mixes[0].master.gain = 0.5;
    candidate.mixes[0].name = "not a recording label".into();
    candidate.outputs.retain(|o| o.id != "headphones");
    let result = preview(&snapshot, &candidate, &inventory()).unwrap();
    assert_eq!(result.changes.sources.len(), 1);
    assert_eq!(result.changes.sends.len(), 1);
    assert_eq!(result.changes.masters.len(), 1);
    assert_eq!(result.changes.outputs.len(), 1);
    assert!(result
        .affected_sensitive_paths_before
        .iter()
        .any(|p| p.output == "headphones"));
    assert!(result
        .affected_sensitive_paths_after
        .iter()
        .any(|p| p.output == "call-input"));
    assert_eq!(bytes(&store), before);
    assert!(!store.directory.join(LOCK).exists());
    let absent = Store::new(fixture.0.join("absent")).unwrap();
    preview(&absent.read().unwrap(), &config(), &inventory()).unwrap();
    assert!(!absent.directory.exists());
}

#[test]
fn concurrent_writers_and_stale_revisions_never_silently_overwrite() {
    let fixture = Fixture::new();
    let store = fixture.store();
    store.edit(0, save("first", config())).unwrap();
    let before = bytes(&store);
    assert_eq!(
        store.edit(0, save("stale", config())).unwrap_err().code,
        "store-conflict"
    );
    assert_eq!(bytes(&store), before);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let handles: Vec<_> = (0..2)
        .map(|n| {
            let barrier = barrier.clone();
            let path = store.directory.clone();
            std::thread::spawn(move || {
                barrier.wait();
                Store::new(path)
                    .unwrap()
                    .edit(1, save(&format!("writer-{n}"), config()))
            })
        })
        .collect();
    barrier.wait();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(results
        .iter()
        .filter_map(|r| r.as_ref().err())
        .all(|e| matches!(e.code, "store-busy" | "store-conflict")));
    let snapshot = store.read().unwrap();
    assert_eq!(snapshot.store_revision, 2);
    assert_eq!(snapshot.profiles.len(), 2);
}

#[test]
fn interrupted_partial_and_complete_pending_writes_preserve_prior_store() {
    for partial in [
        b"{\"format\":".as_slice(),
        b"complete but uncommitted".as_slice(),
    ] {
        let fixture = Fixture::new();
        let store = fixture.store();
        store.edit(0, save("prior", config())).unwrap();
        let before = bytes(&store);
        put(&store.directory.join(LOCK), b"abandoned");
        put(&store.directory.join(PENDING), partial);
        assert_eq!(store.read().unwrap().store_revision, 1);
        assert_eq!(
            store.edit(1, save("must fail", config())).unwrap_err().code,
            "store-busy"
        );
        assert_eq!(bytes(&store), before);
        assert_eq!(fs::read(store.directory.join(PENDING)).unwrap(), partial);
        fs::remove_file(store.directory.join(LOCK)).unwrap();
        assert_eq!(
            store
                .edit(1, save("still must fail", config()))
                .unwrap_err()
                .code,
            "recovery-required"
        );
        assert_eq!(bytes(&store), before);
    }
}

#[test]
fn corrupt_unknown_version_foreign_files_and_media_history_are_not_overwritten() {
    for data in [
        b"{".as_slice(),
        b"{}".as_slice(),
        b"{\"schemaVersion\":999}".as_slice(),
    ] {
        let fixture = Fixture::new();
        let store = fixture.store();
        create_directory(&store.directory).unwrap();
        put(&store.directory.join(DATA), data);
        assert_eq!(
            store.edit(0, save("fail", config())).unwrap_err().code,
            "corrupt-store"
        );
        assert_eq!(bytes(&store), data);
    }
    for (key, value, code) in [
        ("schemaVersion", serde_json::json!(99), "unsupported-store"),
        (
            "mediaResumeHistory",
            serde_json::json!(["private"]),
            "corrupt-store",
        ),
    ] {
        let fixture = Fixture::new();
        let store = fixture.store();
        store.edit(0, save("prior", config())).unwrap();
        let mut data: Value = serde_json::from_slice(&bytes(&store)).unwrap();
        data[key] = value;
        fs::write(store.directory.join(DATA), data.to_string()).unwrap();
        let before = bytes(&store);
        assert_eq!(
            store.edit(1, save("fail", config())).unwrap_err().code,
            code
        );
        assert_eq!(bytes(&store), before);
    }
    let fixture = Fixture::new();
    let store = fixture.store();
    create_directory(&store.directory).unwrap();
    put(&store.directory.join("foreign.txt"), b"not ours");
    assert_eq!(
        store.edit(0, save("fail", config())).unwrap_err().code,
        "foreign-store-path"
    );
    assert_eq!(
        fs::read(store.directory.join("foreign.txt")).unwrap(),
        b"not ours"
    );
}

#[cfg(unix)]
#[test]
fn symlink_hardlink_and_shared_directory_paths_are_rejected() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let fixture = Fixture::new();
    let store = fixture.store();
    store.edit(0, save("prior", config())).unwrap();
    let before = bytes(&store);
    let link = fixture.0.join("link");
    symlink(&store.directory, &link).unwrap();
    assert_eq!(Store::new(link).unwrap_err().code, "unsafe-store-path");
    for name in [DATA, LOCK, PENDING] {
        let other = fixture.0.join(format!("store-{name}"));
        create_directory(&other).unwrap();
        symlink(store.directory.join(DATA), other.join(name)).unwrap();
        let other = Store::new(other).unwrap();
        assert_eq!(
            other.edit(0, save("fail", config())).unwrap_err().code,
            "unsafe-store-path"
        );
        assert_eq!(bytes(&store), before);
    }
    let foreign = fixture.0.join("foreign");
    fs::hard_link(store.directory.join(DATA), &foreign).unwrap();
    assert_eq!(
        store.edit(1, save("fail", config())).unwrap_err().code,
        "unsafe-store-path"
    );
    fs::remove_file(foreign).unwrap();
    fs::set_permissions(&store.directory, fs::Permissions::from_mode(0o777)).unwrap();
    assert_eq!(store.read().unwrap_err().code, "unsafe-store-path");
}

#[test]
fn atomic_replace_failure_never_deletes_destination() {
    let fixture = Fixture::new();
    let destination = fixture.0.join("destination");
    create_directory(&destination).unwrap();
    put(&destination.join("keep"), b"foreign contents");
    let pending = fixture.0.join(PENDING);
    put(&pending, b"new bytes");
    assert!(fs::rename(&pending, &destination).is_err());
    assert_eq!(
        fs::read(destination.join("keep")).unwrap(),
        b"foreign contents"
    );
    assert_eq!(fs::read(&pending).unwrap(), b"new bytes");
}

#[cfg(windows)]
#[test]
fn windows_destination_without_delete_sharing_blocks_atomic_replace() {
    use std::os::windows::fs::OpenOptionsExt;
    let fixture = Fixture::new();
    let store = fixture.store();
    store.edit(0, save("prior", config())).unwrap();
    let before = bytes(&store);
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(store.directory.join(DATA))
        .unwrap();
    assert_eq!(
        store.edit(1, save("blocked", config())).unwrap_err().code,
        "io-error"
    );
    assert_eq!(bytes(&store), before);
    drop(held);
    assert!(store.directory.join(LOCK).exists());
    assert!(store.directory.join(PENDING).exists());
}

#[test]
fn abrupt_writer_fixture() {
    let Some(directory) = env::var_os("MIXYARD_TEST_CRASH_STORE") else {
        return;
    };
    let store = Store::new(PathBuf::from(directory)).unwrap();
    let mut snapshot = store.read().unwrap();
    let revision = snapshot.store_revision;
    store
        .prepare(&mut snapshot, revision, &save("uncommitted", config()))
        .unwrap();
    let _lock = create_file(&store.directory.join(LOCK)).unwrap();
    let pending = serde_json::to_vec(&snapshot).unwrap();
    let bytes = if env::var_os("MIXYARD_TEST_CRASH_PARTIAL").is_some() {
        &pending[..pending.len() / 2]
    } else {
        &pending[..]
    };
    put(&store.directory.join(PENDING), bytes);
    // Terminate without destructors, as an interrupted process would.
    std::process::exit(77);
}

#[test]
fn abrupt_process_exit_before_rename_preserves_prior_snapshot() {
    for partial in [false, true] {
        let fixture = Fixture::new();
        let store = fixture.store();
        store.edit(0, save("prior", config())).unwrap();
        let before = bytes(&store);
        let mut command = std::process::Command::new(env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "profiles::tests::abrupt_writer_fixture",
                "--nocapture",
            ])
            .env("MIXYARD_TEST_CRASH_STORE", &store.directory)
            .env_remove("MIXYARD_TEST_CRASH_PARTIAL");
        if partial {
            command.env("MIXYARD_TEST_CRASH_PARTIAL", "1");
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(77));
        assert_eq!(bytes(&store), before);
        assert_eq!(store.read().unwrap().store_revision, 1);
        assert_eq!(
            store
                .edit(1, save("no automatic recovery", config()))
                .unwrap_err()
                .code,
            "store-busy"
        );
        if !partial {
            let pending: Snapshot =
                serde_json::from_slice(&fs::read(store.directory.join(PENDING)).unwrap()).unwrap();
            pending.validate().unwrap();
            assert_eq!(pending.store_revision, 2);
        }
    }
}

#[test]
fn undo_does_not_reauthorize_core_media_resume_history() {
    use crate::{plan_media, MediaAction, MediaRequest, PlaybackState};
    let fixture = Fixture::new();
    let store = fixture.store();
    let first = store.edit(0, Edit::Draft(config())).unwrap();
    let mut inventory = inventory();
    let mut request: MediaRequest =
        serde_json::from_str(include_str!("../../examples/media-request.json")).unwrap();
    request.action = MediaAction::PlayPause;
    let paused = plan_media(first.draft.as_ref().unwrap(), &inventory, &request).unwrap();
    assert!(!paused.remember_on_confirmed_pause.is_empty());
    request.remembered_playing_session_ids = paused.remember_on_confirmed_pause;
    request.remembered_playing_context = paused.remember_on_confirmed_pause_context;
    for session in &mut inventory.media_sessions {
        session.playback_state = PlaybackState::Paused;
    }
    let mut edited = config();
    edited.sends.retain(|s| s.mix != "desk-a");
    store.edit(1, Edit::Draft(edited)).unwrap();
    let undone = store.edit(2, Edit::Undo).unwrap();
    let result = plan_media(undone.draft.as_ref().unwrap(), &inventory, &request).unwrap();
    assert_eq!(result.status, "no-resume-history");
    assert!(result.remembered_playing_session_ids.is_empty());
    assert!(result.players.iter().all(|p| p.planned_action.is_none()));
    assert_eq!(
        result.unavailable_remembered_session_ids,
        request.remembered_playing_session_ids
    );
    let data: Value = serde_json::from_slice(&bytes(&store)).unwrap();
    assert!(data.get("rememberedPlayingSessionIds").is_none());
    assert!(data.get("rememberedPlayingContext").is_none());
}

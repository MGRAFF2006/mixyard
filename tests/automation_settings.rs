//! Actual private files, competing writers and restart/recovery boundaries.
use mixyard::{
    automation::{evaluate, parse_request, settings::*, Action, Evidence, FactKey, Node, Override},
    profiles,
};
use serde_json::{json, Value as Json};
use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Barrier,
    },
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = env::temp_dir().canonicalize().unwrap().join(format!(
            "mixyard-settings-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn store(&self) -> Store {
        Store::new(self.0.join("settings")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn bytes(store: &Store) -> Vec<u8> {
    fs::read(store.directory().join("settings.json")).unwrap()
}
fn private_file(path: &Path, bytes: &[u8]) {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}
fn declaration(name: &str, subject: &str, value: bool) -> Declaration {
    Declaration {
        key: FactKey {
            name: name.into(),
            subject: Some(subject.into()),
        },
        value,
    }
}
fn policy() -> Settings {
    let request = parse_request(include_bytes!(
        "../examples/automation/interface-monitor.json"
    ))
    .unwrap();
    Settings {
        graph: Some(request.graph),
        catalog: request.catalog,
        declarations: vec![
            declaration(
                "device.hardware-monitor-declared",
                "  Device/♥:EXACT  ",
                false,
            ),
            declaration("manual.flag", "call:manual", true),
        ],
        ..Settings::default()
    }
}
fn code<T: std::fmt::Debug>(result: Result<T, mixyard::profiles::Error>, expected: &str) {
    assert_eq!(result.unwrap_err().code, expected);
}

#[test]
fn read_defaults_create_nothing_and_writes_reopen_losslessly() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let default = store.read().unwrap();
    assert_eq!(default, mixyard::automation::settings::Snapshot::default());
    assert!(!store.directory().exists());
    assert_eq!(
        default.settings.transition_policy,
        TransitionPolicy {
            settle_ms: Milliseconds(1000),
            minimum_hold_ms: Milliseconds(2000),
            cooldown_ms: Milliseconds(2000)
        }
    );
    let mut settings = policy();
    let graph = settings.graph.as_mut().unwrap();
    graph.id = "  opaque GRAPH ♥  ".into();
    graph.revision = u64::MAX;
    let Node::Terminal {
        action: Action::Select { overrides, .. },
        ..
    } = graph
        .nodes
        .iter_mut()
        .find(|n| {
            matches!(
                n,
                Node::Terminal {
                    action: Action::Select { .. },
                    ..
                }
            )
        })
        .unwrap()
    else {
        panic!()
    };
    overrides.push(Override::Output {
        output: "  output ♥ /EXACT  ".into(),
        mute: None,
        gain: Some(0.12345678901234568),
    });
    let executable = env::current_exe().unwrap().to_str().unwrap().to_owned();
    settings
        .observer
        .applications
        .insert(executable, "  APP ♥ exact  ".into());
    settings.observer.host_label = Some("lab ♥ host".into());
    settings.transition_policy.cooldown_ms = Milliseconds(MAX_DURATION_MS);
    let saved = store.write(0, &settings).unwrap();
    assert_eq!(saved.settings_revision, 1);
    assert_eq!(saved.settings, settings);
    assert_eq!(
        Store::new(store.directory()).unwrap().read().unwrap(),
        saved
    );
    let record: Json = serde_json::from_slice(&bytes(&store)).unwrap();
    assert_eq!(record["format"], FORMAT);
    assert_eq!(record["schemaVersion"], 1);
    assert!(record["settings"].get("armed").is_none());
    assert_eq!(fs::read_dir(store.directory()).unwrap().count(), 1);
    // A real second replacement must preserve f64 and opaque identities too.
    let saved2 = store.write(1, &settings).unwrap();
    assert_eq!(saved2.settings_revision, 2);
    assert_eq!(store.read().unwrap().settings, settings);
}

#[test]
fn custom_profile_siblings_never_pollute_or_overlap_profile_store() {
    let fixture = Fixture::new();
    for name in ["profiles", "other profiles ♥", "situation-settings"] {
        let parent = fixture.0.join(format!("case-{}", name));
        fs::create_dir(&parent).unwrap();
        let profile_dir = parent.join(name);
        let profile = profiles::Store::new(&profile_dir).unwrap();
        let config = serde_json::from_str(include_str!("../examples/config.json")).unwrap();
        profile.edit(0, profiles::Edit::Draft(config)).unwrap();
        let before = fs::read(profile_dir.join("profiles.json")).unwrap();
        let settings = Store::sibling_of_profiles(&profile_dir).unwrap();
        assert_eq!(settings.directory().parent(), profile_dir.parent());
        assert_ne!(settings.directory(), profile_dir);
        settings
            .write(
                settings.read().unwrap().settings_revision,
                &Settings::default(),
            )
            .unwrap();
        assert_eq!(profile.read().unwrap().store_revision, 1);
        assert_eq!(fs::read(profile_dir.join("profiles.json")).unwrap(), before);
        assert_eq!(fs::read_dir(profile_dir).unwrap().count(), 1);
    }
    assert!(!fixture.0.join("unused").exists());
    Store::sibling_of_profiles(fixture.0.join("unused"))
        .unwrap()
        .read()
        .unwrap();
    assert!(!fixture.0.join("unused.situation-settings").exists());
}

#[test]
fn concurrent_expected_revision_writers_have_one_winner() {
    let fixture = Fixture::new();
    let store = fixture.store();
    store.write(0, &Settings::default()).unwrap();
    let gate = Arc::new(Barrier::new(9));
    let threads: Vec<_> = (0..8)
        .map(|n| {
            let directory = store.directory().to_owned();
            let gate = gate.clone();
            std::thread::spawn(move || {
                let settings = Settings {
                    declarations: vec![declaration("manual.flag", &format!("writer-{n}"), true)],
                    ..Settings::default()
                };
                gate.wait();
                Store::new(directory).unwrap().write(1, &settings)
            })
        })
        .collect();
    gate.wait();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(results
        .iter()
        .filter_map(|r| r.as_ref().err())
        .all(|e| matches!(e.code, "store-busy" | "store-conflict")));
    assert_eq!(store.read().unwrap().settings_revision, 2);
    assert_eq!(fs::read_dir(store.directory()).unwrap().count(), 1);
    let before = bytes(&store);
    code(store.write(1, &Settings::default()), "store-conflict");
    assert_eq!(bytes(&store), before);
}

#[test]
fn crash_writer_fixture() {
    let Some(path) = env::var_os("MIXYARD_SETTINGS_CRASH_FIXTURE") else {
        return;
    };
    let path = PathBuf::from(path);
    private_file(&path.join("write.lock"), b"");
    private_file(
        &path.join("pending.json"),
        b"{\"format\":\"mixyard-situation-settings\",",
    );
    std::process::exit(23); // exit without destructors, as an interrupted writer would
}

#[test]
fn interrupted_writer_and_orphan_pending_never_promote_or_reclaim() {
    let fixture = Fixture::new();
    let store = fixture.store();
    store.write(0, &policy()).unwrap();
    let before = bytes(&store);
    let status = std::process::Command::new(env::current_exe().unwrap())
        .args(["--exact", "crash_writer_fixture"])
        .env("MIXYARD_SETTINGS_CRASH_FIXTURE", store.directory())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(23));
    assert_eq!(store.read().unwrap().settings_revision, 1);
    code(store.write(1, &Settings::default()), "store-busy");
    assert_eq!(bytes(&store), before);
    assert!(store.directory().join("pending.json").exists());
    // Simulate an operator removing only the sentinel; pending still blocks writes.
    fs::remove_file(store.directory().join("write.lock")).unwrap();
    code(store.write(1, &Settings::default()), "recovery-required");
    assert_eq!(bytes(&store), before);
    assert!(store.directory().join("write.lock").exists());
    assert!(store.directory().join("pending.json").exists());
    // A complete, valid pending record is still not permission to promote it.
    let mut uncommitted = store.read().unwrap();
    uncommitted.settings_revision = 2;
    uncommitted.settings = Settings::default();
    let pending = serde_json::to_vec(&uncommitted).unwrap();
    fs::write(store.directory().join("pending.json"), &pending).unwrap();
    fs::remove_file(store.directory().join("write.lock")).unwrap();
    code(store.write(1, &Settings::default()), "recovery-required");
    assert_eq!(bytes(&store), before);
    assert_eq!(
        fs::read(store.directory().join("pending.json")).unwrap(),
        pending
    );
    // Missing committed data does not make interrupted artifacts a clean store.
    fs::remove_file(store.directory().join("settings.json")).unwrap();
    assert_eq!(store.read().unwrap().settings_revision, 0);
    code(store.write(0, &Settings::default()), "store-busy");
    assert!(!store.directory().join("settings.json").exists());
}

#[test]
fn overflow_corrupt_unknown_and_foreign_records_preserve_prior_bytes() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let mut snapshot = store.write(0, &policy()).unwrap();
    snapshot.settings_revision = u64::MAX;
    fs::write(
        store.directory().join("settings.json"),
        serde_json::to_vec(&snapshot).unwrap(),
    )
    .unwrap();
    let before = bytes(&store);
    code(
        store.write(u64::MAX, &Settings::default()),
        "revision-exhausted",
    );
    assert_eq!(bytes(&store), before);
    let valid = serde_json::to_value(snapshot).unwrap();
    let mut records = vec![(b"{broken".to_vec(), "corrupt-store")];
    for (field, value) in [
        ("schemaVersion", json!(2)),
        ("format", json!("foreign-format")),
        ("armed", json!(true)),
    ] {
        let mut record = valid.clone();
        record[field] = value;
        records.push((
            serde_json::to_vec(&record).unwrap(),
            if field == "armed" {
                "corrupt-store"
            } else {
                "unsupported-store"
            },
        ));
    }
    for (record, error) in records {
        fs::write(store.directory().join("settings.json"), &record).unwrap();
        code(store.read(), error);
        code(store.write(u64::MAX, &Settings::default()), error);
        assert_eq!(bytes(&store), record);
        assert!(!store.directory().join("write.lock").exists());
    }
    fs::write(store.directory().join("settings.json"), &before).unwrap();
    private_file(&store.directory().join("foreign.json"), b"keep");
    code(store.read(), "foreign-store-path");
    code(
        store.write(u64::MAX, &Settings::default()),
        "foreign-store-path",
    );
    assert_eq!(bytes(&store), before);
}

#[test]
fn declarations_are_only_explicit_booleans_never_fact_envelopes_or_native_evidence() {
    let mut settings = Settings {
        declarations: vec![
            declaration("device.hardware-monitor-declared", "EXACT ♥", false),
            declaration("manual.flag", "EXACT ♥", true),
        ],
        ..Settings::default()
    };
    let json = serde_json::to_vec(&settings).unwrap();
    assert_eq!(parse_settings(&json).unwrap(), settings);
    for field in [
        "evidence",
        "provider",
        "observedAtMs",
        "validForMs",
        "status",
        "completeness",
        "reason",
    ] {
        let mut input: Json = serde_json::from_slice(&json).unwrap();
        input["declarations"][0][field] = json!("platform-observed");
        assert!(
            parse_settings(&serde_json::to_vec(&input).unwrap()).is_err(),
            "{field}"
        );
    }
    settings.declarations.push(settings.declarations[0].clone());
    assert!(settings.validate().is_err());
    settings.declarations.pop();
    for name in [
        "device.connected",
        "device.kind",
        "application.call-active",
        "unknown",
        "Manual.flag",
    ] {
        settings.declarations[0].key.name = name.into();
        assert!(settings.validate().is_err());
    }
    settings.declarations[0].key.name = "manual.flag".into();
    for subject in [None, Some(" ".into()), Some("x".repeat(1025))] {
        settings.declarations[0].key.subject = subject;
        assert!(settings.validate().is_err());
    }
    // Restart preserves intent, but does not store a native observation or timestamp.
    let fixture = Fixture::new();
    let store = fixture.store();
    store.write(0, &policy()).unwrap();
    let reopened = Store::new(store.directory()).unwrap().read().unwrap();
    assert!(!reopened.settings.declarations[0].value);
    let mut request = parse_request(include_bytes!(
        "../examples/automation/interface-monitor.json"
    ))
    .unwrap();
    request.snapshot.facts[2].evidence = Evidence::PlatformObserved;
    assert_eq!(
        evaluate(&request).unwrap().terminal.as_deref(),
        Some("hold")
    );
    // This store cannot launder native/imported evidence into a declaration.
    let mut input = serde_json::to_value(&reopened.settings).unwrap();
    input["declarations"][0] = serde_json::to_value(&request.snapshot.facts[2]).unwrap();
    assert!(parse_settings(&serde_json::to_vec(&input).unwrap()).is_err());
}

#[test]
fn settings_admit_core_limits_before_duplicate_scans_and_reject_invalid_policy() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let mut settings = policy();
    let graph = settings.graph.as_mut().unwrap();
    graph.nodes = vec![graph.nodes[0].clone(); 50_000];
    code(store.write(0, &settings), "resource-limit");
    assert!(!store.directory().exists());
    settings = policy();
    settings.catalog = vec![settings.catalog[0].clone(); 50_000];
    code(store.write(0, &settings), "resource-limit");
    settings = policy();
    settings.graph.as_mut().unwrap().root = "missing".into();
    assert!(settings.validate().is_err());
    settings = policy();
    settings.graph = None;
    assert!(settings.validate().is_err());
    settings = Settings::default();
    settings.declarations = vec![declaration("manual.flag", "same", true); 50_000];
    assert!(settings.validate().is_err());
    assert!(!store.directory().exists());
}

#[test]
fn observer_and_duration_options_match_collector_without_executing_helpers() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let mut settings = Settings::default();
    // An absolute executable mapping need not exist; validation must not execute it.
    let executable = fixture
        .0
        .join("must-not-execute")
        .to_str()
        .unwrap()
        .to_owned();
    settings
        .observer
        .applications
        .insert(executable.clone(), "opaque application".into());
    settings.validate().unwrap();
    let options = mixyard::context::Options {
        applications: settings.observer.applications.clone(),
        host_label: settings.observer.host_label.clone(),
        timezone: settings.observer.timezone.clone(),
        helper: fixture.0.join("no-helper"),
        linux_adapter: fixture.0.join("no-adapter"),
    };
    mixyard::context::Collector::new(options).unwrap();
    for field in ["helper", "linuxAdapter"] {
        let mut input = serde_json::to_value(&settings).unwrap();
        input["observer"][field] = json!(executable);
        assert!(parse_settings(&serde_json::to_vec(&input).unwrap()).is_err());
    }
    for field in ["armed", "autoWrite", "authority", "captureConsent"] {
        let mut input = serde_json::to_value(&settings).unwrap();
        input[field] = json!(true);
        assert!(parse_settings(&serde_json::to_vec(&input).unwrap()).is_err());
    }
    settings
        .observer
        .applications
        .insert("relative".into(), "app".into());
    assert!(settings.validate().is_err());
    settings.observer.applications.remove("relative");
    for n in 0..33 {
        settings.observer.applications.insert(
            fixture.0.join(format!("exe-{n}")).to_str().unwrap().into(),
            "app".into(),
        );
    }
    assert!(settings.validate().is_err());
    settings.observer.applications.clear();
    settings.observer.timezone = " ".into();
    assert!(settings.validate().is_err());
    settings.observer.timezone = "Europe/Berlin".into();
    settings.observer.host_label = Some("x".repeat(1025));
    assert!(settings.validate().is_err());
    settings.observer.host_label = None;
    for n in [0, MAX_DURATION_MS] {
        settings.transition_policy.settle_ms = Milliseconds(n);
        settings.validate().unwrap();
    }
    settings.transition_policy.settle_ms = Milliseconds(MAX_DURATION_MS + 1);
    assert!(store.write(0, &settings).is_err());
    assert!(!store.directory().exists());
}

#[test]
fn byte_bound_rejects_disk_and_direct_rust_records_without_truncation() {
    let fixture = Fixture::new();
    let store = fixture.store();
    store.write(0, &Settings::default()).unwrap();
    let before = bytes(&store);
    // Valid individual declarations but escaping makes the full record exceed 1 MiB.
    let settings = Settings {
        declarations: (0..512)
            .map(|n| {
                declaration(
                    "manual.flag",
                    &format!("{}-{n}", "\u{1}".repeat(1000)),
                    true,
                )
            })
            .collect(),
        ..Settings::default()
    };
    settings.validate().unwrap();
    code(store.write(1, &settings), "store-too-large");
    assert_eq!(bytes(&store), before);
    assert!(!store.directory().join("write.lock").exists());
    let too_large = vec![b' '; MAX_STORE_BYTES + 1];
    fs::write(store.directory().join("settings.json"), &too_large).unwrap();
    code(store.read(), "store-too-large");
    code(store.write(1, &Settings::default()), "store-too-large");
    assert_eq!(bytes(&store), too_large);
}

#[cfg(unix)]
#[test]
fn unix_permissions_links_and_ancestors_preserve_store_and_link_targets() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let fixture = Fixture::new();
    let store = fixture.store();
    store.write(0, &policy()).unwrap();
    let before = bytes(&store);
    assert_eq!(
        fs::metadata(store.directory())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let data = store.directory().join("settings.json");
    assert_eq!(
        fs::metadata(&data).unwrap().permissions().mode() & 0o777,
        0o600
    );
    fs::set_permissions(&data, fs::Permissions::from_mode(0o644)).unwrap();
    code(store.read(), "unsafe-store-path");
    code(store.write(1, &Settings::default()), "unsafe-store-path");
    fs::set_permissions(&data, fs::Permissions::from_mode(0o600)).unwrap();
    let hardlink = fixture.0.join("hardlink");
    fs::hard_link(&data, &hardlink).unwrap();
    code(store.read(), "unsafe-store-path");
    fs::remove_file(hardlink).unwrap();
    let prior = fixture.0.join("prior");
    fs::rename(&data, &prior).unwrap();
    symlink(&prior, &data).unwrap();
    code(store.read(), "unsafe-store-path");
    code(store.write(1, &Settings::default()), "unsafe-store-path");
    assert_eq!(fs::read(&prior).unwrap(), before);
    fs::remove_file(&data).unwrap();
    fs::rename(prior, &data).unwrap();
    for name in ["write.lock", "pending.json"] {
        symlink(&data, store.directory().join(name)).unwrap();
        code(store.write(1, &Settings::default()), "unsafe-store-path");
        fs::remove_file(store.directory().join(name)).unwrap();
    }
    fs::set_permissions(store.directory(), fs::Permissions::from_mode(0o755)).unwrap();
    code(store.read(), "unsafe-store-path");
    fs::set_permissions(store.directory(), fs::Permissions::from_mode(0o700)).unwrap();
    let linked = fixture.0.join("linked");
    symlink(store.directory(), &linked).unwrap();
    code(Store::new(linked.join("child")), "unsafe-store-path");
    assert_eq!(bytes(&store), before);
}

#[cfg(unix)]
#[test]
fn failed_write_to_readonly_private_directory_preserves_prior_snapshot() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let store = fixture.store();
    store.write(0, &policy()).unwrap();
    let before = bytes(&store);
    fs::set_permissions(store.directory(), fs::Permissions::from_mode(0o500)).unwrap();
    let result = store.write(1, &Settings::default());
    fs::set_permissions(store.directory(), fs::Permissions::from_mode(0o700)).unwrap();
    if unsafe { libc::geteuid() } != 0 {
        code(result, "io-error");
        assert_eq!(bytes(&store), before);
    }
}

#[cfg(windows)]
#[test]
fn failed_windows_replacement_preserves_destination_and_requires_recovery() {
    use std::os::windows::fs::OpenOptionsExt;
    let fixture = Fixture::new();
    let store = fixture.store();
    store.write(0, &policy()).unwrap();
    let before = bytes(&store);
    // Allow reads but deny deletion/replacement while held, using the real kernel.
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(store.directory().join("settings.json"))
        .unwrap();
    code(store.write(1, &Settings::default()), "io-error");
    assert_eq!(bytes(&store), before);
    assert!(store.directory().join("write.lock").exists());
    assert!(store.directory().join("pending.json").exists());
    drop(held);
    code(store.write(1, &Settings::default()), "store-busy");
    assert_eq!(bytes(&store), before);
}

#[test]
fn strict_json_rejects_duplicate_fields_wrong_scalars_and_incomplete_records() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let valid = serde_json::to_string(&Settings::default()).unwrap();
    let duplicate_map = valid.replace(
        "\"applications\":{}",
        "\"applications\":{\"/absolute/exe\":\"first\",\"/absolute/exe\":\"second\"}",
    );
    assert!(parse_settings(duplicate_map.as_bytes()).is_err());
    let duplicate_field = valid.replacen("\"catalog\":[]", "\"catalog\":[],\"catalog\":[]", 1);
    assert!(parse_settings(duplicate_field.as_bytes()).is_err());
    for field in ["catalog", "declarations", "transitionPolicy", "observer"] {
        let mut input: Json = serde_json::from_str(&valid).unwrap();
        input.as_object_mut().unwrap().remove(field);
        assert!(parse_settings(&serde_json::to_vec(&input).unwrap()).is_err());
    }
    for value in [json!(-1), json!(1.5), json!("1000"), json!(null)] {
        let mut input: Json = serde_json::from_str(&valid).unwrap();
        input["transitionPolicy"]["settleMs"] = value;
        assert!(parse_settings(&serde_json::to_vec(&input).unwrap()).is_err());
    }
    // Corruption on disk must also be rejected before a replacement is attempted.
    store.write(0, &Settings::default()).unwrap();
    let snapshot: Json = serde_json::from_slice(&bytes(&store)).unwrap();
    let mut malformed = snapshot.clone();
    malformed["settings"] = json!({});
    let record = serde_json::to_vec(&malformed).unwrap();
    fs::write(store.directory().join("settings.json"), &record).unwrap();
    code(store.write(1, &Settings::default()), "corrupt-store");
    assert_eq!(bytes(&store), record);
    assert!(!store.directory().join("write.lock").exists());
}

#[test]
fn extracted_filesystem_checks_preserve_legacy_unbounded_profile_reads() {
    let fixture = Fixture::new();
    let profile_dir = fixture.0.join("profiles");
    let profiles = profiles::Store::new(&profile_dir).unwrap();
    let config = serde_json::from_str(include_str!("../examples/config.json")).unwrap();
    let name = "legacy name ".repeat(MAX_STORE_BYTES / 4);
    profiles
        .edit(
            0,
            profiles::Edit::Save {
                id: None,
                name: name.clone(),
                config,
            },
        )
        .unwrap();
    assert!(
        fs::metadata(profile_dir.join("profiles.json"))
            .unwrap()
            .len()
            > MAX_STORE_BYTES as u64
    );
    assert_eq!(profiles.read().unwrap().profiles[0].name, name);
    code(profiles.read_bounded(MAX_STORE_BYTES), "store-too-large");
    let settings = Store::sibling_of_profiles(&profile_dir).unwrap();
    settings.write(0, &Settings::default()).unwrap();
    assert_eq!(profiles.read().unwrap().profiles[0].name, name);
}

//! Black-box CLI tests always pass --store inside disposable temporary directories.
use serde_json::{json, Value};
use std::{
    env, fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
    store: PathBuf,
    config: PathBuf,
    inventory: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = env::temp_dir().canonicalize().unwrap().join(format!(
            "mixyard-profiles-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let config = root.join("config.json");
        let inventory = root.join("inventory.json");
        fs::write(&config, include_str!("../examples/config.json")).unwrap();
        fs::write(&inventory, include_str!("../examples/inventory.json")).unwrap();
        Self {
            store: root.join("store"),
            root,
            config,
            inventory,
        }
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_mixyard"))
            .arg("profiles")
            .arg("--store")
            .arg(&self.store)
            .args(args)
            // Any accidental use of the default would be confined to this fixture.
            .env("HOME", &self.root)
            .env("LOCALAPPDATA", &self.root)
            .env("XDG_DATA_HOME", &self.root)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["schemaVersion"], 1);
        assert_eq!(value["audioApplied"], false);
        assert_eq!(value["confirmedWorkingSetup"], Value::Null);
        assert_eq!(value["storedConsentIsFreshNativeConsent"], false);
        value
    }
    fn error(&self, args: &[&str], exit: i32, code: &str) {
        let output = self.run(args);
        assert_eq!(output.status.code(), Some(exit));
        assert!(output.stdout.is_empty());
        let value: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(value["schemaVersion"], 2);
        assert_eq!(value["code"], code);
        assert_eq!(value["exitCode"], exit);
    }
    fn config_path(&self) -> &str {
        self.config.to_str().unwrap()
    }
    fn inventory_path(&self) -> &str {
        self.inventory.to_str().unwrap()
    }
    fn bytes(&self) -> Vec<u8> {
        fs::read(self.store.join("profiles.json")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn cli_save_rename_preview_load_undo_and_preserve_stored_policy() {
    let fixture = Fixture::new();
    let listed = fixture.ok(&["list"]);
    assert_eq!(listed["storeRevision"], 0);
    assert!(!fixture.store.exists());
    fixture.ok(&["show"]);
    assert!(!fixture.store.exists());
    let config = fixture.config_path();
    let inventory = fixture.inventory_path();
    let saved = fixture.ok(&["save", "0", "../custom ♥ named", config]);
    let id = saved["result"]["profiles"][0]["id"].as_str().unwrap();
    assert_eq!(saved["reviewRequiredBeforeApply"], true);
    assert_eq!(saved["result"]["draft"], Value::Null);
    let stored_config = saved["result"]["profiles"][0]["config"].clone();
    let renamed = fixture.ok(&["rename", "1", id, "custom / entirely new name"]);
    assert_eq!(renamed["result"]["profiles"][0]["id"], id);
    assert_eq!(renamed["result"]["profiles"][0]["config"], stored_config);
    let before = fixture.bytes();
    let preview = fixture.ok(&["preview-profile", id, inventory]);
    assert_eq!(preview["storeRevision"], 2);
    assert_eq!(fixture.bytes(), before);
    assert_eq!(preview["result"]["plan"]["schemaVersion"], 2);
    assert_eq!(preview["result"]["plan"]["mode"], "dry-run");
    let loaded = fixture.ok(&["load", "2", id]);
    let revision = loaded["result"]["draft"]["revision"].as_u64().unwrap();
    assert!(revision > stored_config["revision"].as_u64().unwrap());
    let mut draft = loaded["result"]["draft"].clone();
    draft["revision"] = stored_config["revision"].clone();
    assert_eq!(draft, stored_config);
    let mut candidate: Value =
        serde_json::from_str(include_str!("../examples/config.json")).unwrap();
    candidate["mixes"][0]["master"] = json!({"gain":0.5,"mute":false});
    candidate["sends"][0]["gain"] = json!(0.25);
    candidate["outputs"][0]["mute"] = json!(true);
    candidate["sources"][0]["selector"]["deviceUid"] = json!("unavailable:new-mic");
    fs::write(&fixture.config, candidate.to_string()).unwrap();
    let before = fixture.bytes();
    let input_before = fs::read(&fixture.config).unwrap();
    let preview = fixture.ok(&["preview", config, inventory]);
    assert_eq!(
        preview["result"]["changes"]["sources"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        preview["result"]["changes"]["sends"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        preview["result"]["changes"]["masters"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        preview["result"]["changes"]["outputs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        preview["result"]["unavailableIdentities"]["sources"][0]["selector"]["deviceUid"],
        "unavailable:new-mic"
    );
    assert!(!preview["result"]["affectedSensitivePathsAfter"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(fixture.bytes(), before);
    assert_eq!(fs::read(&fixture.config).unwrap(), input_before);
    let changed = fixture.ok(&["draft", "3", config]);
    assert!(changed["result"]["draft"]["revision"].as_u64().unwrap() > revision);
    let before = fixture.bytes();
    fixture.ok(&["preview-undo", inventory]);
    assert_eq!(fixture.bytes(), before);
    let restored = fixture.ok(&["undo", "4"]);
    assert!(
        restored["result"]["draft"]["revision"].as_u64().unwrap()
            > changed["result"]["draft"]["revision"].as_u64().unwrap()
    );
    let mut restored_config = restored["result"]["draft"].clone();
    restored_config["revision"] = stored_config["revision"].clone();
    assert_eq!(restored_config, stored_config);
    let entries: Vec<_> = fs::read_dir(&fixture.store)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(entries, vec!["profiles.json"]);
    assert!(!String::from_utf8(fixture.bytes())
        .unwrap()
        .contains("resumeHistory"));
}

#[test]
fn cli_preview_missing_devices_and_errors_never_create_or_overwrite_stores() {
    let fixture = Fixture::new();
    let config = fixture.config_path();
    let inventory = fixture.inventory_path();
    let mut missing: Value =
        serde_json::from_str(include_str!("../examples/inventory.json")).unwrap();
    missing["sources"] = json!([]);
    missing["outputs"] = json!([]);
    fs::write(&fixture.inventory, missing.to_string()).unwrap();
    let preview = fixture.ok(&["preview", config, inventory]);
    assert_eq!(
        preview["result"]["unavailableIdentities"]["outputs"]
            .as_array()
            .unwrap()
            .len(),
        7
    );
    assert_eq!(preview["reviewRequiredBeforeApply"], true);
    assert!(!fixture.store.exists());
    fixture.error(&["save", "not-a-revision", "name", config], 2, "usage");
    fixture.error(&["undo", "0"], 3, "no-undo");
    fixture.error(&["load", "0", "missing-id"], 3, "profile-not-found");
    assert!(!fixture.store.exists());
    let mut invalid: Value = serde_json::from_str(include_str!("../examples/config.json")).unwrap();
    invalid["outputs"][2]["allowMicMonitor"] = json!(false);
    fs::write(&fixture.config, invalid.to_string()).unwrap();
    fixture.error(&["save", "0", "bad policy", config], 3, "invalid-policy");
    assert!(!fixture.store.exists());
    fs::write(&fixture.config, "{").unwrap();
    fixture.error(&["save", "0", "bad json", config], 3, "invalid-json");
    fs::write(&fixture.config, include_str!("../examples/config.json")).unwrap();
    fixture.ok(&["save", "0", "prior", config]);
    let before = fixture.bytes();
    fixture.error(&["save", "0", "stale", config], 5, "store-conflict");
    assert_eq!(fixture.bytes(), before);
    fs::write(fixture.store.join("profiles.json"), "corrupt").unwrap();
    fixture.error(&["save", "1", "fail", config], 3, "corrupt-store");
    assert_eq!(fixture.bytes(), b"corrupt");
}

#[test]
fn cli_two_process_writers_use_revision_checks_and_lock() {
    let fixture = Fixture::new();
    fixture.ok(&["save", "0", "first", fixture.config_path()]);
    let spawn = |name| {
        Command::new(env!("CARGO_BIN_EXE_mixyard"))
            .arg("profiles")
            .arg("--store")
            .arg(&fixture.store)
            .args(["save", "1", name, fixture.config_path()])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    };
    let first = spawn("first concurrent");
    let second = spawn("second concurrent");
    let outputs = [
        first.wait_with_output().unwrap(),
        second.wait_with_output().unwrap(),
    ];
    assert_eq!(outputs.iter().filter(|o| o.status.success()).count(), 1);
    let failure = outputs.iter().find(|o| !o.status.success()).unwrap();
    assert_eq!(failure.status.code(), Some(5));
    let error: Value = serde_json::from_slice(&failure.stderr).unwrap();
    assert!(matches!(
        error["code"].as_str().unwrap(),
        "store-busy" | "store-conflict"
    ));
    let listed = fixture.ok(&["list"]);
    assert_eq!(listed["storeRevision"], 2);
    assert_eq!(listed["result"]["profiles"].as_array().unwrap().len(), 2);
    assert!(!fixture.store.join("write.lock").exists());
}

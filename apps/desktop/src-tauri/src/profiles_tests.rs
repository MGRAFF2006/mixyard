use super::*;
use mixyard::profiles::Edit;
use serde_json::{json, Value};
use std::{fs, path::PathBuf, sync::atomic::AtomicU64};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "mixyard-desktop-profiles-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> PathBuf {
        self.0.join("store")
    }
    fn store(&self) -> Store {
        Store::new(self.path()).unwrap()
    }
    fn list(&self) -> Result<Value, String> {
        run_at(Request::List {}, &self.path()).map(|r| serde_json::from_str(&r).unwrap())
    }
    fn preview(&self, rev: u64, id: &str, inv: &Inventory) -> Result<Value, String> {
        run_at(
            Request::Preview {
                store_revision: rev,
                profile_id: id.into(),
                inventory_json: serde_json::to_string(inv).unwrap(),
            },
            &self.path(),
        )
        .map(|r| serde_json::from_str(&r).unwrap())
    }
    fn save(&self, rev: u64, name: &str, c: Config) {
        self.store()
            .edit(
                rev,
                Edit::Save {
                    id: None,
                    name: name.into(),
                    config: c,
                },
            )
            .unwrap();
    }
    fn bytes(&self) -> Vec<u8> {
        fs::read(self.path().join("profiles.json")).unwrap()
    }
    fn replace_json(&self, value: Value) {
        fs::write(self.path().join("profiles.json"), value.to_string()).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn config() -> Config {
    serde_json::from_str(include_str!("../../../../examples/config.json")).unwrap()
}
fn inventory() -> Inventory {
    serde_json::from_str(include_str!("../../../../examples/inventory.json")).unwrap()
}

#[test]
fn list_missing_empty_and_duplicate_names_show_real_stable_ids_without_writes() {
    let f = Fixture::new();
    let missing = f.list().unwrap();
    assert_eq!(missing["storeState"], "missing");
    assert_eq!(missing["profiles"], json!([]));
    assert!(!f.path().exists());
    f.store().edit(0, Edit::Draft(config())).unwrap();
    assert_eq!(f.list().unwrap()["storeState"], "present");
    assert_eq!(f.list().unwrap()["profiles"], json!([]));
    for rev in 1..3 {
        f.save(rev, "../夜 / Record ♥", config());
    }
    let before = f.bytes();
    let result = f.list().unwrap();
    assert_eq!(result["profiles"][0]["name"], result["profiles"][1]["name"]);
    assert_ne!(result["profiles"][0]["id"], result["profiles"][1]["id"]);
    assert_eq!(
        result["profiles"][0]["config"],
        serde_json::to_value(config()).unwrap()
    );
    assert_eq!(f.bytes(), before);
    assert_eq!(fs::read_dir(f.path()).unwrap().count(), 1);
    assert_eq!(result["confirmedWorkingSetup"], Value::Null);
    assert_eq!(result["audioApplied"], false);
}
#[test]
fn preview_is_exact_core_comparison_with_current_draft_bound_to_revisions() {
    let f = Fixture::new();
    let mut candidate = config();
    candidate.sends[0].gain = 0.25;
    candidate.mixes[0].master.gain = 0.5;
    candidate.outputs.retain(|o| o.id != "headphones");
    f.save(0, "Candidate", candidate);
    f.store().edit(1, Edit::Draft(config())).unwrap();
    let snapshot = f.store().read().unwrap();
    let before = f.bytes();
    let inv = inventory();
    let reply = f.preview(2, "profile-1", &inv).unwrap();
    assert_eq!(
        reply["preview"],
        serde_json::to_value(
            profiles::preview(
                &snapshot,
                &snapshot.profile("profile-1").unwrap().config,
                &inv
            )
            .unwrap()
        )
        .unwrap()
    );
    assert_eq!(reply["storeRevision"], 2);
    assert_eq!(reply["profileId"], "profile-1");
    assert_eq!(reply["inventoryRevision"], inv.revision);
    assert_eq!(reply["draftRevision"], snapshot.draft.unwrap().revision);
    assert!(reply["preview"]["affectedSensitivePathsBefore"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["output"] == "headphones"));
    assert_eq!(f.bytes(), before);
    assert_eq!(fs::read_dir(f.path()).unwrap().count(), 1);
    f.store()
        .edit(
            2,
            Edit::Rename {
                id: "profile-1".into(),
                name: "Changed".into(),
            },
        )
        .unwrap();
    assert!(f
        .preview(2, "profile-1", &inv)
        .unwrap_err()
        .contains("store-conflict"));
    assert!(f
        .preview(3, "unknown", &inv)
        .unwrap_err()
        .contains("profile-not-found"));
}
#[test]
fn policy_rejection_and_unavailable_identities_remain_authoritative() {
    let f = Fixture::new();
    f.save(0, "Candidate", config());
    let mut inv = inventory();
    inv.sources.clear();
    inv.outputs.clear();
    let reply = f.preview(1, "profile-1", &inv).unwrap();
    assert_eq!(
        reply["preview"]["unavailableIdentities"]["sources"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        reply["preview"]["unavailableIdentities"]["outputs"]
            .as_array()
            .unwrap()
            .len(),
        7
    );
    inv.schema_version = 1;
    assert!(f
        .preview(1, "profile-1", &inv)
        .unwrap_err()
        .contains("schema-2"));
    let mut inv = inventory();
    inv.limits.max_output_routes = Some(0);
    assert!(f
        .preview(1, "profile-1", &inv)
        .unwrap_err()
        .contains("invalid-policy"));
}
#[test]
fn corrupt_foreign_path_and_busy_states_never_become_empty_or_repaired() {
    let f = Fixture::new();
    f.save(0, "Candidate", config());
    let before = f.bytes();
    for marker in ["write.lock", "pending.json"] {
        let p = f.path().join(marker);
        fs::write(&p, b"do not repair").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert!(f.list().unwrap_err().contains("store-busy"));
        assert_eq!(fs::read(&p).unwrap(), b"do not repair");
        assert_eq!(f.bytes(), before);
        fs::remove_file(p).unwrap();
    }
    fs::write(f.path().join("profiles.json"), b"{").unwrap();
    assert!(f.list().unwrap_err().contains("corrupt-store"));
    assert_eq!(f.bytes(), b"{");
    fs::write(f.path().join("profiles.json"), &before).unwrap();
    fs::write(f.path().join("foreign.txt"), b"foreign").unwrap();
    assert!(f.list().unwrap_err().contains("foreign-store-path"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let link = f.0.join("link");
        symlink(f.path(), &link).unwrap();
        assert!(run_at(Request::List {}, &link)
            .unwrap_err()
            .contains("unsafe-store-path"));
    }
}
#[test]
fn store_read_is_bounded_before_parsing_and_original_read_remains_unchanged() {
    let f = Fixture::new();
    f.save(0, "x", config());
    let len = f.bytes().len();
    assert_eq!(f.store().read_bounded(len).unwrap().store_revision, 1);
    assert_eq!(
        f.store().read_bounded(len - 1).unwrap_err().code,
        "store-too-large"
    );
    let mut v: Value = serde_json::from_slice(&f.bytes()).unwrap();
    v["profiles"][0]["name"] = json!("x".repeat(MAX_STORE_BYTES));
    f.replace_json(v);
    let before = f.bytes();
    assert_eq!(f.store().read().unwrap().profiles.len(), 1);
    assert!(f.list().unwrap_err().contains("store-too-large"));
    assert_eq!(f.bytes(), before);
}
#[test]
fn exact_revisions_count_bounds_and_oversized_requests_fail_whole() {
    let f = Fixture::new();
    f.save(0, "x", config());
    let original: Value = serde_json::from_slice(&f.bytes()).unwrap();
    for key in ["storeRevision", "configRevision"] {
        let mut v = original.clone();
        v[key] = json!(MAX_SAFE_REVISION + 1);
        f.replace_json(v);
        assert!(f.list().unwrap_err().contains("exact integer"));
    }
    let mut at_limit = original.clone();
    at_limit["configRevision"] = json!(MAX_SAFE_REVISION);
    f.replace_json(at_limit);
    assert!(f
        .preview(1, "profile-1", &inventory())
        .unwrap_err()
        .contains("exact integer"));
    f.replace_json(original.clone());
    let mut inv = inventory();
    inv.revision = MAX_SAFE_REVISION + 1;
    assert!(f
        .preview(1, "profile-1", &inv)
        .unwrap_err()
        .contains("exact integer"));
    assert!(run_at(
        Request::Preview {
            store_revision: 1,
            profile_id: "profile-1".into(),
            inventory_json: " ".repeat(MAX_INPUT_BYTES + 1)
        },
        &f.path()
    )
    .unwrap_err()
    .contains("input-limit"));
    let mut v = original;
    v["profiles"] = json!((0..65).map(|i| json!({"id":format!("id-{i}"),"name":"x","config": {"schemaVersion":2,"revision":0,"sources":[],"mixes":[],"outputs":[],"sends":[],"mediaControls":[]}})).collect::<Vec<_>>());
    f.replace_json(v);
    assert!(f.list().unwrap_err().contains("Nothing was truncated"));
}
#[test]
fn admission_reply_and_request_surface_are_bounded() {
    let a = acquire().unwrap();
    assert!(acquire().is_err());
    drop(a);
    assert!(acquire().is_ok());
    let mut reply = Reply(Vec::new());
    assert!(reply.write_all(&vec![0; MAX_RESPONSE_BYTES]).is_ok());
    assert!(reply.write_all(&[0]).is_err());
    for v in [
        json!({"command":"list","path":"/etc"}),
        json!({"command":"save"}),
        json!({"command":"preview","storeRevision":1,"profileId":"x","inventoryJson":"{}","path":"/etc"}),
    ] {
        assert!(serde_json::from_value::<Request>(v).is_err());
    }
}

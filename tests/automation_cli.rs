use serde_json::Value;
use std::{path::Path, process::Command};
fn command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mixyard-automation"))
}
#[test]
fn explicit_fixture_evaluation_is_read_only_json_with_no_live_observation_claim() {
    for (path, terminal) in [
        ("headphone-priority.json", "headphones"),
        ("interface-monitor.json", "suppress"),
        ("application-schedule.json", "call"),
    ] {
        let output = command()
            .arg("evaluate")
            .arg(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("examples/automation")
                    .join(path),
            )
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["terminal"], terminal);
        assert_eq!(json["audioApplied"], false);
        assert_eq!(json["schemaVersion"], 1);
        assert!(json["observations"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["provider"].as_str().unwrap().starts_with("fixture:")));
    }
}
#[test]
fn catalog_usage_and_io_failures_have_explicit_exit_codes() {
    let output = command().arg("catalog").output().unwrap();
    assert!(output.status.success());
    let catalog: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(catalog
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["name"] == "device.hardware-monitor-declared"
            && d["evidencePolicy"] == "user-declaration"));
    assert_eq!(
        command().arg("observe").output().unwrap().status.code(),
        Some(2)
    );
    let output = command()
        .args(["evaluate", "missing-observation-file.json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["audioApplied"], false);
}

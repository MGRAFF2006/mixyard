//! Public CLI contract checks with independent synthetic identities and expectations.
use serde_json::{json, Value};
use std::{
    fs,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

fn fixture() -> (Value, Value) {
    (
        json!({"schemaVersion":2,"revision":17,
            "sources":[
                {"id":"voice","selector":{"kind":"mic","deviceUid":"mic:stable"}},
                {"id":"player","selector":{"kind":"app","applicationId":"app:music"}},
                {"id":"return","selector":{"kind":"call-return","applicationId":"app:call","group":"group:private"}}],
            "mixes":[{"id":"m:α","name":"自由 / Same","channels":2,"master":{"gain":0.5},"excludeGroups":["group:private"]}],
            "sends":[{"source":"voice","mix":"m:α","gain":0.4},{"source":"player","mix":"m:α","gain":0.2}],
            "outputs":[
                {"id":"o:a","mix":"m:α","target":{"kind":"device","deviceUid":"device:A"},"gain":0.5,"allowMicMonitor":true},
                {"id":"o:b","mix":"m:α","target":{"kind":"virtual-input","endpointId":"endpoint:B"},"gain":1}]
        }),
        json!({"schemaVersion":2,"revision":29,"platform":"linux","evidence":"simulated",
            "sources":[
                {"observedId":"obs:voice","selector":{"kind":"mic","deviceUid":"mic:stable"},"origin":"external","format":{"channels":1,"sampleRateHz":48000}},
                {"observedId":"obs:player","selector":{"kind":"app","applicationId":"app:music"},"origin":"external","format":{"channels":2,"sampleRateHz":48000}},
                {"observedId":"obs:return","selector":{"kind":"call-return","applicationId":"app:call","group":"group:private"},"origin":"external","format":{"channels":2,"sampleRateHz":48000}}],
            "outputs":[
                {"observedId":"obs:A","target":{"kind":"device","deviceUid":"device:A"},"format":{"channels":2,"sampleRateHz":48000}},
                {"observedId":"obs:B","target":{"kind":"virtual-input","endpointId":"endpoint:B"},"format":{"channels":2,"sampleRateHz":48000}}]
        }),
    )
}

fn run(command: &str, config: &Value, inventory: &Value, exit: i32) -> Value {
    static SERIAL: AtomicUsize = AtomicUsize::new(0);
    let directory = std::env::temp_dir().join(format!(
        "mixyard-diagnostics-{}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&directory).unwrap();
    let desired = directory.join("config.json");
    let observed = directory.join("inventory.json");
    fs::write(&desired, serde_json::to_vec(config).unwrap()).unwrap();
    fs::write(&observed, serde_json::to_vec(inventory).unwrap()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mixyard"))
        .args([command])
        .arg(desired)
        .arg(observed)
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();
    assert_eq!(
        output.status.code(),
        Some(exit),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    if exit == 0 {
        assert!(output.stderr.is_empty());
    } else {
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stderr).unwrap()["code"],
            "inspection-preflight-failed"
        );
    }
    serde_json::from_slice(&output.stdout).unwrap()
}
fn output<'a>(report: &'a Value, id: &str) -> &'a Value {
    report["outputs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["configured"]["id"] == id)
        .unwrap()
}
fn path<'a>(report: &'a Value, id: &str, source: &str) -> &'a Value {
    output(report, id)["paths"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["source"] == source)
        .unwrap()
}
fn reason(path: &Value, code: &str) -> bool {
    path["reasons"].as_array().unwrap().contains(&json!(code))
}
fn rejected(report: &Value) {
    assert_eq!(report["planAvailable"], false);
    assert!(report["plan"].is_null());
    for o in report["outputs"].as_array().unwrap() {
        for p in o["paths"].as_array().unwrap() {
            assert!(p["plannedEffectiveCoefficient"].is_null());
        }
    }
}

#[test]
fn actual_cli_trace_preserves_intent_and_unknown_live_state() {
    let (config, inventory) = fixture();
    let report = run("inspect", &config, &inventory, 0);
    assert_eq!(report, run("explain", &config, &inventory, 0));
    assert_eq!(report["schemaVersion"], 1);
    assert_eq!(report["configRevision"], 17);
    assert_eq!(report["inventoryRevision"], 29);
    assert_eq!(report["inventoryEvidence"], "simulated");
    assert_eq!(report["submixes"], "not-implemented");
    assert_eq!(output(&report, "o:a")["mix"]["name"], "自由 / Same");
    assert_eq!(
        path(&report, "o:a", "voice")["plannedEffectiveCoefficient"],
        0.1
    );
    assert_eq!(
        path(&report, "o:b", "voice")["plannedEffectiveCoefficient"],
        0.2
    );
    assert_eq!(
        path(&report, "o:a", "voice")["intendedPotentialSharing"],
        true
    );
    let excluded = path(&report, "o:b", "return");
    assert_eq!(excluded["excluded"], true);
    assert_eq!(excluded["intendedPotentialSharing"], false);
    assert!(reason(excluded, "excluded-call-return"));
    for o in report["outputs"].as_array().unwrap() {
        assert_eq!(o["consumer"], "unknown");
        assert_eq!(o["nativeState"], "unknown");
        for p in o["paths"].as_array().unwrap() {
            assert_eq!(p["appliedSharing"], "unknown");
        }
    }
    for s in report["sources"].as_array().unwrap() {
        assert_eq!(s["permission"], "unknown");
        assert_eq!(s["captureState"], "unknown");
    }
    assert!(report["liveLevels"].is_null());
    assert_eq!(report["playerPauseState"], "unknown");
    assert_eq!(report["captureState"], "unknown");
}

#[test]
fn independent_send_master_output_mutes_and_zeros_are_attributed() {
    for (stage, code, zero) in [
        ("send", "send-muted", false),
        ("send", "send-zero", true),
        ("master", "mix-master-muted", false),
        ("master", "mix-master-zero", true),
        ("output", "output-muted", false),
        ("output", "output-zero", true),
    ] {
        let (mut config, inventory) = fixture();
        let level = match stage {
            "send" => &mut config["sends"][0],
            "master" => &mut config["mixes"][0]["master"],
            _ => &mut config["outputs"][0],
        };
        if zero {
            level["gain"] = json!(0);
        } else {
            level["mute"] = json!(true);
        }
        let report = run("inspect", &config, &inventory, 0);
        let p = path(&report, "o:a", "voice");
        assert!(reason(p, code), "{stage}: {p}");
        assert_eq!(p["plannedEffectiveCoefficient"], 0.0);
        assert_eq!(p["intendedPotentialSharing"], true);
        if stage == "output" {
            assert_eq!(
                path(&report, "o:b", "voice")["plannedEffectiveCoefficient"],
                0.2
            );
            assert_eq!(
                path(&report, "o:a", "player")["plannedEffectiveCoefficient"],
                0.0
            );
        } else if stage == "send" {
            assert_eq!(
                path(&report, "o:a", "player")["plannedEffectiveCoefficient"],
                0.05
            );
        }
    }
}

#[test]
fn missing_ambiguous_and_unsupported_are_distinct_without_fallback() {
    for (kind, code, source) in [
        ("missing-source", "source-missing", true),
        ("ambiguous-source", "source-ambiguous", true),
        ("missing-output", "output-missing", false),
        ("ambiguous-output", "output-ambiguous", false),
        ("unsupported-output", "unsupported-observed-format", false),
    ] {
        let (config, mut inventory) = fixture();
        match kind {
            "missing-source" => {
                inventory["sources"].as_array_mut().unwrap().remove(0);
            }
            "ambiguous-source" => {
                let mut v = inventory["sources"][0].clone();
                v["observedId"] = json!("other:voice");
                inventory["sources"].as_array_mut().unwrap().push(v);
            }
            "missing-output" => {
                inventory["outputs"].as_array_mut().unwrap().remove(0);
            }
            "ambiguous-output" => {
                let mut v = inventory["outputs"][0].clone();
                v["observedId"] = json!("other:A");
                inventory["outputs"].as_array_mut().unwrap().push(v);
            }
            _ => inventory["outputs"][0]["format"]["channels"] = json!(6),
        }
        let report = run("inspect", &config, &inventory, 0);
        assert!(reason(path(&report, "o:a", "voice"), code));
        assert_eq!(
            path(&report, "o:a", "voice")["plannedEffectiveCoefficient"],
            0.0
        );
        assert_eq!(
            path(&report, "o:b", "voice")["plannedEffectiveCoefficient"],
            if source { 0.0 } else { 0.2 }
        );
    }
    let (config, mut inventory) = fixture();
    inventory["sources"][0]["format"]["channels"] = json!(6);
    let report = run("inspect", &config, &inventory, 3);
    rejected(&report);
    assert!(reason(
        path(&report, "o:a", "voice"),
        "unsupported-observed-format"
    ));
}

#[test]
fn privacy_consent_and_capacity_reject_even_muted_configured_routes() {
    let (mut config, inventory) = fixture();
    config["sends"]
        .as_array_mut()
        .unwrap()
        .push(json!({"source":"return","mix":"m:α","gain":0,"mute":true}));
    let report = run("explain", &config, &inventory, 3);
    rejected(&report);
    let p = path(&report, "o:b", "return");
    assert!(reason(p, "excluded-call-return"));
    assert_eq!(p["intendedPotentialSharing"], true);
    assert_eq!(p["configuredSend"]["mute"], true);
    let (mut config, inventory) = fixture();
    config["outputs"][0]["allowMicMonitor"] = json!(false);
    config["outputs"][0]["mute"] = json!(true);
    config["sends"][0]["gain"] = json!(0);
    let report = run("inspect", &config, &inventory, 3);
    rejected(&report);
    assert!(reason(
        path(&report, "o:a", "voice"),
        "mic-monitor-consent-required"
    ));
    let (config, mut inventory) = fixture();
    inventory["limits"] = json!({"maxOutputRoutes":1});
    let report = run("inspect", &config, &inventory, 3);
    rejected(&report);
    assert_eq!(
        report["capacity"][1],
        json!({"resource":"output-routes","requested":2,"maximum":1,"reason":"capacity-exceeded"})
    );
}

#[test]
fn invalid_privacy_observations_never_produce_a_plan() {
    for kind in ["recapture", "alias", "conflict"] {
        let (config, mut inventory) = fixture();
        let code = match kind {
            "recapture" => {
                inventory["sources"][0]["origin"] = json!("mixyard-producer");
                "recapture-forbidden"
            }
            "alias" => {
                inventory["sources"][2]["selector"]["applicationId"] = json!("app:music");
                "call-return-alias-forbidden"
            }
            _ => {
                let mut duplicate = inventory["sources"][0].clone();
                duplicate["format"]["channels"] = json!(2);
                inventory["sources"].as_array_mut().unwrap().push(duplicate);
                "conflicting-observation"
            }
        };
        let report = run("inspect", &config, &inventory, 3);
        rejected(&report);
        let source = if kind == "alias" { "player" } else { "voice" };
        assert!(reason(path(&report, "o:b", source), code));
    }
}

#[test]
fn unknown_formats_stay_unknown_even_with_native_discovery_evidence() {
    let (config, mut inventory) = fixture();
    inventory["evidence"] = json!("native-discovery");
    inventory["sources"][0]
        .as_object_mut()
        .unwrap()
        .remove("format");
    inventory["outputs"][0]
        .as_object_mut()
        .unwrap()
        .remove("format");
    let report = run("inspect", &config, &inventory, 0);
    assert!(reason(path(&report, "o:a", "voice"), "format-unknown"));
    assert_eq!(report["sources"][2]["permission"], "unknown");
    assert_eq!(output(&report, "o:a")["consumer"], "unknown");
    assert!(report["liveLevels"].is_null());
    assert!(report["plan"]["sends"][2]["rateConversionRequired"].is_null());
    assert_eq!(report["capacity"][0]["reason"], "capacity-unknown");
}

#[test]
fn many_arbitrary_mixes_fanout_and_reordered_input_are_deterministic() {
    let (mut config, mut inventory) = fixture();
    for n in 0..32 {
        let id = format!("音:{n}");
        config["mixes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":id,"name":"自由 / Same","channels":2}));
        config["sends"]
            .as_array_mut()
            .unwrap()
            .push(json!({"source":"player","mix":id,"gain":0.125}));
        for branch in 0..3 {
            let endpoint = format!("endpoint:{n}:{branch}");
            config["outputs"].as_array_mut().unwrap().push(json!({"id":endpoint,"mix":id,"target":{"kind":"virtual-input","endpointId":endpoint},"gain":0.5}));
            inventory["outputs"].as_array_mut().unwrap().push(json!({"observedId":endpoint,"target":{"kind":"virtual-input","endpointId":endpoint},"format":{"channels":2,"sampleRateHz":48000}}));
        }
    }
    let report = run("inspect", &config, &inventory, 0);
    assert_eq!(report["mixes"].as_array().unwrap().len(), 33);
    assert_eq!(report["outputs"].as_array().unwrap().len(), 98);
    assert_eq!(
        path(&report, "endpoint:31:2", "player")["plannedEffectiveCoefficient"],
        0.0625
    );
    assert!(reason(
        path(&report, "endpoint:31:2", "voice"),
        "send-not-configured"
    ));
    for field in ["mixes", "sources", "sends", "outputs"] {
        config[field].as_array_mut().unwrap().reverse();
    }
    for field in ["sources", "outputs"] {
        inventory[field].as_array_mut().unwrap().reverse();
    }
    assert_eq!(report, run("explain", &config, &inventory, 0));
}

#[test]
fn reordered_exclusions_preserve_the_plan_and_entire_inspection() {
    let (mut config, inventory) = fixture();
    config["mixes"][0]["excludeGroups"] = json!(["unused:b", "group:private", "unused:a"]);
    let report = run("inspect", &config, &inventory, 0);
    config["mixes"][0]["excludeGroups"]
        .as_array_mut()
        .unwrap()
        .reverse();
    let reordered = run("explain", &config, &inventory, 0);
    assert_eq!(report["plan"], reordered["plan"]);
    assert_eq!(report, reordered);
    let exclusions = json!(["group:private", "unused:a", "unused:b"]);
    assert_eq!(report["mixes"][0]["excludeGroups"], exclusions);
    for id in ["o:a", "o:b"] {
        assert_eq!(output(&report, id)["mix"]["excludeGroups"], exclusions);
        assert_eq!(path(&report, id, "return")["excluded"], true);
    }
}

fn check_conflicting_candidate_permutations(field: &str) {
    let (config, mut inventory) = fixture();
    let original = inventory[field][0].clone();
    let mut conflicting = original.clone();
    conflicting["format"]["sampleRateHz"] = json!(44100);
    let mut baseline = None;
    for records in [
        vec![original.clone(), conflicting.clone(), original.clone()],
        vec![original.clone(), original.clone(), conflicting.clone()],
        vec![conflicting.clone(), original.clone(), original.clone()],
    ] {
        inventory[field] = json!(records);
        let report = run("inspect", &config, &inventory, 3);
        rejected(&report);
        let subject = if field == "sources" {
            report["sources"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["configured"]["id"] == "voice")
                .unwrap()
        } else {
            output(&report, "o:a")
        };
        let candidates = subject["candidates"].as_array().unwrap();
        assert_eq!(candidates.len(), 2, "{field}: {candidates:?}");
        assert!(candidates.contains(&original));
        assert!(candidates.contains(&conflicting));
        assert!(reason(subject, "conflicting-observation"));
        if let Some(baseline) = &baseline {
            assert_eq!(&report, baseline);
        } else {
            baseline = Some(report);
        }
    }
}

#[test]
fn conflicting_source_candidates_deduplicate_across_permutations() {
    check_conflicting_candidate_permutations("sources");
}

#[test]
fn conflicting_output_candidates_deduplicate_across_permutations() {
    check_conflicting_candidate_permutations("outputs");
}

#[test]
fn orphan_references_remain_visible_and_never_become_an_applicable_plan() {
    let (mut config, inventory) = fixture();
    config["sends"]
        .as_array_mut()
        .unwrap()
        .push(json!({"source":"nonexistent","mix":"nonexistent-mix","gain":1}));
    config["outputs"][0]["mix"] = json!("nonexistent-mix");
    let report = run("inspect", &config, &inventory, 3);
    rejected(&report);
    assert!(report["sends"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["source"] == "nonexistent"));
    assert!(reason(output(&report, "o:a"), "unknown-mix-reference"));
    assert!(report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["code"] == "unknown-source-reference"));
}

#[test]
fn excluded_send_without_exports_still_has_typed_failure() {
    let (mut config, inventory) = fixture();
    config["outputs"] = json!([]);
    config["sends"]
        .as_array_mut()
        .unwrap()
        .push(json!({"source":"return","mix":"m:α","gain":0,"mute":true}));
    let report = run("inspect", &config, &inventory, 3);
    rejected(&report);
    assert!(report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["code"] == "excluded-call-return" && f["subject"] == "send:return->m:α"));
}

#[cfg(target_os = "linux")]
use mixyard::automation::{Completeness, Value};
use mixyard::automation::{Evidence, FactStatus};
use mixyard::context::{parse_fixture, Collector, Options, ProviderState};
use std::sync::atomic::AtomicBool;

fn fixture() -> serde_json::Value {
    serde_json::json!({"schemaVersion":1,"revision":5,"epoch":"fixture-epoch","facts":[{
        "key":{"name":"device.connected"},"status":"known",
        "value":{"type":"text-set","value":["fixture:endpoint"]},
        "provider":"imported","evidence":"platform-observed","observedAtMs":100,
        "validForMs":5000,"completeness":"complete","reason":null
    }]})
}

#[test]
fn imported_data_cannot_claim_native_or_declaration_evidence() {
    for evidence in ["platform-observed", "user-declared", "derived"] {
        let mut raw = fixture();
        raw["facts"][0]["evidence"] = evidence.into();
        let parsed = parse_fixture(&serde_json::to_vec(&raw).unwrap()).unwrap();
        assert_eq!(parsed.facts[0].evidence, Evidence::Fixture);
        assert_eq!(parsed.epoch, "fixture-epoch");
        assert_eq!(parsed.facts[0].observed_at_ms, Some(100));
    }
}

#[test]
fn imports_reject_duplicates_wrong_types_limits_and_false_empty_results() {
    let mut raw = fixture();
    let duplicate = raw["facts"][0].clone();
    raw["facts"].as_array_mut().unwrap().push(duplicate);
    assert!(parse_fixture(&serde_json::to_vec(&raw).unwrap()).is_err());
    let mut raw = fixture();
    raw["facts"][0]["value"]["value"] = serde_json::json!(vec!["x"; 257]);
    assert!(parse_fixture(&serde_json::to_vec(&raw).unwrap()).is_err());
    let mut raw = fixture();
    raw["facts"][0]["value"]["value"] = serde_json::json!(["x".repeat(1025)]);
    assert!(parse_fixture(&serde_json::to_vec(&raw).unwrap()).is_err());
    let mut raw = fixture();
    raw["facts"][0]["status"] = "unknown".into();
    assert!(parse_fixture(&serde_json::to_vec(&raw).unwrap()).is_err());
    assert!(parse_fixture(&vec![b' '; 1024 * 1024 + 1]).is_err());
    assert!(parse_fixture(b"{ malformed").is_err());
}

#[test]
fn configuration_is_explicit_exact_and_bounded() {
    let mut options = Options::default();
    options
        .applications
        .insert("player".into(), "org.example.Player".into());
    assert!(Collector::new(options).is_err());
    let options = Options {
        host_label: Some(" ".into()),
        ..Options::default()
    };
    assert!(Collector::new(options).is_err());
    let mut options = Options::default();
    options
        .applications
        .extend((0..33).map(|i| (format!("/fixture/{i}"), format!("app:{i}"))));
    assert!(Collector::new(options).is_err());
}

#[test]
fn pre_cancelled_collection_never_runs_a_provider() {
    let mut collector = Collector::new(Options::default()).unwrap();
    assert_eq!(
        collector.capture(&AtomicBool::new(true)).unwrap_err().0,
        "cancelled"
    );
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{atomic::Ordering, Arc};
    use std::time::Duration;

    struct Helper(PathBuf);
    impl Helper {
        fn new(code: &str) -> Self {
            let mut id = [0; 8];
            getrandom::fill(&mut id).unwrap();
            let root = std::env::temp_dir().join(format!(
                "mixyard-observer-test-{:x}",
                u64::from_le_bytes(id)
            ));
            fs::create_dir(&root).unwrap();
            fs::write(root.join("helper.py"), code).unwrap();
            Self(root)
        }
        fn options(&self) -> Options {
            Options {
                helper: self.0.join("helper.py"),
                host_label: Some("fixture-host".into()),
                ..Options::default()
            }
        }
    }
    impl Drop for Helper {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn helper_facts() -> serde_json::Value {
        let mut facts = fixture()["facts"].clone();
        facts[0]["provider"] = "linux.pipewire".into();
        for (name, provider) in [
            ("application.running", "linux.proc"),
            ("power.source", "linux.sysfs"),
            ("power.battery-percent", "linux.sysfs"),
            ("civil.timezone", "civil.zoneinfo"),
            ("civil.date", "civil.zoneinfo"),
            ("civil.weekday", "civil.zoneinfo"),
            ("civil.minute-of-day", "civil.zoneinfo"),
        ] {
            facts.as_array_mut().unwrap().push(serde_json::json!({
                "key":{"name":name}, "status":"unknown", "provider":provider,
                "evidence":"platform-observed", "observedAtMs":null, "validForMs":5000,
                "completeness":"partial", "reason":"fixture-unavailable"
            }));
        }
        facts
    }
    fn known_helper() -> Helper {
        let facts = helper_facts();
        // These are trusted *test helper* outputs; imports use the separate fixture path.
        Helper::new(&format!(
            "import sys\nsys.stdin.buffer.read()\nprint({:?})\n",
            serde_json::to_string(&facts).unwrap()
        ))
    }
    fn fact<'a>(
        capture: &'a mixyard::context::Capture,
        name: &str,
    ) -> &'a mixyard::automation::Fact {
        capture
            .snapshot
            .facts
            .iter()
            .find(|f| f.key.name == name)
            .unwrap()
    }
    #[test]
    fn collector_owns_epoch_revision_capture_time_and_explicit_host_declaration() {
        let helper = known_helper();
        let mut options = helper.options();
        options.applications.extend([
            ("/fixture/first".into(), "org.example.Z".into()),
            ("/fixture/second".into(), "org.example.A".into()),
            ("/fixture/alias".into(), "org.example.Z".into()),
        ]);
        let mut collector = Collector::new(options).unwrap();
        let first = collector.capture(&AtomicBool::new(false)).unwrap();
        std::thread::sleep(Duration::from_millis(2));
        let second = collector.capture(&AtomicBool::new(false)).unwrap();
        assert_eq!(first.snapshot.epoch, second.snapshot.epoch);
        assert_eq!(second.snapshot.revision, first.snapshot.revision + 1);
        assert!(second.now_ms >= first.now_ms);
        assert!(second.now_ms < 10_000); // session elapsed time, never Unix epoch milliseconds
        for f in second
            .snapshot
            .facts
            .iter()
            .filter(|f| f.status == FactStatus::Known)
        {
            assert!(f.observed_at_ms.unwrap() <= second.now_ms);
            assert_eq!(f.valid_for_ms, 5_000);
            assert_eq!(f.completeness, Completeness::Complete);
        }
        let host = fact(&second, "host.identity");
        assert_eq!(host.evidence, Evidence::UserDeclared);
        assert_eq!(host.value, Some(Value::Text("fixture-host".into())));
        let new_owner = Collector::new(helper.options()).unwrap();
        let mut new_owner = new_owner;
        let third = new_owner.capture(&AtomicBool::new(false)).unwrap();
        assert_ne!(first.snapshot.epoch, third.snapshot.epoch);
        assert_eq!(third.snapshot.revision, 1);
        assert_eq!(
            second.application_scope,
            vec!["org.example.A", "org.example.Z"]
        );
        assert!(second
            .providers
            .iter()
            .any(|p| p.provider == "linux.pipewire" && p.status == ProviderState::Complete));
        let serialized = serde_json::to_value(&second).unwrap();
        assert!(serialized.get("nowMs").is_some());
        assert_eq!(
            serialized["applicationScope"],
            serde_json::json!(["org.example.A", "org.example.Z"])
        );
        assert!(serialized["providers"][0].get("reason").is_some());
        assert_eq!(
            fact(&second, "network.ssid").status,
            FactStatus::Unsupported
        );
    }
    #[test]
    fn collector_facts_expire_on_the_collectors_monotonic_clock() {
        use mixyard::automation::*;
        let helper = known_helper();
        let capture = Collector::new(helper.options())
            .unwrap()
            .capture(&AtomicBool::new(false))
            .unwrap();
        let request = Request {
            graph: Graph {
                schema_version: 1,
                id: "expiry".into(),
                name: "Expiry".into(),
                revision: 0,
                root: "test".into(),
                nodes: vec![
                    Node::Decision {
                        id: "test".into(),
                        condition: Condition::Fact {
                            key: FactKey {
                                name: "device.connected".into(),
                                subject: None,
                            },
                            predicate: Predicate::Contains {
                                value: "fixture:endpoint".into(),
                            },
                        },
                        on_true: "present".into(),
                        on_false: "absent".into(),
                        on_unknown: "unknown".into(),
                    },
                    Node::Terminal {
                        id: "present".into(),
                        action: Action::NoChange {},
                    },
                    Node::Terminal {
                        id: "absent".into(),
                        action: Action::NoChange {},
                    },
                    Node::Terminal {
                        id: "unknown".into(),
                        action: Action::NoChange {},
                    },
                ],
            },
            catalog: standard_catalog(),
            snapshot: capture.snapshot,
            now_ms: capture.now_ms,
            context: Context {
                config_revision: 0,
                store_revision: 0,
                inventory_revision: 0,
                manual_revision: 0,
            },
            manual_override: false,
        };
        assert_eq!(
            evaluate(&request).unwrap().terminal.as_deref(),
            Some("present")
        );
        let stale = Request {
            now_ms: request.now_ms + 5001,
            ..request
        };
        assert_eq!(
            evaluate(&stale).unwrap().terminal.as_deref(),
            Some("unknown")
        );
    }
    #[test]
    fn malformed_oversized_and_nonzero_helpers_are_error_snapshots() {
        for code in [
            "print('not json')",
            "print('[]')",
            "print('x'*1100000)",
            "import sys; sys.exit(2)",
            "import sys; sys.stderr.write('x'*1100000)",
        ] {
            let helper = Helper::new(code);
            let mut collector = Collector::new(helper.options()).unwrap();
            let capture = collector.capture(&AtomicBool::new(false)).unwrap();
            assert_eq!(fact(&capture, "device.connected").status, FactStatus::Error);
            assert_eq!(fact(&capture, "device.connected").value, None);
            assert!(capture
                .providers
                .iter()
                .any(|p| p.provider == "local.helper" && p.status == ProviderState::Error));
        }
        let options = Options {
            helper: PathBuf::from("/nonexistent/mixyard-helper.py"),
            ..Options::default()
        };
        let capture = Collector::new(options)
            .unwrap()
            .capture(&AtomicBool::new(false))
            .unwrap();
        assert_eq!(fact(&capture, "device.connected").status, FactStatus::Error);
        assert_eq!(fact(&capture, "host.identity").status, FactStatus::Unknown);
    }
    #[test]
    fn helper_cardinality_and_declaration_claims_are_rejected_without_truncation() {
        for mutate in [0, 1, 2] {
            let mut facts = helper_facts();
            if mutate == 0 {
                facts[0]["value"]["value"] = serde_json::json!((0..257)
                    .map(|i| format!("endpoint:{i}"))
                    .collect::<Vec<_>>());
            } else if mutate == 1 {
                facts[0]["evidence"] = "user-declared".into();
            } else {
                facts[0]["key"]["name"] = "device.hardware-monitor-declared".into();
                facts[0]["key"]["subject"] = "fixture:endpoint".into();
                facts[0]["value"] = serde_json::json!({"type":"bool","value":true});
            }
            let helper = Helper::new(&format!(
                "print({:?})",
                serde_json::to_string(&facts).unwrap()
            ));
            let capture = Collector::new(helper.options())
                .unwrap()
                .capture(&AtomicBool::new(false))
                .unwrap();
            assert_eq!(fact(&capture, "device.connected").status, FactStatus::Error);
        }
    }
    fn assert_dead(pid: i32) {
        // Orphaned grandchildren may briefly remain zombies until PID 1 reaps them;
        // zombies are not executing and cannot keep a pipe/native observer alive.
        std::thread::sleep(Duration::from_millis(20));
        let state = fs::read_to_string(format!("/proc/{pid}/stat"));
        assert!(
            state.is_err()
                || state
                    .unwrap()
                    .split(')')
                    .nth(1)
                    .unwrap()
                    .trim_start()
                    .starts_with('Z')
        );
    }
    fn cleanup_helper() -> Helper {
        Helper::new("import subprocess,sys,time,os,pathlib\nroot=pathlib.Path(__file__).parent\nchild=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)'])\n(root/'pids').write_text(str(os.getpid())+' '+str(child.pid))\nsys.stdin.buffer.read()\nprint('[]',flush=True)\ntime.sleep(60)\n")
    }
    fn pids(root: &Path) -> Vec<i32> {
        fs::read_to_string(root.join("pids"))
            .unwrap()
            .split_whitespace()
            .map(|s| s.parse().unwrap())
            .collect()
    }
    #[test]
    fn timed_out_helper_and_descendants_are_stopped_and_reaped() {
        let helper = cleanup_helper();
        let capture = Collector::new(helper.options())
            .unwrap()
            .capture(&AtomicBool::new(false))
            .unwrap();
        assert_eq!(
            fact(&capture, "device.connected").reason.as_deref(),
            Some("child-timeout")
        );
        for pid in pids(&helper.0) {
            assert_dead(pid);
        }
    }
    #[test]
    fn excess_output_also_stops_the_owned_descendants() {
        let helper = cleanup_helper();
        let code = fs::read_to_string(helper.0.join("helper.py"))
            .unwrap()
            .replace("print('[]',flush=True)", "print('x'*1100000,flush=True)");
        fs::write(helper.0.join("helper.py"), code).unwrap();
        let capture = Collector::new(helper.options())
            .unwrap()
            .capture(&AtomicBool::new(false))
            .unwrap();
        assert_eq!(
            fact(&capture, "device.connected").reason.as_deref(),
            Some("child-output-limit")
        );
        for pid in pids(&helper.0) {
            assert_dead(pid);
        }
    }
    #[test]
    fn successful_capture_cleans_up_descendants_even_without_inherited_pipes() {
        let helper = cleanup_helper();
        let code = fs::read_to_string(helper.0.join("helper.py")).unwrap()
            .replace("time.sleep(60)'])", "time.sleep(60)'],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)")
            .replace("\ntime.sleep(60)\n", "\n");
        let code = code.replace(
            "print('[]',flush=True)",
            &format!(
                "print({:?},flush=True)",
                serde_json::to_string(&helper_facts()).unwrap()
            ),
        );
        fs::write(helper.0.join("helper.py"), code).unwrap();
        let capture = Collector::new(helper.options())
            .unwrap()
            .capture(&AtomicBool::new(false))
            .unwrap();
        assert!(!capture
            .providers
            .iter()
            .any(|p| p.provider == "local.helper"));
        for pid in pids(&helper.0) {
            assert_dead(pid);
        }
    }
    #[test]
    fn cancellation_cleans_up_child_group_and_does_not_publish_a_snapshot() {
        let helper = cleanup_helper();
        let cancel = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&cancel);
        let path = helper.0.join("pids");
        let trigger = std::thread::spawn(move || {
            for _ in 0..200 {
                if path.exists() {
                    signal.store(true, Ordering::Relaxed);
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            panic!("fixture helper did not start");
        });
        let mut collector = Collector::new(helper.options()).unwrap();
        assert_eq!(collector.capture(&cancel).unwrap_err().0, "cancelled");
        trigger.join().unwrap();
        for pid in pids(&helper.0) {
            assert_dead(pid);
        }
        // Cancelled capture did not consume a revision.
        fs::write(
            helper.0.join("helper.py"),
            format!(
                "import sys; sys.stdin.buffer.read(); print({:?})",
                serde_json::to_string(&helper_facts()).unwrap()
            ),
        )
        .unwrap();
        cancel.store(false, Ordering::Relaxed);
        assert_eq!(collector.capture(&cancel).unwrap().snapshot.revision, 1);
    }
}

#[cfg(not(target_os = "linux"))]
#[test]
fn unimplemented_platforms_do_not_emit_false_or_empty_native_observations() {
    let capture = Collector::new(Options::default())
        .unwrap()
        .capture(&AtomicBool::new(false))
        .unwrap();
    assert_eq!(
        capture
            .snapshot
            .facts
            .iter()
            .find(|f| f.key.name == "device.connected")
            .unwrap()
            .status,
        FactStatus::Unsupported
    );
    for fact in &capture.snapshot.facts {
        assert!(fact.value.is_none());
    }
    assert!(capture
        .providers
        .iter()
        .any(|p| p.status == ProviderState::Unsupported));
}

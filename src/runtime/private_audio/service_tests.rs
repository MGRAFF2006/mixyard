//! Actual product coordinator/Store/socket/process integration with an explicit
//! native-owner API double. These tests provide no native SDK or PCM evidence.
use super::*;
use std::{
    fs,
    os::{
        fd::AsRawFd,
        unix::{fs::DirBuilderExt, net::UnixStream},
    },
    process::{Command, Stdio},
};
struct Fixture {
    root: PathBuf,
    service: Option<Service>,
}
const COMPANION: &str = r#"
import os,json,select,sys
buffers={0:b'',3:b''}
while True:
 for fd in select.select([0,3],[],[],10)[0]:
  data=os.read(fd,65536)
  if not data: sys.exit(0)
  buffers[fd]+=data
  while b'\n' in buffers[fd]:
   line,buffers[fd]=buffers[fd].split(b'\n',1)
   r=json.loads(line); a=r['action']; kind=a['kind']
   if kind=='fence':
    receipt={'instanceNonce':r['instanceNonce'],'controlEpoch':a['newControlEpoch'],'operationId':r['operationId'],'outcome':'canceled-before-switch' if a['expected']['pendingInstall'] else 'no-pending','switched':None,'switchEvidence':None,'ownerQuiesced':True,'pendingPcmZero':True,'noLateInstall':True}
    print(json.dumps({'companionVersion':1,'instanceNonce':r['instanceNonce'],'controlEpoch':a['newControlEpoch'],'operationId':r['operationId'],'event':{'kind':'fenced','receipt':receipt}}),flush=True)
   elif kind=='shutdown': sys.exit(0)
   # Install intentionally stays pending. Only cancel fd3 can retire it.
"#;
fn python(source: &str) -> Command {
    let mut command = Command::new("/usr/bin/python3");
    command
        .env_clear()
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .arg("-c")
        .arg(source)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
}
impl Fixture {
    fn new(companion_source: &str) -> Self {
        let mut entropy = [0; 16];
        getrandom::fill(&mut entropy).unwrap();
        let root = PathBuf::from("/dev/shm").join(format!(
            "mixyard-private-service-{:x}",
            u128::from_ne_bytes(entropy)
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let store = Store::new(root.join("profiles")).unwrap();
        let outputs=(0..4).map(|i|serde_json::json!({"id":format!("output-{i}"),"mix":"bus","target":{"kind":"virtual-input","endpointId":format!("generated-{i}")},"gain":1})).collect::<Vec<_>>();
        let config:crate::Config=serde_json::from_value(serde_json::json!({"schemaVersion":2,"revision":0,"sources":[{"id":"generated","selector":{"kind":"app","applicationId":"fixture-only"}}],"mixes":[{"id":"bus","name":"Generated test bus","channels":2}],"sends":[{"source":"generated","mix":"bus","gain":0.25}],"outputs":outputs})).unwrap();
        store.edit(0, crate::profiles::Edit::Draft(config)).unwrap();
        let descriptor = Descriptor {
            schema_version: 1,
            scope: Scope::OwnedPrivateGeneratedV1,
            sdk_prefix: root.clone(),
            prefix_manifest: root.join("not-an-sdk-manifest"),
            prefix_manifest_sha256: "0".repeat(64),
            companion_executable: root.join("not-a-native-binary"),
            companion_sha256: "0".repeat(64),
            sdk_version: "1.4.8".into(),
            sdk_commit: "8b43ed5c910e892d53de5a96aa01e9280a5b6650".into(),
            generators: vec![Generator {
                source_id: "generated".into(),
                left: 0.25,
                right: -0.25,
                marker_period: 128,
                frame_limit: 0,
            }],
            limits: PrivateLimits {
                max_sources: 1,
                max_outputs: 4,
                max_resident_bytes: 16 * 1024 * 1024,
                max_frames: 128,
            },
        };
        let qualified = descriptor::Qualified::test_double(descriptor);
        let (cancel, child_cancel) = UnixStream::pair().unwrap();
        let mut daemon_command = python("import time;time.sleep(60)");
        process::parent_death(&mut daemon_command, None);
        let daemon = daemon_command.spawn().unwrap();
        let mut companion_command = python(companion_source);
        process::parent_death(&mut companion_command, Some(child_cancel.as_raw_fd()));
        let companion = companion_command.spawn().unwrap();
        drop(child_cancel);
        let processes =
            process::Processes::test_owned(qualified, daemon, companion, cancel, || Ok(()))
                .unwrap();
        let mut coordinator = Coordinator::new("a".repeat(64)).unwrap();
        coordinator
            .register_children(&processes.companion, &processes.daemon)
            .unwrap();
        let listener =
            crate::runtime::transport::Listener::bind(root.join("private.sock").to_str().unwrap())
                .unwrap();
        Self {
            root: root.clone(),
            service: Some(Service {
                listener,
                peers: Vec::new(),
                coordinator,
                processes,
                store,
                prepared: None,
                guard: None,
                install_started: None,
                fence_started: None,
                failed: false,
                retired: false,
                retirement: None,
                directory: root,
                shutting_down: false,
            }),
        }
    }
    fn service(&mut self) -> &mut Service {
        self.service.as_mut().unwrap()
    }
    fn client(&mut self, operation: Operation) -> UnixStream {
        let (server, mut client) = UnixStream::pair().unwrap();
        server.set_nonblocking(true).unwrap();
        client.set_nonblocking(true).unwrap();
        self.service().peers.push(Peer::new(server));
        let request = Request {
            private_audio_version: VERSION,
            request_id: "service-test".into(),
            operation,
        };
        let mut bytes = serde_json::to_vec(&request).unwrap();
        bytes.push(b'\n');
        client.write_all(&bytes).unwrap();
        client
    }
    fn reply(&mut self, client: &mut UnixStream) -> Reply {
        let mut bytes = Vec::new();
        let started = Instant::now();
        loop {
            self.service().tick().unwrap();
            let mut buffer = [0; 8192];
            match client.read(&mut buffer) {
                Ok(n) => {
                    bytes.extend_from_slice(&buffer[..n]);
                    if bytes.ends_with(b"\n") {
                        return serde_json::from_slice(&bytes).unwrap();
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => panic!("{e}"),
            };
            assert!(started.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn staged_install(&mut self) {
        let service = self.service();
        let nonce = service.coordinator.snapshot().stop_target.instance_nonce;
        let prepare = service
            .dispatch(Operation::Prepare {
                instance_nonce: nonce.clone(),
                expected_store_revision: 1,
                expected_config_revision: 1,
            })
            .unwrap()
            .unwrap();
        let snapshot = service.prepared.as_ref().unwrap();
        let mut outputs = snapshot
            .config
            .outputs
            .iter()
            .enumerate()
            .map(|(i, output)| PublishedOutput {
                output_id: output.id.clone(),
                mix_id: output.mix.clone(),
                label: "Generated test bus".into(),
                endpoint: Endpoint {
                    node_id: 10 + i as u32,
                    node_serial: 100 + i as u64,
                    port_id: 20 + i as u32,
                    port_serial: 200 + i as u64,
                },
                format: NativeFormat {
                    encoding: "f64-le".into(),
                    sample_rate_hz: 48000,
                    channels: vec!["FL".into(), "FR".into()],
                    frames: 128,
                },
                linked_consumer: None,
            })
            .collect::<Vec<_>>();
        service
            .event(reply(
                &prepare,
                CompanionEvent::Prepared {
                    prepared_token: "b".repeat(64),
                    inventory_revision: 1,
                    outputs: outputs.clone(),
                },
            ))
            .unwrap();
        for i in 0..outputs.len() {
            let consumer = Endpoint {
                node_id: 30 + i as u32,
                node_serial: 300 + i as u64,
                port_id: 40 + i as u32,
                port_serial: 400 + i as u64,
            };
            let link = service
                .dispatch(Operation::Link {
                    instance_nonce: nonce.clone(),
                    prepared_token: "b".repeat(64),
                    output_id: outputs[i].output_id.clone(),
                    consumer: consumer.clone(),
                })
                .unwrap()
                .unwrap();
            outputs[i].linked_consumer = Some(consumer);
            service
                .event(reply(
                    &link,
                    CompanionEvent::Linked {
                        inventory_revision: 2 + i as u64,
                        outputs: outputs.clone(),
                    },
                ))
                .unwrap();
        }
        let inventory = service.coordinator.snapshot().inventory_revision.unwrap();
        let stage = service
            .dispatch(Operation::Apply {
                instance_nonce: nonce,
                prepared_token: "b".repeat(64),
                expected_store_revision: 1,
                expected_config_revision: 1,
                expected_inventory_revision: inventory,
            })
            .unwrap()
            .unwrap();
        let generation = service
            .coordinator
            .snapshot()
            .stop_target
            .pending_install
            .unwrap()
            .reserved_generation;
        service
            .event(reply(
                &stage,
                CompanionEvent::Staged {
                    staged_token: "e".repeat(64),
                    generation,
                    config_revision: 1,
                    inventory_revision: inventory,
                    request_digest: "c".repeat(64),
                    endpoint_set_digest: "d".repeat(64),
                },
            ))
            .unwrap();
        assert!(service.guard.is_some());
        assert_eq!(service.coordinator.snapshot().phase, Phase::Installing);
    }
}
fn reply(request: &CompanionRequest, event: CompanionEvent) -> CompanionReply {
    CompanionReply {
        companion_version: VERSION,
        instance_nonce: request.instance_nonce.clone(),
        control_epoch: request.control_epoch,
        operation_id: request.operation_id.clone(),
        event,
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        drop(self.service.take());
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn stop_during_owned_install_bypasses_guard_and_waits_for_both_actual_child_reaps() {
    let mut fixture = Fixture::new(COMPANION);
    fixture.staged_install();
    let snapshot = fixture
        .service()
        .store
        .current_snapshot(STORE_LIMIT)
        .unwrap();
    assert_eq!(
        fixture
            .service()
            .store
            .edit(1, crate::profiles::Edit::Draft(snapshot.config.clone()))
            .unwrap_err()
            .code,
        "store-busy"
    );
    // The public request may time out and disconnect while coordinator/guard
    // ownership remains. Only the peer's transport lifetime ends.
    let (server, mut abandoned) = UnixStream::pair().unwrap();
    server.set_nonblocking(true).unwrap();
    abandoned.set_nonblocking(true).unwrap();
    let mut waiting = Peer::new(server);
    waiting.request_id = "timed-out-install".into();
    waiting.operation = fixture.service().coordinator.snapshot().pending_operation;
    waiting.started = Instant::now() - Duration::from_millis(INSTALL_MS);
    fixture.service().peers.push(waiting);
    let pending = fixture.reply(&mut abandoned);
    assert!(matches!(pending.result, Some(ResultValue::Pending { .. })));
    assert!(fixture.service().guard.is_some());
    drop(abandoned);
    fixture.service().tick().unwrap();
    assert!(fixture.service().guard.is_some());
    let mut client = fixture.client(Operation::Status {});
    let status = fixture.reply(&mut client);
    let Some(ResultValue::Status { status }) = status.result else {
        panic!("status response expected")
    };
    assert_eq!(status.outputs.len(), 4);
    assert_eq!(status.phase, Phase::Installing);
    assert!(fixture.service().guard.is_some());
    drop(client);
    let mut stale = status.stop_target.clone();
    stale.instance_nonce = "f".repeat(64);
    assert_eq!(
        fixture
            .service()
            .dispatch(Operation::Stop { expected: stale })
            .unwrap_err()
            .code,
        ErrorCode::StaleInstance
    );
    for wrong in 0..2 {
        let mut stale = status.stop_target.clone();
        if wrong == 0 {
            stale.control_epoch += 1;
        } else {
            stale.active_generation = Some(999);
        }
        assert!(fixture
            .service()
            .dispatch(Operation::Stop { expected: stale })
            .is_err());
        assert_eq!(
            fixture.service().coordinator.snapshot().stop_target,
            status.stop_target
        );
    }
    let mut client = fixture.client(Operation::Stop {
        expected: status.stop_target,
    });
    let reply = fixture.reply(&mut client);
    let Some(ResultValue::StopComplete {
        children_reaped,
        status,
        ..
    }) = reply.result
    else {
        panic!("normal double retirement must complete")
    };
    assert!(children_reaped);
    assert_eq!(status.phase, Phase::Stopped);
    assert!(fixture.service().processes.reaped().unwrap());
    assert!(fixture.service().guard.is_none());
    fixture
        .service()
        .store
        .edit(1, crate::profiles::Edit::Draft(snapshot.config))
        .unwrap();
}

#[test]
fn forced_reap_releases_writer_without_promoting_scrub_or_stop_complete() {
    let mut fixture = Fixture::new("import time;time.sleep(60)");
    fixture.staged_install();
    let expected = fixture.service().coordinator.snapshot().stop_target;
    let command = fixture
        .service()
        .dispatch(Operation::Stop { expected })
        .unwrap()
        .unwrap();
    fixture.service().processes.queue(&command).unwrap();
    fixture.service().fence_started = Some(Instant::now() - Duration::from_millis(FENCE_MS));
    fixture.service().tick().unwrap();
    let started = Instant::now();
    while !fixture.service().retired {
        fixture.service().tick().unwrap();
        assert!(started.elapsed() < Duration::from_secs(3));
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        fixture.service().coordinator.snapshot().phase,
        Phase::FailedStopped
    );
    assert!(fixture
        .service()
        .coordinator
        .snapshot()
        .last_error
        .is_some());
    assert!(fixture.service().guard.is_none());
    assert!(fixture.service().processes.reaped().unwrap());
}

#[test]
fn disconnected_install_deadline_owns_fence_and_retirement_without_a_peer() {
    let mut fixture = Fixture::new(COMPANION);
    fixture.staged_install();
    assert!(fixture.service().peers.is_empty());
    fixture.service().install_started = Some(Instant::now() - Duration::from_millis(INSTALL_MS));
    fixture.service().tick().unwrap();
    assert_eq!(
        fixture.service().coordinator.snapshot().phase,
        Phase::Stopping
    );
    assert!(fixture.service().guard.is_some()); // Queuing Fence is not proof.
    let started = Instant::now();
    while !fixture.service().retired {
        fixture.service().tick().unwrap();
        assert!(started.elapsed() < Duration::from_secs(3));
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        fixture.service().coordinator.snapshot().phase,
        Phase::Stopped
    );
    assert!(fixture.service().guard.is_none());
    assert!(fixture.service().processes.reaped().unwrap());
}

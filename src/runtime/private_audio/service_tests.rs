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
   if kind=='reserve':
    print(json.dumps({'companionVersion':1,'instanceNonce':r['instanceNonce'],'controlEpoch':r['controlEpoch'],'operationId':r['operationId'],'event':{'kind':'reserved','reservedGeneration':a['reservedGeneration'],'requestDigest':a['requestDigest']}}),flush=True)
   elif kind=='fence':
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
        .stderr(Stdio::piped());
    command
}
impl Fixture {
    fn new(companion_source: &str) -> Self {
        Self::with_daemon(companion_source, "import time;time.sleep(60)")
    }
    fn with_daemon(companion_source: &str, daemon_source: &str) -> Self {
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
        let mut daemon_command = python(daemon_source);
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
        coordinator
            .register_private_core(processes.connection())
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
                operation_started: None,
                startup_started: None,
                child_ready: true, // Explicit owner API-double startup.
                reservation_ready: false,
                deferred_native: None,
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
    fn stage_request(&mut self) -> (CompanionRequest, u64, u64) {
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
        accept_reservation(service, &prepare);
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
            accept_reservation(service, &link);
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
        (stage, generation, inventory)
    }
    fn staged_install(&mut self) {
        let (stage, generation, inventory) = self.stage_request();
        let service = self.service();
        accept_reservation(service, &stage);
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
    fn install_receipt(&mut self) -> (CompanionRequest, Installed) {
        let status = self.service().coordinator.snapshot();
        let pending = status.stop_target.pending_install.as_ref().unwrap();
        let request = CompanionRequest {
            companion_version: VERSION,
            instance_nonce: status.stop_target.instance_nonce.clone(),
            control_epoch: status.stop_target.control_epoch,
            operation_id: pending.operation_id.clone(),
            action: CompanionAction::Install {
                staged_token: "e".repeat(64),
            },
        };
        let receipt = Installed {
            instance_nonce: request.instance_nonce.clone(),
            control_epoch: request.control_epoch,
            operation_id: request.operation_id.clone(),
            generation: pending.reserved_generation,
            store_revision: 1,
            config_revision: 1,
            inventory_revision: status.inventory_revision.unwrap(),
            request_digest: "c".repeat(64),
            endpoint_set_digest: "d".repeat(64),
            clock_id: 11,
            native_switch_frame: 1024,
            engine_first_frame: 0,
            first_processed_frame: 1024,
            output_commits: status
                .outputs
                .iter()
                .map(|o| OutputCommit {
                    output_id: o.output_id.clone(),
                    native_frame: 1024,
                })
                .collect(),
            owner_install_terminal: true,
            no_late_install: true,
        };
        (request, receipt)
    }
}

// Native-owner API doubles: these prove control/Store/process behavior, not PCM.
#[test]
fn settled_source_loss_rejects_mutations_without_retirement_and_exact_stop_reaps() {
    let mut fixture = Fixture::new(COMPANION);
    fixture.staged_install();
    let (mut request, receipt) = fixture.install_receipt();
    fixture
        .service()
        .event(reply(&request, CompanionEvent::Installed { receipt }))
        .unwrap();
    request.operation_id = "native-fault".into();
    fixture
        .service()
        .event(reply(
            &request,
            CompanionEvent::Fault {
                error: failure(ErrorCode::SourceLost, "finite source EOF API double"),
            },
        ))
        .unwrap();
    let status = fixture.service().coordinator.snapshot();
    assert_eq!(status.phase, Phase::FailedHeld);
    assert!(status.installed.is_some());
    assert!(fixture.service().guard.is_none());
    let mut stale = status.stop_target.clone();
    stale.active_generation = Some(999);
    for operation in [
        Operation::Prepare {
            instance_nonce: request.instance_nonce.clone(),
            expected_store_revision: 0,
            expected_config_revision: 0,
        },
        Operation::Prepare {
            instance_nonce: request.instance_nonce.clone(),
            expected_store_revision: 1,
            expected_config_revision: 1,
        },
        Operation::Link {
            instance_nonce: request.instance_nonce.clone(),
            prepared_token: "b".repeat(64),
            output_id: "output-0".into(),
            consumer: status.outputs[0].linked_consumer.clone().unwrap(),
        },
        Operation::Apply {
            instance_nonce: request.instance_nonce.clone(),
            prepared_token: "b".repeat(64),
            expected_store_revision: 1,
            expected_config_revision: 1,
            expected_inventory_revision: status.inventory_revision.unwrap(),
        },
        Operation::Stop { expected: stale },
    ] {
        let mut client = fixture.client(operation);
        assert!(fixture.reply(&mut client).error.is_some());
        assert!(!fixture.service().failed);
        assert!(!fixture.service().retired);
        assert!(fixture.service().retirement.is_none());
        assert!(!fixture.service().processes.any_exited().unwrap());
        assert_eq!(
            fixture.service().coordinator.snapshot().stop_target,
            status.stop_target
        );
    }
    let mut stop = fixture.client(Operation::Stop {
        expected: status.stop_target,
    });
    let response = fixture.reply(&mut stop);
    let Some(ResultValue::StopComplete {
        children_reaped,
        status,
        ..
    }) = response.result
    else {
        panic!("source loss must retain the normal exact Stop path: {response:?}")
    };
    assert!(children_reaped);
    assert_eq!(status.phase, Phase::Stopped);
    assert!(status.private_core.is_none());
    let reap = status.last_child_reap.as_ref().unwrap();
    assert_eq!(reap.control_epoch, status.stop_target.control_epoch);
    assert_eq!(
        reap.companion.pid,
        fixture.service().processes.companion.id()
    );
    assert_eq!(
        (reap.companion.code, reap.companion.signal),
        (Some(0), None)
    );
    assert_eq!(reap.server.pid, fixture.service().processes.daemon.id());
    // This fixture's plain sleeping Python daemon is terminated by SIGTERM.
    // Preserve the actual signal instead of presenting classification as exit0.
    assert_eq!(
        (reap.server.code, reap.server.signal),
        (None, Some(libc::SIGTERM))
    );
    assert!(!reap.forced);
    assert!(fixture.service().processes.reaped().unwrap());
}

#[test]
fn pending_source_loss_cancels_now_retains_guard_and_ignores_late_stage() {
    let mut fixture = Fixture::new(COMPANION);
    fixture.staged_install();
    let (request, _) = fixture.install_receipt();
    let target = fixture.service().coordinator.snapshot().stop_target;
    let started = Instant::now() - Duration::from_millis(INSTALL_MS / 2);
    fixture.service().operation_started = Some(started);
    fixture
        .service()
        .event(reply(
            &request,
            CompanionEvent::Fault {
                error: failure(
                    ErrorCode::SourceLost,
                    "finite source EOF during install API double",
                ),
            },
        ))
        .unwrap();
    assert_eq!(
        fixture.service().coordinator.snapshot().phase,
        Phase::Stopping
    );
    assert_eq!(
        fixture
            .service()
            .coordinator
            .snapshot()
            .stop_target
            .control_epoch,
        target.control_epoch + 1
    );
    assert!(fixture.service().guard.is_some()); // Queuing Fence is not release proof.
    assert!(fixture.service().operation_started.is_none());
    assert!(fixture.service().deferred_native.is_none());
    let deadline = fixture.service().fence_started;
    fixture
        .service()
        .event(reply(
            &request,
            CompanionEvent::Staged {
                staged_token: "e".repeat(64),
                generation: 1,
                config_revision: 1,
                inventory_revision: 5,
                request_digest: "c".repeat(64),
                endpoint_set_digest: "d".repeat(64),
            },
        ))
        .unwrap();
    assert_eq!(fixture.service().fence_started, deadline);
    assert_eq!(
        fixture.service().coordinator.snapshot().phase,
        Phase::Stopping
    );
    assert!(fixture.service().guard.is_some());
    let snapshot = fixture
        .service()
        .store
        .current_snapshot(STORE_LIMIT)
        .unwrap();
    assert_eq!(
        fixture
            .service()
            .store
            .edit(1, crate::profiles::Edit::Draft(snapshot.config))
            .unwrap_err()
            .code,
        "store-busy"
    );
    let start = Instant::now();
    while !fixture.service().retired {
        fixture.service().tick().unwrap();
        assert!(start.elapsed() < Duration::from_secs(3));
        std::thread::yield_now();
    }
    assert_eq!(
        fixture.service().coordinator.snapshot().phase,
        Phase::Stopped
    );
    assert!(fixture.service().guard.is_none());
    assert!(fixture.service().processes.reaped().unwrap());
}

#[test]
fn ready_child_reservation_deadline_fences_without_a_peer_and_ignores_late_reserved() {
    use sha2::{Digest, Sha256};
    let mut fixture = Fixture::new("import time;time.sleep(60)");
    let (stage, reserved_generation, _) = fixture.stage_request();
    let request_digest = format!("{:x}", Sha256::digest(serde_json::to_vec(&stage).unwrap()));
    fixture.service().queue_command(&stage).unwrap();
    assert!(fixture.service().child_ready);
    assert!(fixture.service().startup_started.is_none());
    assert!(fixture.service().peers.is_empty());
    fixture.service().operation_started = Some(Instant::now() - Duration::from_millis(INSTALL_MS));
    fixture.service().tick().unwrap();
    assert_eq!(
        fixture.service().coordinator.snapshot().phase,
        Phase::Stopping
    );
    assert!(fixture.service().deferred_native.is_none());
    assert!(fixture.service().guard.is_none());
    fixture
        .service()
        .event(reply(
            &stage,
            CompanionEvent::Reserved {
                reserved_generation,
                request_digest,
            },
        ))
        .unwrap();
    assert_eq!(
        fixture.service().coordinator.snapshot().phase,
        Phase::Stopping
    );
    assert!(fixture.service().deferred_native.is_none());
    assert!(fixture.service().operation_started.is_none());
}

#[test]
fn late_staged_cannot_acquire_guard_or_restart_an_expired_apply() {
    let mut fixture = Fixture::new("import time;time.sleep(60)");
    let (stage, generation, inventory) = fixture.stage_request();
    accept_reservation(fixture.service(), &stage);
    fixture.service().operation_started = Some(Instant::now() - Duration::from_millis(INSTALL_MS));
    fixture
        .service()
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
    assert_eq!(
        fixture.service().coordinator.snapshot().phase,
        Phase::Stopping
    );
    assert!(fixture.service().guard.is_none());
    assert!(fixture.service().operation_started.is_none());
    assert!(fixture.service().fence_started.is_some());
}

#[test]
fn staged_preserves_original_apply_and_peer_deadlines() {
    let mut fixture = Fixture::new(COMPANION);
    let (stage, generation, inventory) = fixture.stage_request();
    accept_reservation(fixture.service(), &stage);
    let started = Instant::now() - Duration::from_millis(INSTALL_MS / 2);
    fixture.service().operation_started = Some(started);
    let (server, _client) = UnixStream::pair().unwrap();
    server.set_nonblocking(true).unwrap();
    let mut peer = Peer::new(server);
    peer.operation = Some(stage.operation_id.clone());
    peer.started = started;
    fixture.service().peers.push(peer);
    fixture
        .service()
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
    assert_eq!(
        fixture.service().coordinator.snapshot().phase,
        Phase::Installing
    );
    assert_eq!(fixture.service().operation_started, Some(started));
    assert_eq!(fixture.service().peers[0].started, started);
    assert!(fixture.service().guard.is_some());
}
fn accept_reservation(service: &mut Service, request: &CompanionRequest) {
    let reserve = service.coordinator.reserve_request(request).unwrap();
    let CompanionAction::Reserve {
        reserved_generation,
        request_digest,
        ..
    } = reserve.action
    else {
        panic!()
    };
    service
        .event(reply(
            request,
            CompanionEvent::Reserved {
                reserved_generation,
                request_digest,
            },
        ))
        .unwrap();
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
    assert!(status.private_core.is_none());
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
    fixture.service().operation_started = Some(Instant::now() - Duration::from_millis(INSTALL_MS));
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

const READY_GATE: &str = "NORMAL_DISPATCH_OBSERVED";
fn gated_companion() -> String {
    COMPANION.replace("   elif kind=='shutdown': sys.exit(0)",
        "   elif kind=='prepare':\n    print(json.dumps({'companionVersion':1,'instanceNonce':r['instanceNonce'],'controlEpoch':r['controlEpoch'],'operationId':r['operationId'],'event':{'kind':'fault','error':{'code':'owner-failed','message':'NORMAL_DISPATCH_OBSERVED'}}}),flush=True)\n   elif kind=='shutdown': sys.exit(0)")
}
fn waiting_prepare(fixture: &mut Fixture) -> UnixStream {
    fixture.service().child_ready = false;
    fixture.service().startup_started = Some(Instant::now());
    let nonce = fixture
        .service()
        .coordinator
        .snapshot()
        .stop_target
        .instance_nonce;
    let client = fixture.client(Operation::Prepare {
        instance_nonce: nonce,
        expected_store_revision: 1,
        expected_config_revision: 1,
    });
    let started = Instant::now();
    while !fixture.service().reservation_ready {
        fixture.service().tick().unwrap();
        assert!(!fixture.service().failed);
        assert!(started.elapsed() < Duration::from_secs(2));
        std::thread::yield_now();
    }
    assert!(fixture.service().deferred_native.is_some());
    client
}
#[test]
fn normal_prepare_waits_for_current_ready_after_exact_reservation() {
    let mut fixture = Fixture::new(&gated_companion());
    let _waiting = waiting_prepare(&mut fixture);
    assert_eq!(
        fixture.service().coordinator.snapshot().phase,
        Phase::Preparing
    );
    let status = fixture.service().coordinator.snapshot();
    fixture
        .service()
        .event(CompanionReply {
            companion_version: VERSION,
            instance_nonce: status.stop_target.instance_nonce,
            control_epoch: status.stop_target.control_epoch,
            operation_id: "startup".into(),
            event: CompanionEvent::Ready {
                inventory_revision: 1,
                endpoint_set_digest: "a".repeat(64),
            },
        })
        .unwrap();
    let started = Instant::now();
    loop {
        let events = fixture.service().processes.events().unwrap();
        if !events.is_empty() {
            assert_eq!(events.len(), 1);
            let CompanionEvent::Fault { error } = &events[0].event else {
                panic!("expected the double's normal-dispatch witness")
            };
            assert_eq!(error.code, ErrorCode::OwnerFailed);
            assert_eq!(error.message, READY_GATE);
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(2));
        std::thread::yield_now();
    }
}
#[test]
fn pending_prepare_stop_fences_without_sending_unready_normal_command() {
    let mut fixture = Fixture::new(&gated_companion());
    let _waiting = waiting_prepare(&mut fixture);
    let expected = fixture.service().coordinator.snapshot().stop_target;
    assert!(expected.pending_install.is_some());
    let mut stop = fixture.client(Operation::Stop { expected });
    let reply = fixture.reply(&mut stop);
    let Some(ResultValue::StopComplete {
        receipt, status, ..
    }) = reply.result
    else {
        panic!("{reply:?}")
    };
    assert_eq!(receipt.outcome, FenceOutcome::CanceledBeforeSwitch);
    assert_eq!(status.phase, Phase::Stopped);
    assert!(fixture.service().deferred_native.is_none());
}
#[test]
fn two_current_stop_peers_do_not_retransmit_or_reset_blocked_fence() {
    let mut fixture = Fixture::new("import time;time.sleep(60)");
    fixture.staged_install();
    assert!(fixture.service().processes.block_priority_for_test() > 0);
    let expected = fixture.service().coordinator.snapshot().stop_target;
    let _first = fixture.client(Operation::Stop { expected });
    fixture.service().tick().unwrap();
    let deadline = fixture.service().fence_started.unwrap();
    let pending = fixture.service().processes.priority_pending_bytes();
    assert!(pending > 0);
    let expected = fixture.service().coordinator.snapshot().stop_target;
    let mut second = fixture.client(Operation::Stop { expected });
    let reply = fixture.reply(&mut second);
    assert_eq!(reply.error.unwrap().code, ErrorCode::Busy);
    assert_eq!(fixture.service().fence_started, Some(deadline));
    assert_eq!(
        fixture.service().processes.priority_pending_bytes(),
        pending
    );
    assert!(!fixture.service().failed);
    assert_eq!(
        fixture.service().coordinator.snapshot().phase,
        Phase::Stopping
    );
}

#[test]
fn expired_startup_fences_pending_prepare_and_late_ready_cannot_dispatch() {
    let mut fixture = Fixture::new(&gated_companion());
    let _waiting = waiting_prepare(&mut fixture);
    let before = fixture.service().coordinator.snapshot().stop_target;
    fixture.service().startup_started = Some(Instant::now() - Duration::from_millis(INSTALL_MS));
    fixture.service().tick().unwrap();
    fixture
        .service()
        .event(CompanionReply {
            companion_version: VERSION,
            instance_nonce: before.instance_nonce,
            control_epoch: before.control_epoch,
            operation_id: "startup".into(),
            event: CompanionEvent::Ready {
                inventory_revision: 1,
                endpoint_set_digest: "a".repeat(64),
            },
        })
        .unwrap();
    assert!(!fixture.service().child_ready);
    assert!(fixture.service().deferred_native.is_none());
    assert_eq!(
        fixture.service().coordinator.snapshot().phase,
        Phase::Stopping
    );
    let started = Instant::now();
    while !fixture.service().retired {
        fixture.service().tick().unwrap();
        assert!(started.elapsed() < Duration::from_secs(3));
        std::thread::yield_now();
    }
    assert_eq!(
        fixture.service().coordinator.snapshot().phase,
        Phase::Stopped
    );
}

// Actual owned children and production Service/Coordinator retirement; no SDK.
#[test]
fn owner_failure_stop_cannot_rearm_retirement_or_reap_children_twice() {
    let mut fixture = Fixture::new("import time;time.sleep(60)");
    fixture.staged_install();
    fixture
        .service()
        .fail(failure(ErrorCode::OwnerFailed, "native fault double"));
    let target = fixture.service().coordinator.snapshot().stop_target;
    assert!(fixture.service().guard.is_some());

    // Two IPC peers arrive after failure, as cleanup Stop did in the native run.
    let mut first = fixture.client(Operation::Stop {
        expected: target.clone(),
    });
    let mut second = fixture.client(Operation::Stop {
        expected: target.clone(),
    });
    assert_eq!(
        fixture.reply(&mut first).error.unwrap().code,
        ErrorCode::Busy
    );
    assert_eq!(
        fixture.reply(&mut second).error.unwrap().code,
        ErrorCode::Busy
    );
    // Reaping may already have cleared pendingInstall, but Stop must not
    // increment the epoch or reserve another owner operation.
    assert_eq!(
        fixture
            .service()
            .coordinator
            .snapshot()
            .stop_target
            .control_epoch,
        target.control_epoch
    );
    assert!(fixture.service().fence_started.is_none());
    let start = Instant::now();
    while !fixture.service().retired {
        fixture.service().tick().unwrap();
        assert!(start.elapsed() < Duration::from_secs(3));
        std::thread::yield_now();
    }
    assert!(fixture.service().processes.reaped().unwrap());
    assert!(fixture.service().guard.is_none());
    assert_eq!(
        fixture.service().coordinator.snapshot().phase,
        Phase::FailedStopped
    );
    let error = fixture.service().coordinator.snapshot().last_error.unwrap();
    assert_eq!(error.code, ErrorCode::OwnerFailed);
    assert!(error
        .message
        .starts_with("native fault double; processes reaped;"));
    assert!(error.message.contains("StopComplete unverified"));
    assert!(error.message.contains("forced=true"));

    // A completed forced reap is still not native scrub or StopComplete.
    let target = fixture.service().coordinator.snapshot().stop_target;
    let mut late = fixture.client(Operation::Stop {
        expected: target.clone(),
    });
    assert_eq!(
        fixture.reply(&mut late).error.unwrap().code,
        ErrorCode::Busy
    );
    // A stale timer cannot create a second retirement after PID proof was consumed.
    fixture.service().fence_started = Some(Instant::now() - Duration::from_millis(FENCE_MS));
    fixture.service().tick().unwrap();
    assert!(fixture.service().retirement.is_none());
    assert_eq!(
        fixture.service().coordinator.snapshot().phase,
        Phase::FailedStopped
    );
    assert_eq!(fixture.service().coordinator.snapshot().stop_target, target);
    // A later fatal release/ownership error must return to FailedHeld even
    // after failed reaping; preserving its first cause cannot preserve phase.
    fixture.service().fail(failure(
        ErrorCode::StopIncomplete,
        "post-reap release uncertainty double",
    ));
    let status = fixture.service().coordinator.snapshot();
    assert_eq!(status.phase, Phase::FailedHeld);
    let error = status.last_error.unwrap();
    assert_eq!(error.code, ErrorCode::OwnerFailed);
    assert!(error
        .message
        .starts_with("native fault double; processes reaped;"));
    assert!(error
        .message
        .contains("post-reap release uncertainty double"));
    fixture.service().shutdown().unwrap();

    // Explicit signal/transport doubles. A final Stop failure must survive a
    // forced reap, while an unbound message must never acquire proof authority.
    for case in 0..5 {
        let source = r#"
import json,signal,sys,time
def emit(event,nonce='a'*64,epoch=1,op='startup'):
 print(json.dumps({'companionVersion':1,'instanceNonce':nonce,'controlEpoch':epoch,'operationId':op,'event':event}),flush=True)
def stopped(sig,frame):
 event={'kind':'fault','error':{'code':'stop-incomplete','message':'API_DOUBLE_FENCE_SAMPLE'+'Q'*768}}
 if CASE==2: event={'kind':'X'*70000}
 if CASE==3: event={'kind':'stopped'}
 if CASE==4: event={'kind':'fenced','receipt':{'instanceNonce':'a'*64,'controlEpoch':2,'operationId':'fence-2','outcome':'canceled-before-switch','switched':None,'switchEvidence':None,'ownerQuiesced':True,'pendingPcmZero':True,'noLateInstall':True}}
 emit(event,'NONCE'*64,2,'fence-2')
 sys.exit(0)
signal.signal(signal.SIGTERM,stopped)
emit({'kind':'stopped'}) # Test-only handshake: handler installed, not native Stop.
while True: time.sleep(60)
"#
        .replace("NONCE", if case == 1 { "f" } else { "a" })
        .replace("CASE", &case.to_string());
        let mut fixture = Fixture::new(&source);
        let start = Instant::now();
        loop {
            let events = fixture.service().processes.retirement_events().unwrap();
            if !events.is_empty() {
                assert_eq!(events.len(), 1);
                assert!(matches!(events[0].event, CompanionEvent::Stopped {}));
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(2));
            std::thread::yield_now();
        }
        fixture.staged_install();
        let target = fixture.service().coordinator.snapshot().stop_target;
        fixture.service().coordinator.begin_stop(&target).unwrap();
        fixture.service().fail(failure(
            ErrorCode::StopIncomplete,
            "native fence deadline double",
        ));
        assert!(fixture.service().guard.is_some());
        let start = Instant::now();
        while !fixture.service().retired {
            fixture.service().tick().unwrap();
            assert!(start.elapsed() < Duration::from_secs(3));
            std::thread::yield_now();
        }
        let status = fixture.service().coordinator.snapshot();
        let error = status.last_error.unwrap();
        assert_eq!(status.phase, Phase::FailedStopped);
        assert_eq!(error.code, ErrorCode::StopIncomplete);
        assert!(error.message.starts_with("native fence deadline double;"));
        assert_eq!(error.message.contains("API_DOUBLE_FENCE_SAMPLE"), case == 0);
        assert_eq!(error.message.contains(&"Q".repeat(768)), case == 0);
        assert_eq!(
            error.message.contains("rejected unbound/oversized"),
            case == 1
        );
        assert_eq!(
            error
                .message
                .contains("rejected oversized late native Stop parser diagnostic"),
            case == 2
        );
        assert!(error.message.len() < 2048);
        assert!(error.message.contains("forced=true"));
        if case == 3 || case == 4 {
            assert!(error
                .message
                .contains("unaccepted late native Stop observation only: bound=true"));
            assert!(error.message.contains(if case == 3 {
                "event=stopped"
            } else {
                "event=fenced"
            }));
        }
        if case == 4 {
            assert!(error
                .message
                .contains("ownerQuiesced=true pendingPcmZero=true noLateInstall=true"));
        }
        assert!(fixture.service().processes.reaped().unwrap());
        assert!(fixture.service().guard.is_none());
    }

    // Actual stderr pipes and graceful Stop handlers, still API doubles. A
    // warning invalidates StopComplete even when both children return zero.
    // The companion completes Shutdown despite failure retirement's TERM; the
    // daemon ignores repeat TERM after entering its first handler. Both use
    // immediate exit after the bounded write, avoiding Python shutdown races.
    for overflow in [false, true] {
        let companion_bytes = if overflow {
            "bytes(range(256))*17"
        } else {
            "b'warning\\n\\x00\\xfffirst'"
        };
        let daemon_bytes = if overflow {
            "bytes(range(255,-1,-1))*17"
        } else {
            "b'\\x80daemon\\x00'"
        };
        let companion = COMPANION
            .replace(
                "import os,json,select,sys",
                "import os,json,select,sys,signal\nsignal.signal(signal.SIGTERM,signal.SIG_IGN)",
            )
            .replace(
                "   elif kind=='shutdown': sys.exit(0)",
                &format!(
                    "   elif kind=='shutdown':\n    os.write(2,{companion_bytes})\n    os._exit(0)"
                ),
            );
        let daemon = format!("import os,signal,sys,time\ndef stopped(sig,frame):\n signal.signal(signal.SIGTERM,signal.SIG_IGN)\n os.write(2,{daemon_bytes})\n os._exit(0)\nsignal.signal(signal.SIGTERM,stopped)\nprint('READY',flush=True)\nwhile True: time.sleep(60)\n");
        let mut fixture = Fixture::with_daemon(&companion, &daemon);
        let stdout = fixture.service().processes.daemon.stdout.as_mut().unwrap();
        let flags = unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_GETFL) };
        assert!(flags >= 0);
        assert_eq!(
            unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) },
            0
        );
        let start = Instant::now();
        let mut ready = Vec::new();
        while !ready.ends_with(b"READY\n") {
            let mut bytes = [0; 16];
            match stdout.read(&mut bytes) {
                Ok(0) => panic!("daemon API-double handler exited before ready"),
                Ok(n) => ready.extend_from_slice(&bytes[..n]),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("daemon API-double readiness: {error}"),
            }
            assert!(ready.len() <= 16 && start.elapsed() < Duration::from_secs(2));
            std::thread::yield_now();
        }
        fixture.staged_install();
        let expected = fixture.service().coordinator.snapshot().stop_target;
        let mut stop = fixture.client(Operation::Stop { expected });
        let response = fixture.reply(&mut stop);
        assert!(!matches!(
            response.result,
            Some(ResultValue::StopComplete { .. })
        ));
        let start = Instant::now();
        while !fixture.service().retired {
            fixture.service().tick().unwrap();
            assert!(start.elapsed() < Duration::from_secs(3));
            std::thread::yield_now();
        }
        let status = fixture.service().coordinator.snapshot();
        assert_eq!(status.phase, Phase::FailedStopped);
        let message = &status.last_error.as_ref().unwrap().message;
        assert!(message.contains("validated Fence retained as observation only: event=fenced"));
        assert!(
            message.contains("companion=exit status: 0; server=exit status: 0; forced=true"),
            "stderr overflow={overflow}: {message}"
        );
        assert!(
            message.contains("owned stderr daemon:") && message.contains("owned stderr companion:")
        );
        if overflow {
            assert_eq!(message.matches("observedBytes=4352 retainedBytes=4096 eof=true overflow=true incomplete=true readFailed=false").count(), 2);
            let companion_hex = (0..=255u8)
                .cycle()
                .take(4096)
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            let daemon_hex = (0..=255u8)
                .rev()
                .cycle()
                .take(4096)
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            assert!(message.contains(&format!("rawHex={companion_hex}")));
            assert!(message.contains(&format!("rawHex={daemon_hex}")));
        } else {
            assert!(message.contains("observedBytes=15 retainedBytes=15 eof=true overflow=false incomplete=false readFailed=false rawHex=7761726e696e670a00ff6669727374"));
            assert!(message.contains("observedBytes=8 retainedBytes=8 eof=true overflow=false incomplete=false readFailed=false rawHex=806461656d6f6e00"));
        }
        assert!(message.len() <= 24 * 1024);
        let mut client = fixture.client(Operation::Status {});
        let status_reply = fixture.reply(&mut client);
        assert!(matches!(
            status_reply.result,
            Some(ResultValue::Status { .. })
        ));
        assert!(serde_json::to_vec(&status_reply).unwrap().len() <= RESULT_LIMIT);
        assert!(fixture.service().guard.is_none());
        assert!(fixture.service().processes.reaped().unwrap());
    }
}

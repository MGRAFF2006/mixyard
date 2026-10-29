use anyhow::Result;
use mixyard_control::{IdentityStore, Permission};
use mixyard_network::{
    Codec, Limits, Offer, PairedPeer, Permissions, Provenance, Session, Source, Spec, VERSION,
    transport::SecureEndpoint,
};
use mixyard_network_control_identity::endpoint_from_store;
use std::{
    collections::BTreeSet,
    fs,
    net::{IpAddr, Ipv4Addr},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

// All stores here are synthetic fixtures created by the actual production API.
// This does not provision a native app identity or approve a real remote peer.
struct Stores {
    root: PathBuf,
    a: Option<IdentityStore>,
    b: Option<IdentityStore>,
}

impl Stores {
    fn new() -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "mixyard-control-audio-fixture-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let builder = fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = builder;
            builder.mode(0o700);
            builder
        };
        builder.create(&root)?;
        let mut stores = Self {
            root,
            a: None,
            b: None,
        };
        let loopback = Some(IpAddr::V4(Ipv4Addr::LOCALHOST));
        stores.a = Some(IdentityStore::open(&stores.root.join("a"), loopback)?);
        stores.b = Some(IdentityStore::open(&stores.root.join("b"), loopback)?);
        Ok(stores)
    }

    fn a(&self) -> &IdentityStore {
        self.a.as_ref().unwrap()
    }
    fn b(&self) -> &IdentityStore {
        self.b.as_ref().unwrap()
    }

    fn reopen(&mut self) -> Result<()> {
        drop(self.a.take());
        drop(self.b.take());
        self.a = Some(IdentityStore::open(&self.root.join("a"), None)?);
        self.b = Some(IdentityStore::open(&self.root.join("b"), None)?);
        Ok(())
    }
}

impl Drop for Stores {
    fn drop(&mut self) {
        drop(self.a.take());
        drop(self.b.take());
        fs::remove_dir_all(&self.root).expect("remove synthetic identity fixtures");
    }
}

// Only the fixture simulates local approval of the public pins. The bridge does
// not derive a remote pin from a control invitation, role or HTTP response.
fn approved_fixture_pin(store: &IdentityStore) -> Result<PairedPeer> {
    let material = store.audio_identity_material(store.device_id())?;
    assert_eq!(material.device_id, store.device_id());
    assert_eq!(material.server_name, "mixyard-control");
    Ok(PairedPeer {
        peer: material.device_id,
        certificate: material.certificate,
        server_name: material.server_name,
    })
}

fn limits() -> Limits {
    Limits {
        max_streams: 8,
        max_connections: 2,
        packets_per_stream: 8,
        max_datagram: 1200,
        max_bytes_per_second: 32_000_000,
        control_timeout: Duration::from_secs(2),
    }
}

fn permissions(local: &str, remote: &str, send: &[Spec], receive: &[Spec]) -> Permissions {
    Permissions {
        local_peer: local.into(),
        remote_peer: remote.into(),
        send: send.iter().cloned().map(|s| (s.stream, s)).collect(),
        receive: receive.iter().cloned().map(|s| (s.stream, s)).collect(),
        excluded_receive_groups: BTreeSet::new(),
    }
}

fn spec(stream: u64, sender: &str, codec: Codec, rate: u32, channels: &[&str]) -> Spec {
    Spec {
        stream,
        name: format!("Fixture bus {stream}"),
        codec,
        sample_rate: rate,
        channels: channels.iter().map(|s| (*s).into()).collect(),
        frames_per_packet: 4,
        provenance: Provenance {
            sources: vec![Source {
                peer: sender.into(),
                source: format!("fixture-source-{stream}"),
                groups: vec!["fixture-selected-app".into()],
            }],
            visited_peers: vec![sender.into()],
        },
    }
}

fn offer(spec: Spec) -> Offer {
    Offer {
        version: VERSION,
        generation: 1,
        spec,
    }
}

fn open(store: &IdentityStore, peer: PairedPeer, grants: Permissions) -> Result<SecureEndpoint> {
    endpoint_from_store(
        "127.0.0.1:0".parse()?,
        store,
        store.device_id(),
        peer,
        grants,
        limits(),
    )
}

async fn sessions(a: &SecureEndpoint, b: &SecureEndpoint) -> Result<(Arc<Session>, Arc<Session>)> {
    Ok(tokio::try_join!(a.connect(b.local_addr()?), b.accept())?)
}

// Explicit sample words include signed extrema, low bits, negative zero,
// subnormals and NaN payloads. No protocol encoder generates the expected bytes.
fn pcm(s: &Spec, packet: usize) -> Vec<u8> {
    let words: &[u64] = match s.codec {
        Codec::RawS16Le => &[0x8000, 0x7fff, 1, 0xffff],
        Codec::RawS24Le => &[0x800000, 0x7fffff, 1, 0xffffff],
        Codec::RawS32Le => &[0x80000000, 0x7fffffff, 1, 0xffffffff],
        Codec::RawF32Le => &[0x80000000, 1, 0x7fc12345, 0x3f800001],
        Codec::RawF64Le => &[
            0x8000000000000000,
            1,
            0x7ff8123456789abc,
            0x3ff0000000000001,
        ],
        Codec::Opus => unreachable!(),
    };
    let width = s.codec.sample_bytes().unwrap();
    (0..usize::from(s.frames_per_packet) * s.channels.len())
        .flat_map(|i| {
            let word = words[(i + packet) % words.len()] ^ ((i as u64 / words.len() as u64) << 8);
            word.to_le_bytes()[..width].to_vec()
        })
        .collect()
}

async fn exact_raw(stores: &Stores) -> Result<()> {
    let a_id = stores.a().device_id();
    let b_id = stores.b().device_id();
    let send = [
        spec(
            101,
            a_id,
            Codec::RawS24Le,
            96_000,
            &["FR", "FL", "FC", "LFE", "SL", "SR"],
        ),
        spec(
            102,
            a_id,
            Codec::RawF64Le,
            44_117,
            &["height", "FR", "FL", "aux"],
        ),
        spec(103, a_id, Codec::RawS16Le, 48_000, &["fixture-mono"]),
    ];
    let receive = [
        spec(201, b_id, Codec::RawS32Le, 44_100, &["FR", "FL"]),
        spec(202, b_id, Codec::RawF32Le, 192_000, &["aux", "height"]),
    ];
    let ea = open(
        stores.a(),
        approved_fixture_pin(stores.b())?,
        permissions(a_id, b_id, &send, &receive),
    )?;
    let eb = open(
        stores.b(),
        approved_fixture_pin(stores.a())?,
        permissions(b_id, a_id, &receive, &send),
    )?;
    let (a, b) = sessions(&ea, &eb).await?;
    let run_a = tokio::spawn(a.clone().run());
    let run_b = tokio::spawn(b.clone().run());
    for (sender, receiver, streams) in [(&a, &b, send.as_slice()), (&b, &a, receive.as_slice())] {
        for s in streams {
            sender.negotiate(offer(s.clone())).await?;
            for packet in 0..3 {
                sender.enqueue(s.stream, pcm(s, packet)).await?;
                assert_eq!(sender.flush_round().await?, 1);
                tokio::time::timeout(Duration::from_secs(2), async {
                    while receiver.queued(s.stream) == 0 {
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                })
                .await?;
                assert_eq!(receiver.pull(s.stream)?, pcm(s, packet));
            }
            assert_eq!(receiver.counters(s.stream)?.accepted, 3);
            assert_eq!(receiver.counters(s.stream)?.missing_silence, 0);
        }
    }
    a.close();
    let _ = run_a.await?;
    let _ = run_b.await?;
    Ok(())
}

fn reject_wrong_context(stores: &Stores) -> Result<()> {
    let a = stores.a();
    let b = stores.b();
    let pin = approved_fixture_pin(b)?;
    let empty = permissions(a.device_id(), b.device_id(), &[], &[]);
    let wrong = endpoint_from_store(
        "127.0.0.1:0".parse()?,
        a,
        "wrong-local-device",
        pin.clone(),
        empty.clone(),
        limits(),
    );
    assert!(
        wrong
            .err()
            .unwrap()
            .to_string()
            .contains("identity export rejected")
    );
    let mut wrong = empty.clone();
    wrong.local_peer = "invented-local-peer".into();
    assert!(open(a, pin.clone(), wrong).is_err());
    let mut wrong = empty.clone();
    wrong.remote_peer = "invented-remote-peer".into();
    assert!(open(a, pin.clone(), wrong).is_err());
    let mut wrong_pin = pin.clone();
    wrong_pin.peer = "unapproved-peer-label".into();
    assert!(open(a, wrong_pin, empty).is_err());

    let mut s = spec(301, a.device_id(), Codec::RawS24Le, 96_000, &["FR", "FL"]);
    s.provenance.visited_peers = vec!["invented-source-peer".into()];
    assert!(open(a, pin, permissions(a.device_id(), b.device_id(), &[s], &[])).is_err());
    let s = spec(302, a.device_id(), Codec::RawF64Le, 44_117, &["FR", "FL"]);
    let mut excluded = permissions(b.device_id(), a.device_id(), &[], &[s]);
    excluded
        .excluded_receive_groups
        .insert("fixture-selected-app".into());
    assert!(open(b, approved_fixture_pin(a)?, excluded).is_err());
    Ok(())
}

async fn empty_grants_cannot_publish(stores: &Stores, local_grant: bool) -> Result<()> {
    let a_id = stores.a().device_id();
    let b_id = stores.b().device_id();
    let s = spec(401, a_id, Codec::RawS32Le, 48_000, &["FR", "FL"]);
    let send = if local_grant {
        std::slice::from_ref(&s)
    } else {
        &[]
    };
    let ea = open(
        stores.a(),
        approved_fixture_pin(stores.b())?,
        permissions(a_id, b_id, send, &[]),
    )?;
    let eb = open(
        stores.b(),
        approved_fixture_pin(stores.a())?,
        permissions(b_id, a_id, &[], &[]),
    )?;
    let (a, b) = sessions(&ea, &eb).await?;
    let run_a = tokio::spawn(a.clone().run());
    let run_b = tokio::spawn(b.clone().run());
    assert!(a.negotiate(offer(s.clone())).await.is_err());
    assert!(a.enqueue(s.stream, pcm(&s, 0)).await.is_err());
    assert_eq!(b.queued(s.stream), 0);
    assert!(b.pull(s.stream).is_err());
    a.close();
    let _ = run_a.await?;
    let _ = run_b.await?;
    Ok(())
}

async fn reject_wrong_tls_pin_and_name(stores: &Stores) -> Result<()> {
    let a = stores.a();
    let b = stores.b();
    for wrong_name in [false, true] {
        let mut pin = approved_fixture_pin(b)?;
        if wrong_name {
            pin.server_name = "unapproved.invalid".into();
        } else {
            pin.certificate = approved_fixture_pin(a)?.certificate;
        }
        let ea = open(a, pin, permissions(a.device_id(), b.device_id(), &[], &[]))?;
        let eb = open(
            b,
            approved_fixture_pin(a)?,
            permissions(b.device_id(), a.device_id(), &[], &[]),
        )?;
        let (client, _server) = tokio::join!(ea.connect(eb.local_addr()?), eb.accept());
        assert!(client.is_err(), "wrong remote pin/name must fail TLS");
    }
    // A correctly pinned server must also authenticate the client's exact leaf.
    let ea = open(
        a,
        approved_fixture_pin(b)?,
        permissions(a.device_id(), b.device_id(), &[], &[]),
    )?;
    let mut wrong_client_pin = approved_fixture_pin(a)?;
    wrong_client_pin.certificate = approved_fixture_pin(b)?.certificate;
    let eb = open(
        b,
        wrong_client_pin,
        permissions(b.device_id(), a.device_id(), &[], &[]),
    )?;
    let (client, server) = tokio::join!(ea.connect(eb.local_addr()?), eb.accept());
    assert!(
        server.is_err(),
        "server must reject the unapproved client leaf"
    );
    // TLS may finish locally before the peer's rejection reaches the client.
    if let Ok(client) = client {
        assert!(
            tokio::time::timeout(Duration::from_secs(2), client.run())
                .await?
                .is_err()
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn persisted_owner_identity_raw_mtls_and_independent_audio_consent() -> Result<()> {
    let mut stores = Stores::new()?;
    // Management roles exist only as management state; the bridge never consumes
    // these invitations or turns their roles into send/receive specs.
    for store in [&mut stores.a, &mut stores.b] {
        drop(store.as_mut().unwrap().invite(
            vec![Permission::View, Permission::Control, Permission::Settings],
            60,
        )?);
    }
    let pin_a = approved_fixture_pin(stores.a())?;
    let pin_b = approved_fixture_pin(stores.b())?;
    reject_wrong_context(&stores)?;
    empty_grants_cannot_publish(&stores, false).await?;
    empty_grants_cannot_publish(&stores, true).await?; // Local grant cannot replace remote consent.
    reject_wrong_tls_pin_and_name(&stores).await?;
    exact_raw(&stores).await?;
    stores.reopen()?;
    for (store, before) in [(stores.a(), pin_a), (stores.b(), pin_b)] {
        let after = approved_fixture_pin(store)?;
        assert_eq!(after.peer, before.peer);
        assert_eq!(after.certificate, before.certificate);
        assert_eq!(after.server_name, before.server_name);
    }
    exact_raw(&stores).await?;
    Ok(())
}

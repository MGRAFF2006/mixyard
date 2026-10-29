#![allow(dead_code)] // Shared ephemeral fixtures for integration tests and the runnable example.
use anyhow::Result;
use mixyard_network::*;
use rustls::pki_types::PrivatePkcs8KeyDer;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

pub fn limits() -> Limits {
    Limits {
        max_streams: 16,
        max_connections: 2,
        packets_per_stream: 8,
        max_datagram: 1200,
        max_bytes_per_second: 32_000_000,
        control_timeout: Duration::from_secs(2),
    }
}

pub fn spec(
    stream: u64,
    sender: &str,
    codec: Codec,
    rate: u32,
    channels: &[&str],
    frames: u16,
) -> Spec {
    Spec {
        stream,
        name: format!("Independent bus {stream}"),
        codec,
        sample_rate: rate,
        channels: channels.iter().map(|s| (*s).into()).collect(),
        frames_per_packet: frames,
        provenance: Provenance {
            sources: vec![Source {
                peer: sender.into(),
                source: format!("selected-app-{stream}"),
                groups: vec![],
            }],
            visited_peers: vec![sender.into()],
        },
    }
}

pub fn offer(spec: Spec, generation: u64) -> Offer {
    Offer {
        version: VERSION,
        generation,
        spec,
    }
}

pub fn permissions(local: &str, remote: &str, send: &[Spec], receive: &[Spec]) -> Permissions {
    Permissions {
        local_peer: local.into(),
        remote_peer: remote.into(),
        send: send
            .iter()
            .cloned()
            .map(|s| (s.stream, s))
            .collect::<BTreeMap<_, _>>(),
        receive: receive.iter().cloned().map(|s| (s.stream, s)).collect(),
        excluded_receive_groups: BTreeSet::new(),
    }
}

pub fn identity(peer: &str) -> Result<Identity> {
    let generated = rcgen::generate_simple_self_signed(vec!["mixyard-peer".into()])?;
    Ok(Identity {
        peer: peer.into(),
        certificate: generated.cert.der().clone(),
        private_key: PrivatePkcs8KeyDer::from(generated.signing_key.serialize_der()).into(),
    })
}

pub async fn pair(
    send: &[Spec],
    receive: &[Spec],
) -> Result<(
    Arc<Session>,
    Arc<Session>,
    transport::SecureEndpoint,
    transport::SecureEndpoint,
)> {
    pair_with_limits(send, receive, limits()).await
}

pub async fn pair_with_limits(
    send: &[Spec],
    receive: &[Spec],
    budget: Limits,
) -> Result<(
    Arc<Session>,
    Arc<Session>,
    transport::SecureEndpoint,
    transport::SecureEndpoint,
)> {
    let a = identity("a")?;
    let b = identity("b")?;
    let pin_a = PairedPeer {
        peer: "a".into(),
        certificate: a.certificate.clone(),
        server_name: "mixyard-peer".into(),
    };
    let pin_b = PairedPeer {
        peer: "b".into(),
        certificate: b.certificate.clone(),
        server_name: "mixyard-peer".into(),
    };
    let endpoint_a = endpoint(
        "127.0.0.1:0".parse()?,
        a,
        pin_b,
        permissions("a", "b", send, receive),
        budget.clone(),
    )?;
    let endpoint_b = endpoint(
        "127.0.0.1:0".parse()?,
        b,
        pin_a,
        permissions("b", "a", receive, send),
        budget,
    )?;
    let (a, b) = tokio::try_join!(
        endpoint_a.connect(endpoint_b.local_addr()?),
        endpoint_b.accept()
    )?;
    Ok((a, b, endpoint_a, endpoint_b))
}

pub async fn wait_queued(session: &Session, stream: u64, count: usize) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(2), async {
        while session.queued(stream) < count {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await?;
    Ok(())
}

/// Known signed extrema, low-bit precision, negative zero, subnormals and NaN payloads.
/// Expected bytes are generated from independent sample words, never the protocol codec.
pub fn sample_bytes(spec: &Spec, packet: usize) -> Vec<u8> {
    let words: &[u64] = match spec.codec {
        Codec::RawS16Le => &[0x8000, 0x7fff, 0x0001, 0xffff],
        Codec::RawS24Le => &[0x800000, 0x7fffff, 0x000001, 0xffffff],
        Codec::RawS32Le => &[0x80000000, 0x7fffffff, 0x00000001, 0xffffffff],
        Codec::RawF32Le => &[0x80000000, 0x00000001, 0x7fc12345, 0x3f800001],
        Codec::RawF64Le => &[
            0x8000000000000000,
            0x0000000000000001,
            0x7ff8123456789abc,
            0x3ff0000000000001,
        ],
        Codec::Opus => panic!("unavailable codec"),
    };
    let width = spec.codec.sample_bytes().unwrap();
    (0..usize::from(spec.frames_per_packet) * spec.channels.len())
        .flat_map(|i| {
            let word = words[(i + packet) % words.len()];
            // Preserve the first extrema, then distinguish channel/sample positions.
            let word = if i < words.len() {
                word
            } else {
                word ^ ((i as u64) << 8)
            };
            word.to_le_bytes()[..width].to_vec()
        })
        .collect()
}

pub async fn raw_proof() -> Result<()> {
    let streams = [
        spec(
            101,
            "a",
            Codec::RawS24Le,
            96_000,
            &["FL", "FR", "FC", "LFE", "SL", "SR"],
            8,
        ),
        spec(
            202,
            "a",
            Codec::RawF64Le,
            192_000,
            &["FL", "FR", "SL", "SR"],
            8,
        ),
    ];
    let returns = [spec(303, "b", Codec::RawS32Le, 44_100, &["FL", "FR"], 16)];
    let (a, b, _endpoint_a, _endpoint_b) = pair(&streams, &returns).await?;
    let run_a = tokio::spawn(a.clone().run());
    let run_b = tokio::spawn(b.clone().run());
    for stream in &streams {
        a.negotiate(offer(stream.clone(), 1)).await?;
    }
    b.negotiate(offer(returns[0].clone(), 1)).await?;
    for packet in 0..4 {
        for stream in &streams {
            let pcm = sample_bytes(stream, packet);
            let half = pcm.len() / 2;
            assert_eq!(a.enqueue_pcm(stream.stream, &pcm[..half]).await?, 0);
            assert_eq!(a.enqueue_pcm(stream.stream, &pcm[half..]).await?, 1);
        }
        b.enqueue(returns[0].stream, sample_bytes(&returns[0], packet))
            .await?;
        a.flush_round().await?;
        b.flush_round().await?;
    }
    for stream in &streams {
        wait_queued(&b, stream.stream, 4).await?;
        for packet in 0..4 {
            assert_eq!(b.pull(stream.stream)?, sample_bytes(stream, packet));
        }
        assert_eq!(b.counters(stream.stream)?.missing_silence, 0);
    }
    wait_queued(&a, returns[0].stream, 4).await?;
    for packet in 0..4 {
        assert_eq!(
            a.pull(returns[0].stream)?,
            sample_bytes(&returns[0], packet)
        );
    }
    // Restart requires a reliable generation change, never an old PCM replay.
    a.negotiate(offer(streams[0].clone(), 2)).await?;
    a.enqueue(101, sample_bytes(&streams[0], 3)).await?;
    a.flush_round().await?;
    wait_queued(&b, 101, 1).await?;
    assert_eq!(b.pull(101)?, sample_bytes(&streams[0], 3));
    a.close();
    let _ = run_a.await?;
    let _ = run_b.await?;
    assert!(b.pull(101)?.iter().all(|byte| *byte == 0));
    assert!(b.counters(101)?.disconnected_silence > 0);
    assert!(a.enqueue(101, sample_bytes(&streams[0], 0)).await.is_err());
    Ok(())
}

/// Encrypted network to caller-owned blocks; times below are deliberately injected
/// LOCAL scheduled block starts, not an audio device's observed clock.
pub async fn playout_proof() -> Result<()> {
    use mixyard_network::playout::*;
    use std::time::Instant;
    let streams = [
        spec(
            401,
            "a",
            Codec::RawS24Le,
            96_000,
            &["FL", "FR", "FC", "LFE", "SL", "SR"],
            5,
        ),
        spec(
            402,
            "a",
            Codec::RawF64Le,
            44_117,
            &["FR", "FL", "height", "aux"],
            5,
        ),
        spec(403, "a", Codec::RawF32Le, 192_000, &["selected-mono"], 5),
    ];
    let (a, b, _ea, _eb) = pair(&streams, &[]).await?;
    let run_a = tokio::spawn(a.clone().run());
    let run_b = tokio::spawn(b.clone().run());
    for s in &streams {
        a.negotiate(offer(s.clone(), 1)).await?;
    }
    let anchor = Instant::now();
    let mut handles: Vec<_> = streams
        .iter()
        .map(|s| {
            b.create_playout(
                s.stream,
                PlayoutConfig {
                    anchor,
                    target_buffer_frames: 7,
                },
            )
        })
        .collect::<Result<_>>()?;
    for seq in 0..4 {
        for s in &streams {
            let bytes = sample_bytes(s, seq);
            let frame_bytes = s.codec.sample_bytes()? * s.channels.len();
            assert_eq!(a.enqueue_pcm(s.stream, &bytes[..frame_bytes]).await?, 0);
            assert_eq!(a.enqueue_pcm(s.stream, &bytes[frame_bytes..]).await?, 1);
        }
        a.flush_round().await?;
    }
    for (s, p) in streams.iter().zip(&mut handles) {
        wait_queued(&b, s.stream, 4).await?;
        b.service_playout(p)?;
        let frame_bytes = s.codec.sample_bytes()? * s.channels.len();
        let mut before = vec![0xff; frame_bytes * 3];
        assert_eq!(p.render_into(anchor, &mut before)?.prebuffer_frames, 3);
        assert!(before.iter().all(|&b| b == 0));
        let expected: Vec<_> = (0..4).flat_map(|seq| sample_bytes(s, seq)).collect();
        let mut cursor = 0;
        for frames in [2, 7, 1, 10] {
            let elapsed_frames = 7 + cursor as u64;
            let nanos = (elapsed_frames * 1_000_000_000).div_ceil(u64::from(s.sample_rate));
            let local_block_start = anchor
                .checked_add(Duration::from_nanos(nanos))
                .expect("short fixture time");
            let mut block = vec![0xff; frames * frame_bytes];
            let report = p.render_into(local_block_start, &mut block)?;
            assert_eq!(report.audio_frames, frames as u64);
            assert_eq!(
                block,
                expected[cursor * frame_bytes..(cursor + frames) * frame_bytes]
            );
            cursor += frames;
        }
        println!("{}", serde_json::to_string(&p.snapshot())?);
    }
    // Retain old-generation bytes, then prove reliable restart invalidates them
    // without a service tick. Sibling generation remains active.
    b.service_playout(&mut handles[0])?; // Advance ingress before sending/waiting.
    a.enqueue(streams[0].stream, sample_bytes(&streams[0], 4))
        .await?;
    a.flush_round().await?;
    wait_queued(&b, streams[0].stream, 1).await?;
    b.service_playout(&mut handles[0])?;
    assert_eq!(handles[0].snapshot().buffered_frames, 5);
    let receipt = handles[0].revocation();
    a.negotiate(offer(streams[0].clone(), 2)).await?;
    assert_eq!(receipt.status(), RevocationStatus::Complete);
    assert!(handles[1].snapshot().active);
    let mut out = vec![0xff; streams[0].payload_bytes()?];
    assert_eq!(
        handles[0]
            .render_into(
                anchor
                    .checked_add(Duration::from_millis(10))
                    .expect("short fixture time"),
                &mut out
            )?
            .inactive_frames,
        5
    );
    assert!(out.iter().all(|&b| b == 0));
    let revoked = b.revoke_receive(streams[1].stream)?;
    assert_eq!(revoked.status(), RevocationStatus::Complete);
    b.close(); // Local close invalidates remaining consumer without another handoff.
    out.resize(streams[2].payload_bytes()?, 0xff);
    assert_eq!(
        handles[2]
            .render_into(
                anchor
                    .checked_add(Duration::from_millis(10))
                    .expect("short fixture time"),
                &mut out
            )?
            .inactive_frames,
        5
    );
    assert!(out.iter().all(|&b| b == 0));
    a.close();
    let _ = run_a.await?;
    let _ = run_b.await?;
    Ok(())
}

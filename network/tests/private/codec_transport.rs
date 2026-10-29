use super::*;
use crate::transport::tests::support as common;
#[path = "../codec/support.rs"]
mod support;
use support::*;

async fn poll_once<F: Future>(mut future: Pin<&mut F>) -> Poll<F::Output> {
    std::future::poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await
}
async fn inject(
    session: &Arc<FragmentSession>,
    stream: u64,
    generation: u64,
    sequence: u64,
    packet: &[u8],
    reverse: bool,
) -> Result<()> {
    let d = {
        let tx = session.outgoing.lock().unwrap();
        let state = &tx.streams[&stream];
        geometry(
            &Offer {
                generation,
                ..state.offer.clone()
            },
            state.codec.as_ref().unwrap().encoder.negotiated(),
        )?
    };
    let sender = Fragmenter::variable(session.config.clone(), d)?;
    let mut fragments: Vec<_> = sender.fragments(sequence, packet)?.collect();
    if reverse {
        fragments.reverse();
    }
    for fragment in fragments {
        session
            .base
            .connection
            .send_datagram_wait(fragment.into())
            .await?;
    }
    Ok(())
}
fn encode(session: &FragmentSession, stream: u64, raw: &[u8]) -> Vec<u8> {
    session
        .outgoing
        .lock()
        .unwrap()
        .streams
        .get_mut(&stream)
        .unwrap()
        .codec
        .as_mut()
        .unwrap()
        .encoder
        .encode(raw)
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn regression_prefill_refills_consumed_slots_across_partial_and_combined_renders()
-> Result<()> {
    let mut encodings = vec![Encoding::Raw];
    if cfg!(feature = "system-zstd") {
        encodings.push(Encoding::Zstd { level: 1 });
    }
    if cfg!(feature = "system-opus") {
        encodings.push(support::opus());
    }
    for encoding in encodings {
        for prefix in [0usize, 60] {
            let s = spec(1, "a", Codec::RawS16Le, 48_000, &["mono"], 120);
            let req = request(1, 1, encoding);
            let (a, b, _ea, _eb) = pair(
                std::slice::from_ref(&s),
                &[],
                std::slice::from_ref(&req),
                &[],
            )
            .await?;
            let arun = tokio::spawn(a.clone().run());
            let brun = tokio::spawn(b.clone().run());
            let anchor = Instant::now();
            a.negotiate_codec(req, anchor).await?;
            let mut p = b.create_codec_playout(
                1,
                1,
                PlayoutConfig {
                    anchor,
                    target_buffer_frames: 0,
                },
            )?;
            let raw0 = vec![0x11; 240];
            let raw1 = vec![0x22; 240];
            let packet0 = encode(&a, 1, &raw0);
            let packet1 = encode(&a, 1, &raw1);
            inject(&a, 1, 1, 0, &packet0, false).await?;
            service(&b, &mut p).await?;
            if prefix != 0 {
                let mut first = vec![0; prefix * 2];
                assert_eq!(
                    p.render_into(anchor, &mut first)?.audio_frames,
                    prefix as u64
                );
            }
            inject(&a, 1, 1, 1, &packet1, false).await?;
            tokio::time::timeout(Duration::from_secs(1), async {
                while b.receiver.lock().unwrap().reassembler.buffered_bytes() == 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await?;
            let inserted = b.service_codec_playout(&mut p)?;
            let mut output = vec![0xff; (240 - prefix) * 2];
            let now = anchor + Duration::from_nanos(prefix as u64 * 1_000_000_000 / 48_000);
            let report = p.render_into(now, &mut output)?;
            a.close();
            let _ = arun.await?;
            let _ = brun.await?;
            assert_eq!(
                (inserted, report.audio_frames, report.missing_frames),
                (1, (240 - prefix) as u64, 0),
                "{encoding:?}, partial prefix={prefix}"
            );
            if !matches!(encoding, Encoding::Opus { .. }) {
                assert_eq!(&output[..(120 - prefix) * 2], &raw0[prefix * 2..]);
                assert_eq!(&output[(120 - prefix) * 2..], &raw1);
            }
        }
    }
    Ok(())
}

async fn lossless_recovery(cases: &[bool]) -> Result<()> {
    let mut encodings = vec![Encoding::Raw];
    if cfg!(feature = "system-zstd") {
        encodings.push(Encoding::Zstd { level: 1 });
    }
    for encoding in encodings {
        for &partial in cases {
            let s = spec(1, "a", Codec::RawS24Le, 48_000, &["mono"], 960);
            let sibling = spec(2, "a", Codec::RawS16Le, 48_000, &["mono"], 120);
            let req = request(1, 1, encoding);
            let other = request(2, 1, Encoding::Raw);
            let mut c = config();
            c.timeout = Duration::from_millis(40);
            let requests = [req.clone(), other.clone()];
            let (a, b, _ea, _eb) = pair_configured(
                &[s, sibling],
                &[],
                &requests,
                &[],
                &requests,
                &[],
                c,
                limits(),
            )
            .await?;
            let arun = tokio::spawn(a.clone().run());
            let brun = tokio::spawn(b.clone().run());
            let anchor = Instant::now();
            a.negotiate_codec(req, anchor).await?;
            a.negotiate_codec(other, anchor).await?;
            let mut p = b.create_codec_playout(
                1,
                1,
                PlayoutConfig {
                    anchor,
                    target_buffer_frames: 0,
                },
            )?;
            let mut sibling = b.create_codec_playout(
                2,
                1,
                PlayoutConfig {
                    anchor,
                    target_buffer_frames: 0,
                },
            )?;
            let mut word = 0x1a2b3c4du32;
            let raw: Vec<_> = (0..2880)
                .map(|_| {
                    word ^= word << 13;
                    word ^= word >> 17;
                    word ^= word << 5;
                    word as u8
                })
                .collect();
            let lost = encode(&a, 1, &raw);
            let next = encode(&a, 1, &raw);
            let recovery = encode(&a, 1, &raw);
            if partial {
                let d = {
                    let tx = a.outgoing.lock().unwrap();
                    let state = &tx.streams[&1];
                    geometry(
                        &state.offer,
                        state.codec.as_ref().unwrap().encoder.negotiated(),
                    )?
                };
                let f = Fragmenter::variable(a.config.clone(), d)?;
                let fragments: Vec<_> = f.fragments(0, &lost)?.collect();
                assert!(
                    fragments.len() > 1,
                    "incompressible multi-fragment loss fixture"
                );
                a.base
                    .connection
                    .send_datagram_wait(fragments[0].clone().into())
                    .await?;
                tokio::time::timeout(Duration::from_secs(1), async {
                    while b.receiver.lock().unwrap().reassembler.buffered_bytes() == 0 {
                        tokio::task::yield_now().await;
                    }
                })
                .await?;
                tokio::time::sleep(Duration::from_millis(80)).await;
            }
            let before = b.service_codec_playout(&mut p);
            if let Err(error) = before {
                a.close();
                let _ = arun.await?;
                let _ = brun.await?;
                anyhow::bail!(
                    "{encoding:?} partial={partial}: ordinary loss retired generation: {error}"
                );
            }
            assert!(p.snapshot().active);
            inject(&a, 1, 1, 1, &next, false).await?;
            service(&b, &mut p).await?;
            let mut output = vec![0xff; 5760];
            let report = p.render_into(anchor, &mut output)?;
            assert_eq!((report.audio_frames, report.missing_frames), (960, 960));
            assert_eq!(p.snapshot().counters.missing_frames, 960);
            assert!(output[..2880].iter().all(|v| *v == 0));
            assert_eq!(&output[2880..], &raw);
            inject(&a, 1, 1, 2, &recovery, false).await?;
            service(&b, &mut p).await?;
            let mut resumed = vec![0; 2880];
            let report = p.render_into(anchor + Duration::from_millis(40), &mut resumed)?;
            assert_eq!((report.audio_frames, report.missing_frames), (960, 0));
            assert_eq!(resumed, raw);
            // A later local schedule jump must count the skipped source slot,
            // never move the available packet backwards to hide the loss.
            let _skipped = encode(&a, 1, &raw);
            let fifth = encode(&a, 1, &raw);
            inject(&a, 1, 1, 4, &fifth, false).await?;
            service(&b, &mut p).await?;
            let report = p.render_into(anchor + Duration::from_millis(80), &mut resumed)?;
            assert_eq!(
                (
                    report.first_sample_frame,
                    report.audio_frames,
                    report.skipped_frames
                ),
                (Some(3840), 960, 960)
            );
            assert_eq!(p.snapshot().counters.skipped_frames, 960);
            assert_eq!(resumed, raw);
            let other = vec![0x25; 240];
            let packet = encode(&a, 2, &other);
            inject(&a, 2, 1, 0, &packet, false).await?;
            service(&b, &mut sibling).await?;
            assert_eq!(render(&mut sibling, anchor, 0), other);
            assert!(p.snapshot().active && sibling.snapshot().active);
            a.close();
            let _ = arun.await?;
            let _ = brun.await?;
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn regression_lossless_holes_conceal_then_resume_without_retiring_siblings() -> Result<()> {
    lossless_recovery(&[false]).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn regression_lossless_fragment_expiry_conceals_then_resumes_without_retiring_siblings()
-> Result<()> {
    lossless_recovery(&[true]).await
}

#[cfg(feature = "system-opus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn regression_opus_waits_for_predictor_order_and_retires_only_after_source_loss() -> Result<()>
{
    let s = spec(1, "a", Codec::RawS16Le, 48_000, &["mono"], 120);
    let req = request(1, 1, support::opus());
    let (a, b, _ea, _eb) = pair(
        std::slice::from_ref(&s),
        &[],
        std::slice::from_ref(&req),
        &[],
    )
    .await?;
    let arun = tokio::spawn(a.clone().run());
    let brun = tokio::spawn(b.clone().run());
    let anchor = Instant::now();
    a.negotiate_codec(req, anchor).await?;
    let mut p = b.create_codec_playout(
        1,
        1,
        PlayoutConfig {
            anchor,
            target_buffer_frames: 0,
        },
    )?;
    let first = encode(&a, 1, &vec![0x11; 240]);
    let second = encode(&a, 1, &vec![0x22; 240]);
    inject(&a, 1, 1, 1, &second, false).await?;
    tokio::time::timeout(Duration::from_secs(1), async {
        while b.receiver.lock().unwrap().reassembler.buffered_bytes() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    assert_eq!(
        b.service_codec_playout(&mut p)?,
        0,
        "no predictor skip to complete block one"
    );
    assert!(p.snapshot().active);
    inject(&a, 1, 1, 0, &first, false).await?;
    tokio::time::timeout(Duration::from_secs(1), async {
        while p.snapshot().buffered_frames < 240 {
            b.service_codec_playout(&mut p)?;
            tokio::task::yield_now().await;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    let mut output = vec![0; 480];
    assert_eq!(p.render_into(anchor, &mut output)?.audio_frames, 240);
    let _lost = encode(&a, 1, &vec![0x33; 240]);
    let fourth = encode(&a, 1, &vec![0x44; 240]);
    inject(&a, 1, 1, 3, &fourth, false).await?;
    let mut missing = vec![0xff; 240];
    assert_eq!(
        p.render_into(anchor + Duration::from_millis(5), &mut missing)?
            .missing_frames,
        120
    );
    assert!(missing.iter().all(|v| *v == 0));
    assert!(
        b.service_codec_playout(&mut p).is_err(),
        "source loss requires explicit restart, no hidden PLC"
    );
    assert!(!p.snapshot().active);
    a.close();
    let _ = arun.await?;
    let _ = brun.await?;
    Ok(())
}

#[tokio::test]
async fn denied_exact_codec_generation_and_raw_grant_never_escalate_or_close_siblings() -> Result<()>
{
    let one = spec(1, "a", Codec::RawS16Le, 100, &["mono"], 1000);
    let two = spec(2, "a", Codec::RawS32Le, 100, &["aux"], 1000);
    let first = request(1, 1, Encoding::Raw);
    let restart = request(1, 2, Encoding::Raw);
    let sibling = request(2, 1, Encoding::Raw);
    let streams = [one.clone(), two.clone()];
    let (a, b, _ea, _eb) = pair_configured(
        &streams,
        &[],
        &[first.clone(), restart.clone(), sibling.clone()],
        &[],
        &[first.clone(), sibling.clone()],
        &[],
        config(),
        limits(),
    )
    .await?;
    let arun = tokio::spawn(a.clone().run());
    let brun = tokio::spawn(b.clone().run());
    let anchor = Instant::now() + Duration::from_millis(100);
    a.negotiate_codec(first, anchor).await?;
    a.negotiate_codec(sibling, anchor).await?;
    assert!(a.negotiate_codec(restart, anchor).await.is_err());
    assert!(
        a.base.connection.close_reason().is_none(),
        "explicit rejection preserves old generation/siblings"
    );
    assert!(
        a.negotiate(common::offer(one.clone(), 3)).await.is_err(),
        "ordinary raw exchange cannot bypass exact codec endpoint authority"
    );
    let foreign = request(1, 3, Encoding::Zstd { level: 1 });
    assert!(!a.codec_capability(foreign.clone(), true).available);
    assert!(a.negotiate_codec(foreign, anchor).await.is_err());
    let mut p1 = b.create_codec_playout(
        1,
        1,
        PlayoutConfig {
            anchor,
            target_buffer_frames: 0,
        },
    )?;
    let mut p2 = b.create_codec_playout(
        2,
        1,
        PlayoutConfig {
            anchor,
            target_buffer_frames: 0,
        },
    )?;
    let raw1 = vec![0x11; one.payload_bytes()?];
    let raw2 = vec![0x52; two.payload_bytes()?];
    tokio::try_join!(a.send_codec(1, 1, &raw1), a.send_codec(2, 1, &raw2))?;
    service(&b, &mut p1).await?;
    service(&b, &mut p2).await?;
    assert_eq!(render(&mut p1, anchor, 0), raw1);
    assert_eq!(render(&mut p2, anchor, 0), raw2);
    a.close();
    let _ = arun.await?;
    let _ = brun.await?;
    Ok(())
}

#[tokio::test]
async fn replacement_reserves_before_exchange_and_invalidates_old_playout_at_commit() -> Result<()>
{
    let s = spec(1, "a", Codec::RawS16Le, 100, &["mono"], 1000);
    let reqs = [request(1, 1, Encoding::Raw), request(1, 2, Encoding::Raw)];
    let (a, b, _ea, _eb) = pair(std::slice::from_ref(&s), &[], &reqs, &[]).await?;
    let arun = tokio::spawn(a.clone().run());
    let brun = tokio::spawn(b.clone().run());
    let anchor = Instant::now() + Duration::from_millis(100);
    a.negotiate_codec(reqs[0].clone(), anchor).await?;
    let old = b.create_codec_playout(
        1,
        1,
        PlayoutConfig {
            anchor,
            target_buffer_frames: 0,
        },
    )?;
    let change = a.negotiate_codec(reqs[1].clone(), anchor);
    tokio::pin!(change);
    assert!(poll_once(change.as_mut()).await.is_pending());
    assert!(a.send_codec(1, 1, &vec![0; 2000]).await.is_err());
    change.await?;
    assert_eq!(old.revocation().status(), crate::RevocationStatus::Complete);
    assert!(b.codec_timeline_observation(&old).is_err());
    let mut new = b.create_codec_playout(
        1,
        2,
        PlayoutConfig {
            anchor,
            target_buffer_frames: 0,
        },
    )?;
    a.send_codec(1, 2, &vec![0x36; 2000]).await?;
    service(&b, &mut new).await?;
    assert_eq!(render(&mut new, anchor, 0), vec![0x36; 2000]);
    a.close();
    let _ = arun.await?;
    let _ = brun.await?;
    Ok(())
}

#[tokio::test]
async fn revoke_stops_actual_capacity_blocked_enqueue_before_driver_tick() -> Result<()> {
    let s = spec(1, "a", Codec::RawS16Le, 10, &["mono"], 952);
    let req = request(1, 1, Encoding::Raw);
    let mut budget = limits();
    budget.max_streams = 1;
    budget.packets_per_stream = 1;
    budget.max_datagram = 1000;
    let mut c = config();
    c.max_streams = 1;
    c.window = 1;
    let (a, b, _ea, _eb) = pair_configured(
        std::slice::from_ref(&s),
        &[],
        std::slice::from_ref(&req),
        &[],
        std::slice::from_ref(&req),
        &[],
        c,
        budget,
    )
    .await?;
    let arun = tokio::spawn(a.clone().run());
    let brun = tokio::spawn(b.clone().run());
    let anchor = Instant::now() - Duration::from_millis(1);
    a.negotiate_codec(req, anchor).await?;
    let raw = vec![0x5a; 1904];
    let sending = a.send_codec(1, 1, &raw);
    tokio::pin!(sending);
    // The first poll may register the elapsed Tokio timer; keep polling locally
    // only until the first fragment occupies the actual one-datagram send queue.
    for _ in 0..3 {
        assert!(poll_once(sending.as_mut()).await.is_pending());
    }
    assert!(
        a.base.connection.datagram_send_buffer_space() < a.config.max_datagram,
        "actual QUIC queue filled"
    );
    a.revoke_send(1)?;
    assert!(
        matches!(poll_once(sending.as_mut()).await, Poll::Ready(Err(_))),
        "revocation must end the actual blocked enqueue before any driver turn"
    );
    assert!(
        !a.codec_capability(request(1, 1, Encoding::Raw), true)
            .available
    );
    a.close();
    let _ = arun.await?;
    let _ = brun.await?;
    Ok(())
}

#[tokio::test]
async fn cancel_pacing_or_partial_send_retires_only_owned_generation_and_restart_recovers()
-> Result<()> {
    for partial in [false, true] {
        let s = spec(1, "a", Codec::RawS16Le, 100, &["mono"], 1000);
        let sibling = spec(2, "a", Codec::RawS16Le, 100, &["mono"], 1000);
        let reqs = [
            request(1, 1, Encoding::Raw),
            request(1, 2, Encoding::Raw),
            request(2, 1, Encoding::Raw),
        ];
        let (a, b, _ea, _eb) = pair(&[s.clone(), sibling], &[], &reqs, &[]).await?;
        let arun = tokio::spawn(a.clone().run());
        let brun = tokio::spawn(b.clone().run());
        let future_anchor = if partial {
            Instant::now() - Duration::from_millis(1)
        } else {
            Instant::now() + Duration::from_secs(20)
        };
        a.negotiate_codec(reqs[0].clone(), future_anchor).await?;
        let raw = vec![0x77; 2000];
        {
            let sending = a.send_codec(1, 1, &raw);
            tokio::pin!(sending);
            assert!(poll_once(sending.as_mut()).await.is_pending());
        }
        assert!(
            a.send_codec(1, 1, &raw).await.is_err(),
            "canceled generation cannot resume stale encoder state"
        );
        let anchor = Instant::now() + Duration::from_millis(80);
        a.negotiate_codec(reqs[2].clone(), anchor).await?;
        a.negotiate_codec(reqs[1].clone(), anchor).await?;
        let mut p1 = b.create_codec_playout(
            1,
            2,
            PlayoutConfig {
                anchor,
                target_buffer_frames: 0,
            },
        )?;
        let mut p2 = b.create_codec_playout(
            2,
            1,
            PlayoutConfig {
                anchor,
                target_buffer_frames: 0,
            },
        )?;
        tokio::try_join!(a.send_codec(1, 2, &raw), a.send_codec(2, 1, &raw))?;
        service(&b, &mut p1).await?;
        service(&b, &mut p2).await?;
        assert_eq!(render(&mut p1, anchor, 0), raw);
        assert_eq!(render(&mut p2, anchor, 0), raw);
        b.close();
        assert_eq!(p1.revocation().status(), crate::RevocationStatus::Complete);
        assert_eq!(p2.revocation().status(), crate::RevocationStatus::Complete);
        a.close();
        let _ = arun.await?;
        let _ = brun.await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reordered_variable_raw_envelopes_replay_foreign_generation_and_reconnect_binding()
-> Result<()> {
    let s = spec(
        1,
        "a",
        Codec::RawS24Le,
        96_000,
        &["FR", "FL", "height"],
        1000,
    );
    let reqs = [request(1, 1, Encoding::Raw), request(1, 2, Encoding::Raw)];
    let (a, b, _ea, _eb) = pair(std::slice::from_ref(&s), &[], &reqs, &[]).await?;
    let arun = tokio::spawn(a.clone().run());
    let brun = tokio::spawn(b.clone().run());
    let anchor = Instant::now();
    a.negotiate_codec(reqs[0].clone(), anchor).await?;
    let raw0 = vec![0x12; 9000];
    let raw1 = vec![0x56; 9000];
    let packet0 = encode(&a, 1, &raw0);
    let packet1 = encode(&a, 1, &raw1);
    let mut p = b.create_codec_playout(
        1,
        1,
        PlayoutConfig {
            anchor,
            target_buffer_frames: 0,
        },
    )?;
    inject(&a, 1, 1, 1, &packet1, true).await?;
    inject(&a, 1, 1, 0, &packet0, true).await?;
    // Stateless service can transfer block one while block zero is still in
    // flight. Wait for both arrivals, not merely the first successful transfer.
    tokio::time::timeout(Duration::from_secs(1), async {
        while p.snapshot().buffered_frames < 2000 {
            b.service_codec_playout(&mut p)?;
            tokio::task::yield_now().await;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    assert_eq!(render(&mut p, anchor, 0), raw0);
    // Stateless envelopes can decode in arrival order; playout preserves the
    // original source positions even when block one arrived before block zero.
    assert_eq!(render(&mut p, anchor, 1), raw1);
    a.negotiate_codec(reqs[1].clone(), anchor).await?;
    assert_eq!(p.revocation().status(), crate::RevocationStatus::Complete);
    let mut restarted = b.create_codec_playout(
        1,
        2,
        PlayoutConfig {
            anchor,
            target_buffer_frames: 0,
        },
    )?;
    // Old-generation fragment header cannot allocate a new-generation slot.
    inject(&a, 1, 1, 0, &packet0, false).await?;
    let packet2 = encode(&a, 1, &raw1);
    inject(&a, 1, 2, 0, &packet2, false).await?;
    service(&b, &mut restarted).await?;
    assert_eq!(render(&mut restarted, anchor, 0), raw1);
    let old_binding = packet0[8..24].to_vec();
    a.close();
    let _ = arun.await?;
    let _ = brun.await?;
    let (a, b, _ea2, _eb2) = pair(std::slice::from_ref(&s), &[], &reqs, &[]).await?;
    let arun = tokio::spawn(a.clone().run());
    let brun = tokio::spawn(b.clone().run());
    a.negotiate_codec(reqs[0].clone(), anchor).await?;
    let fresh = encode(&a, 1, &raw0);
    assert_ne!(&fresh[8..24], old_binding);
    let mut p = b.create_codec_playout(
        1,
        1,
        PlayoutConfig {
            anchor,
            target_buffer_frames: 0,
        },
    )?;
    inject(&a, 1, 1, 0, &packet0, false).await?;
    assert!(
        service(&b, &mut p).await.is_err(),
        "old TLS exporter binding cannot be replayed on a new connection"
    );
    assert_eq!(p.revocation().status(), crate::RevocationStatus::Complete);
    a.close();
    let _ = arun.await?;
    let _ = brun.await?;
    Ok(())
}

#[cfg(feature = "system-zstd")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_zstd_dictionary_bomb_and_foreign_plan_retire_only_affected_stream() -> Result<()>
{
    for attack in 0..5 {
        let s = spec(1, "a", Codec::RawF64Le, 48_000, &["FR", "FL"], 1000);
        let sibling = spec(2, "a", Codec::RawS32Le, 48_000, &["aux"], 1000);
        let reqs = [
            request(1, 1, Encoding::Zstd { level: 1 }),
            request(2, 1, Encoding::Raw),
        ];
        let (a, b, _ea, _eb) = pair(&[s, sibling], &[], &reqs, &[]).await?;
        let arun = tokio::spawn(a.clone().run());
        let brun = tokio::spawn(b.clone().run());
        let anchor = Instant::now() + Duration::from_millis(80);
        for req in &reqs {
            a.negotiate_codec(req.clone(), anchor).await?;
        }
        let mut packet = encode(&a, 1, &vec![0; 16000]);
        match attack {
            0 => packet[24..32].copy_from_slice(&2u64.to_be_bytes()), // foreign stream in MXC1
            1 => packet[32..40].copy_from_slice(&2u64.to_be_bytes()), // foreign generation
            2 => packet[4] = 0,                                       // raw envelope on Zstd plan
            3 => packet[codec::HEADER_BYTES + 4] |= 1,                // dictionary flag forbidden
            _ => {
                // Known-size content claim beyond exact admitted output (expansion bomb).
                packet[codec::HEADER_BYTES + 6] = 0xff;
                packet[codec::HEADER_BYTES + 7] = 0xff;
            }
        }
        let mut p1 = b.create_codec_playout(
            1,
            1,
            PlayoutConfig {
                anchor,
                target_buffer_frames: 0,
            },
        )?;
        let mut p2 = b.create_codec_playout(
            2,
            1,
            PlayoutConfig {
                anchor,
                target_buffer_frames: 0,
            },
        )?;
        inject(&a, 1, 1, 0, &packet, true).await?;
        assert!(service(&b, &mut p1).await.is_err(), "attack {attack}");
        assert_eq!(p1.revocation().status(), crate::RevocationStatus::Complete);
        let raw = vec![0x24; 4000];
        a.send_codec(2, 1, &raw).await?;
        service(&b, &mut p2).await?;
        assert_eq!(render(&mut p2, anchor, 0), raw);
        assert!(a.base.connection.close_reason().is_none());
        assert!(
            b.receiver.lock().unwrap().reassembler.buffered_bytes() <= b.config.max_buffered_bytes
        );
        a.close();
        let _ = arun.await?;
        let _ = brun.await?;
    }
    Ok(())
}

#[cfg(feature = "system-opus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn opus_nonfinite_input_and_loss_need_explicit_restart_without_harming_raw_sibling()
-> Result<()> {
    let s = spec(1, "a", Codec::RawF64Le, 48_000, &["mono"], 960);
    let sibling = spec(2, "a", Codec::RawF64Le, 48_000, &["mono"], 960);
    let reqs = [
        request(1, 1, support::opus()),
        request(1, 2, support::opus()),
        request(2, 1, Encoding::Raw),
    ];
    let (a, b, _ea, _eb) = pair(&[s, sibling], &[], &reqs, &[]).await?;
    let arun = tokio::spawn(a.clone().run());
    let brun = tokio::spawn(b.clone().run());
    let anchor = Instant::now() + Duration::from_millis(80);
    a.negotiate_codec(reqs[0].clone(), anchor).await?;
    a.negotiate_codec(reqs[2].clone(), anchor).await?;
    let nan: Vec<_> = std::iter::repeat_n(0x7ff8000000001234u64, 960)
        .flat_map(u64::to_le_bytes)
        .collect();
    let mut p2 = b.create_codec_playout(
        2,
        1,
        PlayoutConfig {
            anchor,
            target_buffer_frames: 0,
        },
    )?;
    let (failed, raw) = tokio::join!(a.send_codec(1, 1, &nan), a.send_codec(2, 1, &nan));
    assert!(failed.is_err(), "Opus finite conversion required");
    raw?;
    service(&b, &mut p2).await?;
    assert_eq!(
        render(&mut p2, anchor, 0),
        nan,
        "raw sibling retains NaN payloads"
    );
    a.negotiate_codec(reqs[1].clone(), Instant::now()).await?;
    let mut p = b.create_codec_playout(
        1,
        2,
        PlayoutConfig {
            anchor,
            target_buffer_frames: 0,
        },
    )?;
    let finite = vec![0; 7680];
    let _lost = encode(&a, 1, &finite);
    let second = encode(&a, 1, &finite);
    inject(&a, 1, 2, 1, &second, false).await?;
    // Consumer explicitly advances after missing source block zero. No hidden PLC
    // or timestamp shift can continue the old Opus predictor generation.
    let mut silence = vec![1; 7680];
    p.render_into(anchor, &mut silence)?;
    assert!(silence.iter().all(|v| *v == 0));
    assert!(service(&b, &mut p).await.is_err());
    assert_eq!(p.revocation().status(), crate::RevocationStatus::Complete);
    a.close();
    let _ = arun.await?;
    let _ = brun.await?;
    Ok(())
}

#[tokio::test]
async fn missed_source_slot_is_never_retimed_and_bounded_work_guard_retires_generation()
-> Result<()> {
    let s = spec(1, "a", Codec::RawS16Le, 48_000, &["mono"], 960);
    let reqs = [request(1, 1, Encoding::Raw), request(1, 2, Encoding::Raw)];
    let (mut a, b, _ea, _eb) = pair(std::slice::from_ref(&s), &[], &reqs, &[]).await?;
    // A valid nonzero configured bound, deliberately below the work required by
    // this fixture. No native clock or hardware deadline is simulated.
    Arc::get_mut(&mut a)
        .unwrap()
        .codecs
        .as_mut()
        .unwrap()
        .max_work_wall = Duration::from_nanos(1);
    let arun = tokio::spawn(a.clone().run());
    let brun = tokio::spawn(b.clone().run());
    a.negotiate_codec(reqs[0].clone(), Instant::now() - Duration::from_secs(1))
        .await?;
    let raw = vec![0; 1920];
    let missed = a.send_codec(1, 1, &raw).await.unwrap_err();
    assert!(missed.to_string().contains("missed source slot"));
    assert!(!a.codec_capability(reqs[0].clone(), true).available);
    let anchor = Instant::now() + Duration::from_millis(50);
    a.negotiate_codec(reqs[1].clone(), anchor).await?;
    let exceeded = a.send_codec(1, 2, &raw).await.unwrap_err();
    assert!(exceeded.to_string().contains("work wall budget exceeded"));
    assert!(!a.codec_capability(reqs[1].clone(), true).available);
    assert_eq!(
        b.receiver.lock().unwrap().reassembler.buffered_bytes(),
        0,
        "neither failure may enqueue a partial or retimed block"
    );
    a.close();
    let _ = arun.await?;
    let _ = brun.await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reliable_socket_cannot_replace_canonical_precision_channels_provenance_or_codec_authority()
-> Result<()> {
    let s = spec(1, "a", Codec::RawF64Le, 44_117, &["FR", "FL"], 1000);
    let req = request(1, 1, Encoding::Raw);
    let (a, b, _ea, _eb) = pair(
        std::slice::from_ref(&s),
        &[],
        std::slice::from_ref(&req),
        &[],
    )
    .await?;
    let arun = tokio::spawn(a.clone().run());
    let brun = tokio::spawn(b.clone().run());
    for attack in 0..8 {
        let mut forged = s.clone();
        let mut selection = req.clone();
        match attack {
            0 => forged.sample_rate = 48_000,
            1 => forged.codec = Codec::RawF32Le,
            2 => forged.channels.reverse(),
            3 => forged.provenance.sources[0].source = "another-source".into(),
            4 => forged.provenance.sources[0]
                .groups
                .push("call-return".into()),
            5 => selection = request(1, 1, Encoding::Zstd { level: 1 }),
            6 => selection = request(1, 1, support::opus()),
            _ => {}
        }
        let wire = BlockOffer {
            version: VERSION,
            generation: 1,
            spec: forged,
            max_datagram: a.config.max_datagram,
            codec: Some(selection),
            max_envelope_bytes: Some(
                s.payload_bytes()? + codec::HEADER_BYTES + usize::from(attack == 7),
            ),
        };
        let (mut send, mut receive) = a.base.connection.open_bi().await?;
        write_control(&mut send, &wire).await?;
        send.finish()?;
        let reply: Reply = read_control(&mut receive).await?;
        assert!(!reply.accepted, "foreign descriptor/plan attack {attack}");
        assert!(b.receiver.lock().unwrap().codecs.is_empty());
    }
    let anchor = Instant::now() + Duration::from_millis(50);
    a.negotiate_codec(req, anchor).await?;
    let mut p = b.create_codec_playout(
        1,
        1,
        PlayoutConfig {
            anchor,
            target_buffer_frames: 0,
        },
    )?;
    let original: Vec<_> = std::iter::repeat_n(0x3ff0000000000001u64, 2000)
        .flat_map(u64::to_le_bytes)
        .collect();
    a.send_codec(1, 1, &original).await?;
    service(&b, &mut p).await?;
    assert_eq!(render(&mut p, anchor, 0), original);
    assert_eq!(p.spec(), &s);
    a.close();
    let _ = arun.await?;
    let _ = brun.await?;
    Ok(())
}

#[cfg(feature = "system-zstd")]
#[tokio::test]
async fn unequal_encoded_maxima_are_rejected_reliably_before_variable_admission() -> Result<()> {
    let s = spec(1, "a", Codec::RawS24Le, 48_000, &["mono"], 960);
    let req = request(1, 1, Encoding::Zstd { level: 1 });
    let (mut a, b, _ea, _eb) = pair(
        std::slice::from_ref(&s),
        &[],
        std::slice::from_ref(&req),
        &[],
    )
    .await?;
    // Both valid budgets support the exact codec, but their reserved variable
    // geometries differ. The reliable exchange must reject this explicitly.
    Arc::get_mut(&mut a)
        .unwrap()
        .codecs
        .as_mut()
        .unwrap()
        .budget
        .max_encoded_bytes -= 1;
    let arun = tokio::spawn(a.clone().run());
    let brun = tokio::spawn(b.clone().run());
    let failure = a.negotiate_codec(req, Instant::now()).await.unwrap_err();
    assert!(failure.to_string().contains("envelope maximum mismatch"));
    assert!(b.receiver.lock().unwrap().codecs.is_empty());
    assert!(a.base.connection.close_reason().is_none());
    a.close();
    let _ = arun.await?;
    let _ = brun.await?;
    Ok(())
}

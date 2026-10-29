use super::*;
#[path = "../support/mod.rs"]
pub(crate) mod support;
use support::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reviewer_cancel_or_timeout_before_peer_turnaround_preserves_audio() -> Result<()> {
    reviewer_abandon_probe_before_peer_turnaround(true).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reviewer_timeout_before_peer_turnaround_preserves_audio() -> Result<()> {
    reviewer_abandon_probe_before_peer_turnaround(false).await
}

async fn reviewer_abandon_probe_before_peer_turnaround(cancel: bool) -> Result<()> {
    let s = spec(1, "a", Codec::RawS16Le, 48_000, &["FL"], 48);
    let (a, b, _ea, _eb) = pair(std::slice::from_ref(&s), &[]).await?;
    // Keep the actual receive worker alive but pause its polling after admission.
    let mut worker = Box::pin(b.clone().run());
    tokio::select! {
        result = a.negotiate(offer(s.clone(), 1)) => result?,
        result = &mut worker => panic!("unexpected worker exit: {result:?}"),
    }
    let caller = a.clone();
    let task = tokio::spawn(async move { caller.probe_rtt().await });
    tokio::time::sleep(Duration::from_millis(30)).await;
    if cancel {
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
    } else {
        assert!(task.await?.is_err());
    }
    // Allow QUIC to deliver STOP_SENDING, then resume the production worker.
    tokio::time::sleep(Duration::from_millis(30)).await;
    tokio::select! {
        result = &mut worker => panic!("cancel={cancel}: optional probe closed audio worker: {result:?}"),
        _ = tokio::time::sleep(Duration::from_millis(100)) => {},
    }
    a.enqueue(1, sample_bytes(&s, 0)).await?;
    a.flush_round().await?;
    tokio::select! {
        result = wait_queued(&b, 1, 1) => result?,
        result = &mut worker => panic!("unexpected worker exit: {result:?}"),
    }
    assert_eq!(b.pull(1)?, sample_bytes(&s, 0));
    a.close();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reviewer_concurrent_probe_and_negotiation_preserve_connection() -> Result<()> {
    for attempt in 0..20 {
        let s = spec(1, "a", Codec::RawS16Le, 48_000, &["FL"], 48);
        let (a, b, _ea, _eb) = pair(std::slice::from_ref(&s), &[]).await?;
        let caller = a.clone();
        let probe = tokio::spawn(async move { caller.probe_rtt().await });
        tokio::time::sleep(Duration::from_millis(10)).await;
        let sender = a.clone();
        let negotiation = tokio::spawn(async move { sender.negotiate(offer(s, 1)).await });
        tokio::time::sleep(Duration::from_millis(10)).await;
        let run_b = tokio::spawn(b.clone().run());
        let measured = probe.await?;
        let admitted = negotiation.await?;
        assert!(
            admitted.is_ok(),
            "attempt {attempt}: probe={measured:?}, admission={admitted:?}"
        );
        assert!(a.connection.close_reason().is_none());
        a.close();
        let _ = run_b.await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn encrypted_loss_reorder_stale_packets_and_flooded_stream_leave_sibling_usable() -> Result<()>
{
    let first = spec(1, "a", Codec::RawS24Le, 96_000, &["FL", "FR"], 48);
    let sibling = spec(2, "a", Codec::RawF64Le, 192_000, &["FL", "FR"], 24);
    let (a, b, _ea, _eb) = pair(&[first.clone(), sibling.clone()], &[]).await?;
    let run_a = tokio::spawn(a.clone().run());
    let run_b = tokio::spawn(b.clone().run());
    for s in [&first, &sibling] {
        a.negotiate(offer(s.clone(), 1)).await?;
    }
    for sequence in [2, 0, 0, 99] {
        // omit packet 1; send packet 2 before 0
        a.connection.send_datagram(
            Frame {
                stream: 1,
                generation: 1,
                sequence,
                sample_timestamp: sequence * 48,
                payload: sample_bytes(&first, sequence as usize),
            }
            .encode(1200)?
            .into(),
        )?;
    }
    a.enqueue(2, sample_bytes(&sibling, 0)).await?;
    a.flush_round().await?;
    wait_queued(&b, 2, 1).await?;
    wait_queued(&b, 1, 2).await?;
    let timing = b.timing_snapshot().await;
    assert_eq!(timing.receive_streams[0].queued_sample_frames, 96);
    assert_eq!(timing.receive_streams[0].queued_sample_ms, 1.0);
    assert_eq!(timing.receive_streams[0].buffered_timeline_ms, Some(1.5));
    assert_eq!(timing.receive_streams[1].queued_sample_ms, 0.125);
    assert_eq!(b.pull(2)?, sample_bytes(&sibling, 0));
    assert_eq!(b.pull(1)?, sample_bytes(&first, 0));
    assert_eq!(b.pull(1)?, vec![0; first.payload_bytes()?]);
    assert_eq!(b.pull(1)?, sample_bytes(&first, 2));
    // A saturated producer only loses its own bounded pending packets.
    for sequence in 0..30 {
        a.enqueue(1, sample_bytes(&first, sequence)).await?;
    }
    a.enqueue(2, sample_bytes(&sibling, 1)).await?;
    a.flush_round().await?;
    wait_queued(&b, 2, 1).await?;
    assert_eq!(b.pull(2)?, sample_bytes(&sibling, 1));
    assert_eq!(a.sender_counters(1).await?.send_overflow, 22);
    // A generation change must reject authenticated, but obsolete, payloads too.
    a.negotiate(offer(first.clone(), 2)).await?;
    a.connection.send_datagram(
        Frame {
            stream: 1,
            generation: 1,
            sequence: 0,
            sample_timestamp: 0,
            payload: sample_bytes(&first, 0),
        }
        .encode(1200)?
        .into(),
    )?;
    a.enqueue(1, sample_bytes(&first, 3)).await?;
    a.flush_round().await?;
    wait_queued(&b, 1, 1).await?;
    assert_eq!(b.pull(1)?, sample_bytes(&first, 3));
    assert_eq!(b.counters(1)?.stale_generation, 1);
    a.close();
    let _ = run_a.await?;
    let _ = run_b.await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slow_control_does_not_block_audio_and_deadline_fails_closed() -> Result<()> {
    let s = spec(1, "a", Codec::RawS16Le, 48_000, &["FL", "FR"], 48);
    let (a, b, _ea, _eb) = pair(std::slice::from_ref(&s), &[]).await?;
    let run_a = tokio::spawn(a.clone().run());
    let run_b = tokio::spawn(b.clone().run());
    a.negotiate(offer(s.clone(), 1)).await?;
    let (mut stalled, _reply) = a.connection.open_bi().await?;
    stalled.write_all(&[0]).await?; // no complete length; keep stream open
    a.enqueue(1, sample_bytes(&s, 0)).await?;
    a.flush_round().await?;
    wait_queued(&b, 1, 1).await?;
    assert_eq!(b.pull(1)?, sample_bytes(&s, 0));
    timeout(Duration::from_secs(3), b.connection.closed()).await?;
    let _ = run_a.await?;
    let _ = run_b.await?;
    assert!(b.pull(1)?.iter().all(|b| *b == 0));
    assert!(a.enqueue(1, sample_bytes(&s, 1)).await.is_err());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_oversize_trailing_and_partial_control_close_session() -> Result<()> {
    let cases = [
        (CONTROL_LIMIT as u32 + 1).to_be_bytes().to_vec(),
        [1u32.to_be_bytes().as_slice(), b"{"].concat(),
        [2u32.to_be_bytes().as_slice(), b"{}", b"trailing"].concat(),
        vec![0, 0],
    ];
    for bytes in cases {
        let (a, b, _ea, _eb) = pair(&[], &[]).await?;
        let run_b = tokio::spawn(b.clone().run());
        let (mut send, _) = a.connection.open_bi().await?;
        send.write_all(&bytes).await?;
        send.finish()?;
        timeout(Duration::from_secs(3), b.connection.closed()).await?;
        assert!(run_b.await?.is_err());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_rejects_ungranted_stream_format_opus_and_version() -> Result<()> {
    let allowed = spec(1, "a", Codec::RawS16Le, 48_000, &["FL", "FR"], 48);
    let (a, b, _ea, _eb) = pair(std::slice::from_ref(&allowed), &[]).await?;
    let run_b = tokio::spawn(b.clone().run());
    let mut unknown = offer(allowed.clone(), 1);
    unknown.spec.stream = 99;
    let mut opus = offer(allowed.clone(), 1);
    opus.spec.codec = Codec::Opus;
    let mut changed_rate = offer(allowed.clone(), 1);
    changed_rate.spec.sample_rate = 96_000;
    let mut version = offer(allowed, 1);
    version.version = 99;
    for bad in [unknown, opus, changed_rate, version] {
        let (mut send, mut receive) = a.connection.open_bi().await?;
        write_control(&mut send, &bad).await?;
        send.finish()?;
        let reply: Reply = timeout(Duration::from_secs(2), read_control(&mut receive)).await??;
        assert!(!reply.accepted);
        assert!(!reply.reason.is_empty());
    }
    assert_eq!(b.queued(1), 0);
    a.close();
    let _ = run_b.await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_receive_worker_flushes_pcm_and_closes_connection() -> Result<()> {
    let s = spec(1, "a", Codec::RawS16Le, 48_000, &["FL", "FR"], 48);
    let (a, b, _ea, _eb) = pair(std::slice::from_ref(&s), &[]).await?;
    let run_b = tokio::spawn(b.clone().run());
    a.negotiate(offer(s.clone(), 1)).await?;
    a.enqueue(1, sample_bytes(&s, 0)).await?;
    a.flush_round().await?;
    wait_queued(&b, 1, 1).await?;
    run_b.abort();
    assert!(run_b.await.unwrap_err().is_cancelled());
    assert_eq!(b.queued(1), 0);
    assert!(b.pull(1)?.iter().all(|byte| *byte == 0));
    timeout(Duration::from_secs(2), a.connection.closed()).await?;
    assert!(a.enqueue(1, sample_bytes(&s, 0)).await.is_err());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_uncertain_admission_closes_connection() -> Result<()> {
    let s = spec(1, "a", Codec::RawS16Le, 48_000, &["FL", "FR"], 48);
    let (a, b, _ea, _eb) = pair(std::slice::from_ref(&s), &[]).await?;
    let sender = a.clone();
    let task = tokio::spawn(async move { sender.negotiate(offer(s, 1)).await });
    let (_send, mut receive) = timeout(Duration::from_secs(2), b.connection.accept_bi()).await??;
    let _: Offer = timeout(Duration::from_secs(2), read_control(&mut receive)).await??;
    // Peer received the offer; caller cannot assume whether it was applied.
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    timeout(Duration::from_secs(2), b.connection.closed()).await?;
    assert!(a.enqueue(1, vec![0; 192]).await.is_err());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn paired_probe_has_no_initial_sample_and_expires_without_refresh() -> Result<()> {
    let (a, b, _ea, _eb) = pair(&[], &[]).await?;
    let initial = a.timing_snapshot().await;
    assert!(initial.connected);
    assert_eq!(initial.application_rtt.validity, RttValidity::Unavailable);
    assert_eq!(initial.application_rtt.value_ms, None);
    let run_a = tokio::spawn(a.clone().run());
    let run_b = tokio::spawn(b.clone().run());
    let elapsed = a.probe_rtt().await?;
    let first = a.timing_snapshot().await;
    assert_eq!(first.application_rtt.validity, RttValidity::Fresh);
    assert_eq!(first.application_rtt.value_ms, Some(ms(elapsed)));
    assert_eq!(first.application_rtt.last_attempt, ProbeStatus::Succeeded);
    assert!(a.probe_rtt().await.is_err()); // admission does not overwrite the successful sample
    let json = serde_json::to_value(&first)?;
    assert_eq!(
        json["application_rtt"]["kind"],
        "application_control_round_trip"
    );
    assert!(json["clock_sync"]["offset_ms"].is_null());
    assert!(json["clock_sync"]["uncertainty_ms"].is_null());
    assert!(json["one_way_network_ms"].is_null());
    assert!(json["end_to_end_audio_ms"].is_null());
    tokio::time::sleep(RTT_FRESH_FOR + Duration::from_millis(20)).await;
    let stale = a.timing_snapshot().await;
    assert_eq!(stale.application_rtt.validity, RttValidity::Stale);
    assert_eq!(
        stale.application_rtt.measured_at_session_ms,
        first.application_rtt.measured_at_session_ms
    );
    assert!(stale.application_rtt.age_ms.unwrap() > 3000.0);
    a.close();
    assert_eq!(
        a.timing_snapshot().await.application_rtt.validity,
        RttValidity::Disconnected
    );
    let _ = run_a.await?;
    let _ = run_b.await?;
    assert!(!b.timing_snapshot().await.connected);
    Ok(())
}

async fn incoming_ping(session: &Session) -> Result<(SendStream, RecvStream, u64)> {
    let (send, mut receive) = session.connection.accept_bi().await?;
    let mut tag = [0; 4];
    receive.read_exact(&mut tag).await?;
    ensure!(tag == PROBE_TAG, "expected probe tag");
    let nonce = read_probe_body(&mut receive, PROBE_PING).await?;
    Ok((send, receive, nonce))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delayed_authenticated_echo_is_application_rtt_not_a_symmetric_one_way_estimate()
-> Result<()> {
    let (a, b, _ea, _eb) = pair(&[], &[]).await?;
    let peer = tokio::spawn(async move {
        let (mut send, _, nonce) = incoming_ping(&b).await?;
        tokio::time::sleep(Duration::from_millis(45)).await;
        write_probe(&mut send, PROBE_ECHO, nonce).await?;
        send.finish()?;
        // Keep the remote Session alive through delivery.
        let _ = send.stopped().await;
        Ok::<_, anyhow::Error>(b)
    });
    assert!(a.probe_rtt().await? >= Duration::from_millis(45));
    let timing = a.timing_snapshot().await;
    assert!(timing.one_way_network_ms.is_none());
    assert!(timing.clock_sync.uncertainty_ms.is_none());
    let b = peer.await??;
    a.close();
    b.close();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn timeout_and_cancellation_release_probe_admission_without_closing_audio_session()
-> Result<()> {
    for cancel in [false, true] {
        let s = spec(1, "a", Codec::RawS16Le, 48_000, &["FL", "FR"], 48);
        let (a, b, _ea, _eb) = pair(std::slice::from_ref(&s), &[]).await?;
        let caller = a.clone();
        let task = tokio::spawn(async move { caller.probe_rtt().await });
        let (send, receive, _) = incoming_ping(&b).await?;
        assert_eq!(
            a.timing_snapshot().await.application_rtt.last_attempt,
            ProbeStatus::InFlight
        );
        assert!(a.probe_rtt().await.is_err());
        if cancel {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            assert!(task.await?.is_err());
        }
        drop((send, receive));
        let timing = a.timing_snapshot().await;
        assert!(timing.connected);
        assert_eq!(timing.application_rtt.value_ms, None);
        assert_eq!(
            timing.application_rtt.last_attempt,
            if cancel {
                ProbeStatus::Cancelled
            } else {
                ProbeStatus::TimedOut
            }
        );
        let run_b = tokio::spawn(b.clone().run());
        a.negotiate(offer(s.clone(), 1)).await?;
        a.enqueue(1, sample_bytes(&s, 0)).await?;
        a.flush_round().await?;
        wait_queued(&b, 1, 1).await?;
        assert_eq!(b.pull(1)?, sample_bytes(&s, 0));
        tokio::time::sleep(PROBE_INTERVAL).await;
        a.probe_rtt().await?;
        a.close();
        let _ = run_b.await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_echo_cannot_supply_a_timing_sample() -> Result<()> {
    let (a, b, _ea, _eb) = pair(&[], &[]).await?;
    let caller = a.clone();
    let task = tokio::spawn(async move { caller.probe_rtt().await });
    let (mut send, _, nonce) = incoming_ping(&b).await?;
    write_probe(&mut send, PROBE_ECHO, nonce + 1).await?;
    send.finish()?;
    assert!(task.await?.is_err());
    let timing = a.timing_snapshot().await;
    assert_eq!(timing.application_rtt.last_attempt, ProbeStatus::Failed);
    assert_eq!(timing.application_rtt.validity, RttValidity::Unavailable);
    a.close();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_probe_and_control_flood_are_bounded() -> Result<()> {
    let good = [
        PROBE_TAG.as_slice(),
        &PROBE_VERSION.to_be_bytes(),
        &PROBE_PING.to_be_bytes(),
        &1u64.to_be_bytes(),
    ]
    .concat();
    let mut version = good.clone();
    version[5] = 99;
    let mut kind = good.clone();
    kind[7] = 99;
    for bytes in [
        version,
        kind,
        good[..10].to_vec(),
        [good.as_slice(), b"x"].concat(),
    ] {
        let (a, b, _ea, _eb) = pair(&[], &[]).await?;
        let run_b = tokio::spawn(b.clone().run());
        let (mut send, _) = a.connection.open_bi().await?;
        send.write_all(&bytes).await?;
        send.finish()?;
        timeout(Duration::from_secs(3), b.connection.closed()).await?;
        assert!(run_b.await?.is_err());
        assert!(b.timing_snapshot().await.receive_streams.is_empty());
    }
    let (a, b, _ea, _eb) = pair(&[], &[]).await?;
    let run_b = tokio::spawn(b.clone().run());
    for nonce in 0..(limits().max_streams as u64 + 17) {
        let request = async {
            let (mut send, mut receive) = a.connection.open_bi().await?;
            write_probe(&mut send, PROBE_PING, nonce).await?;
            send.finish()?;
            let mut tag = [0; 4];
            receive.read_exact(&mut tag).await?;
            ensure!(tag == PROBE_TAG, "bad echo");
            ensure!(
                read_probe_body(&mut receive, PROBE_ECHO).await? == nonce,
                "bad nonce"
            );
            Ok::<_, anyhow::Error>(())
        }
        .await;
        if request.is_err() {
            break;
        }
    }
    timeout(Duration::from_secs(3), b.connection.closed()).await?;
    assert!(
        run_b
            .await?
            .unwrap_err()
            .to_string()
            .contains("control stream rate exceeded")
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incomplete_probe_and_busy_controls_leave_datagrams_independent() -> Result<()> {
    let s = spec(1, "a", Codec::RawS24Le, 96_000, &["FL", "FR"], 48);
    let (a, b, _ea, _eb) = pair(std::slice::from_ref(&s), &[]).await?;
    let run_b = tokio::spawn(b.clone().run());
    a.negotiate(offer(s.clone(), 1)).await?;
    let (mut send, _receive) = a.connection.open_bi().await?;
    send.write_all(&PROBE_TAG).await?;
    // A second request is reset, without an unbounded task or waiting queue.
    let caller = a.clone();
    let probe = tokio::spawn(async move { caller.probe_rtt().await });
    a.enqueue(1, sample_bytes(&s, 0)).await?;
    a.flush_round().await?;
    wait_queued(&b, 1, 1).await?;
    assert_eq!(b.pull(1)?, sample_bytes(&s, 0));
    assert!(probe.await?.is_err());
    timeout(Duration::from_secs(3), b.connection.closed()).await?;
    let _ = run_b.await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_configured_admission_burst_leaves_headroom_for_telemetry() -> Result<()> {
    let streams: Vec<_> = (1..=limits().max_streams as u64)
        .map(|id| spec(id, "a", Codec::RawS16Le, 48_000, &["FL"], 48))
        .collect();
    let (a, b, _ea, _eb) = pair(&streams, &[]).await?;
    let run_b = tokio::spawn(b.clone().run());
    for s in streams {
        a.negotiate(offer(s, 1)).await.context("burst admission")?;
    }
    a.probe_rtt().await.context("burst probe")?;
    assert_eq!(
        b.timing_snapshot().await.receive_streams.len(),
        limits().max_streams
    );
    a.close();
    let _ = run_b.await?;
    Ok(())
}

async fn abandoned_probe_then_negotiate(cancel: bool) -> Result<()> {
    let mut failures = Vec::new();
    for attempt in 0..20 {
        let s = spec(1, "a", Codec::RawS16Le, 48_000, &["FL"], 48);
        let (a, b, _ea, _eb) = pair(std::slice::from_ref(&s), &[]).await?;
        let mut worker = Box::pin(b.clone().run());
        tokio::select! {
            r=a.negotiate(offer(s.clone(),1)) => r?,
            r=&mut worker => panic!("worker exited: {r:?}"),
        }
        let caller = a.clone();
        let probe = tokio::spawn(async move { caller.probe_rtt().await });
        tokio::time::sleep(Duration::from_millis(25)).await;
        if cancel {
            probe.abort();
            assert!(probe.await.unwrap_err().is_cancelled());
        } else {
            assert!(probe.await?.is_err());
        }
        assert!(a.connection.close_reason().is_none());
        let caller = a.clone();
        let next = s.clone();
        let mut admission = tokio::spawn(async move { caller.negotiate(offer(next, 2)).await });
        tokio::time::sleep(Duration::from_millis(25)).await;
        let admitted = tokio::select! {
            r=&mut admission => r?,
            r=&mut worker => panic!("attempt={attempt}: worker exited: {r:?}"),
        };
        if let Err(error) = admitted {
            timeout(Duration::from_secs(1), b.connection.closed()).await?;
            let silence = b.pull(1)?.iter().all(|v| *v == 0);
            let evidence = format!(
                "cancel={cancel}, attempt={attempt}: admission={error:#}, local_close={:?}, peer_close={:?}, admitted_audio_silent={silence}",
                a.connection.close_reason(),
                b.connection.close_reason()
            );
            println!("{evidence}");
            failures.push(evidence);
        } else {
            a.enqueue(1, sample_bytes(&s, 1)).await?;
            a.flush_round().await?;
            tokio::select! {
                r=wait_queued(&b,1,1) => r?,
                r=&mut worker => panic!("worker exited: {r:?}"),
            }
            assert_eq!(b.pull(1)?, sample_bytes(&s, 1));
        }
        a.close();
    }
    assert!(
        failures.is_empty(),
        "{} production lifecycle failures: {failures:#?}",
        failures.len()
    );
    Ok(())
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_probe_then_immediate_negotiation_preserves_audio() -> Result<()> {
    abandoned_probe_then_negotiate(true).await
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn timed_out_probe_then_immediate_negotiation_preserves_audio() -> Result<()> {
    abandoned_probe_then_negotiate(false).await
}

use super::*;
#[path = "../support/mod.rs"]
mod support;
use support::*;

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

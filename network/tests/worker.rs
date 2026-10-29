mod support;
use mixyard_network::{
    pcm::Outbox,
    worker::{control::*, *},
    *,
};
use std::time::{Duration, Instant};
use support::*;

fn config(packet: u16) -> JitterConfig {
    JitterConfig {
        min_frames: u64::from(packet),
        max_frames: u64::from(packet) * 6,
        initial_frames: u64::from(packet) * 3,
        adjust_interval: Duration::from_secs(1),
        fresh_for: Duration::from_secs(2),
        min_drift_span: Duration::from_secs(1),
    }
}

#[test]
fn cadence_exact_rational_slots_no_bursts_or_sibling_rate_coupling() {
    let anchor = Instant::now();
    let mut fast = Cadence::new(anchor, 44_117, 5).unwrap();
    let mut slow = Cadence::new(anchor, 1, 1).unwrap();
    assert_eq!(fast.due(anchor).unwrap().unwrap().sequence, 0);
    assert!(fast.due(anchor).unwrap().is_none());
    assert_eq!(slow.due(anchor).unwrap().unwrap().sequence, 0);
    for sequence in 1..1000 {
        let ns = (sequence * 5u64 * 1_000_000_000).div_ceil(44_117);
        let deadline = anchor + Duration::from_nanos(ns);
        assert_eq!(fast.next_deadline().unwrap(), deadline);
        assert!(
            fast.due(deadline - Duration::from_nanos(1))
                .unwrap()
                .is_none()
        );
        let tick = fast.due(deadline).unwrap().unwrap();
        assert_eq!((tick.sequence, tick.skipped), (sequence, 0));
    }
    let late = anchor + Duration::from_secs(60 * 60);
    let tick = fast.due(late).unwrap().unwrap();
    assert!(tick.skipped > 30_000_000);
    assert!(fast.due(late).unwrap().is_none());
    assert_eq!(slow.due(late).unwrap().unwrap().sequence, 3600);
    assert!(fast.due(anchor).is_err());
    // A legal very high rate has several sample slots per nanosecond: one wake, one packet.
    let mut high = Cadence::new(anchor, u32::MAX, 1).unwrap();
    assert_eq!(
        high.due(anchor + Duration::from_nanos(1))
            .unwrap()
            .unwrap()
            .sequence,
        4
    );
}

#[test]
fn paced_outbox_expires_old_bytes_partial_and_empty_slots_without_touching_siblings() {
    let a = spec(1, "a", Codec::RawF64Le, 1000, &["FR", "FL"], 5);
    let b = spec(2, "a", Codec::RawS24Le, 1000, &["selected"], 5);
    let oa = offer(a.clone(), 1);
    let ob = offer(b.clone(), 1);
    let mut out = Outbox::new(8);
    out.admit(oa.clone());
    out.admit(ob.clone());
    for seq in 0..5 {
        out.enqueue(a.stream, sample_bytes(&a, seq)).unwrap();
    }
    out.enqueue(b.stream, sample_bytes(&b, 0)).unwrap();
    let frame = out.due(&oa, 3).unwrap().unwrap();
    assert_eq!((frame.sequence, frame.sample_timestamp), (3, 15));
    assert_eq!(frame.payload, sample_bytes(&a, 3));
    assert_eq!(out.paced_expired(a.stream), 3);
    assert!(out.due(&oa, 3).is_err());
    assert!(out.due(&oa, 2).is_err());
    assert_eq!(
        out.due(&ob, 0).unwrap().unwrap().payload,
        sample_bytes(&b, 0)
    );
    assert!(out.due(&oa, 100_000_000).unwrap().is_none());
    out.enqueue(a.stream, sample_bytes(&a, 2)).unwrap();
    let next = out.due(&oa, 100_000_001).unwrap().unwrap();
    assert_eq!(next.sequence, 100_000_001);
    assert_eq!(next.payload, sample_bytes(&a, 2));
    out.admit(offer(a.clone(), 2));
    assert!(out.due(&oa, 100_000_002).is_err());
    let new = offer(a.clone(), 2);
    let bytes = sample_bytes(&a, 0);
    out.enqueue_pcm(a.stream, &bytes[..16]).unwrap();
    assert!(out.due(&new, 0).unwrap().is_none());
    assert_eq!(out.enqueue_pcm(a.stream, &bytes).unwrap(), 1);
    assert_eq!(out.due(&new, 1).unwrap().unwrap().payload, bytes);
    let mut wrong = new.clone();
    wrong.spec.channels.reverse();
    assert!(out.due(&wrong, 2).is_err());
}

#[test]
fn jitter_loss_drift_freshness_and_generation_are_explicit_and_bounded() {
    let anchor = Instant::now();
    let mut c = Controller::new(48_000, 480, 7, config(480), 480 * 8).unwrap();
    assert_eq!(c.estimate(anchor).validity, Validity::Unavailable);
    for n in 0..1001u64 {
        // Source is 200 ppm faster than the local arrival clock. No remote timestamp used.
        let at = anchor + Duration::from_nanos(n * 10_000_000 * 10_000 / 10_002);
        assert!(c.observe(7, n * 480, at));
    }
    let now = anchor + Duration::from_nanos(1000 * 10_000_000 * 10_000 / 10_002);
    let e = c.estimate(now);
    assert_eq!(e.sample_count, 1001);
    assert!((e.arrival_rate_error_ppm.unwrap() - 200.0).abs() < 0.01);
    assert!(e.uncertainty_ppm.unwrap() > 0.0);
    assert!(!c.observe(6, 999_999, now + Duration::from_millis(1)));
    assert!(!c.observe(7, 100, now + Duration::from_millis(1)));
    assert!(!c.observe(7, 999_999, anchor));
    assert_eq!(c.estimate(now).sample_count, 1001);
    assert_eq!(
        c.estimate(now + Duration::from_secs(3)).validity,
        Validity::Stale
    );
    assert!(c.adjust(now + Duration::from_secs(3)).is_none());
    let (old, new) = c.adjust(now).unwrap();
    assert_eq!(old - new, 480); // bounded explicit local delay removal
    c.note_loss(480);
    assert!(c.adjust(now + Duration::from_millis(100)).is_none());
    let (_, increased) = c.adjust(now + Duration::from_secs(1)).unwrap();
    assert_eq!(increased, old);
    c.invalidate();
    assert_eq!(c.estimate(now).validity, Validity::Inactive);
    assert_eq!(c.estimate(now).sample_count, 0);
    assert!(c.estimate(now).arrival_rate_error_ppm.is_none());
    // Alternating jitter/loss fixture cannot grow delay or alter source descriptors.
    let mut c = Controller::new(48_000, 480, 8, config(480), 480 * 8).unwrap();
    for n in 0..1000u64 {
        let at = anchor + Duration::from_millis(n * 10 + if n % 2 == 0 { 0 } else { 7 });
        c.observe(8, n * 480, at);
        if n % 20 == 0 {
            c.note_loss(480);
        }
        if let Some((old, new)) = c.adjust(at) {
            assert!(old.abs_diff(new) <= 480);
            assert!((480..=480 * 6).contains(&new));
        }
    }
    assert!(c.estimate(anchor + Duration::from_secs(10)).jitter_ns > 1_000_000.0);
}

#[test]
fn target_changes_count_silence_and_skips_and_never_replay_raw_data() {
    let s = spec(1, "a", Codec::RawF64Le, 1000, &["FR", "FL"], 5);
    let mut rx = Receiver::new(
        permissions("b", "a", &[], std::slice::from_ref(&s)),
        limits(),
    )
    .unwrap();
    rx.admit(offer(s.clone(), 1)).unwrap();
    let anchor = Instant::now();
    let mut p = rx
        .create_playout(
            1,
            PlayoutConfig {
                anchor,
                target_buffer_frames: 0,
            },
        )
        .unwrap();
    for sequence in 0..8 {
        rx.ingest(
            &Frame {
                stream: 1,
                generation: 1,
                sequence,
                sample_timestamp: sequence * 5,
                payload: sample_bytes(&s, sequence as usize),
            }
            .encode(1200)
            .unwrap(),
        );
    }
    rx.service_playout(&mut p).unwrap();
    let obs = rx.arrival_observation(1, 1).unwrap();
    assert_eq!((obs.sequence, obs.count), (7, 8));
    assert_eq!(obs.offer.spec, s);
    // Duplicates cannot refresh age/count.
    rx.ingest(
        &Frame {
            stream: 1,
            generation: 1,
            sequence: 7,
            sample_timestamp: 35,
            payload: sample_bytes(&s, 7),
        }
        .encode(1200)
        .unwrap(),
    );
    assert_eq!(
        rx.arrival_observation(1, 1).unwrap().arrived_at,
        obs.arrived_at
    );
    let mut out = vec![0; s.payload_bytes().unwrap()];
    assert_eq!(p.render_into(anchor, &mut out).unwrap().audio_frames, 5);
    assert_eq!(out, sample_bytes(&s, 0));
    p.set_target_buffer_frames(5).unwrap();
    let report = p
        .render_into(anchor + Duration::from_millis(5), &mut out)
        .unwrap();
    assert_eq!(report.early_frames, 5);
    assert!(out.iter().all(|&x| x == 0));
    p.set_target_buffer_frames(0).unwrap();
    let report = p
        .render_into(anchor + Duration::from_millis(10), &mut out)
        .unwrap();
    assert_eq!((report.skipped_frames, report.audio_frames), (5, 5));
    assert_eq!(out, sample_bytes(&s, 2));
    assert!(p.set_target_buffer_frames(41).is_err());
    rx.admit(offer(s.clone(), 2)).unwrap();
    assert!(rx.arrival_observation(1, 1).is_none());
    assert!(rx.arrival_observation(1, 2).is_none());
}

#[tokio::test]
async fn production_workers_deliver_paced_raw_loopback_and_cancel_old_generation()
-> anyhow::Result<()> {
    let streams = [
        spec(1, "a", Codec::RawF64Le, 1000, &["FR", "FL"], 50),
        spec(
            2,
            "a",
            Codec::RawS24Le,
            1000,
            &["ordered-a", "ordered-b"],
            25,
        ),
    ];
    let (a, b, _ea, _eb) = pair(&streams, &[]).await?;
    let run_a = tokio::spawn(a.clone().run());
    let run_b = tokio::spawn(b.clone().run());
    for s in &streams {
        a.negotiate(offer(s.clone(), 1)).await?;
        for seq in 0..6 {
            a.enqueue(s.stream, sample_bytes(s, seq)).await?;
        }
    }
    let anchor = Instant::now() + Duration::from_millis(30);
    let offers: Vec<_> = streams.iter().map(|s| offer(s.clone(), 1)).collect();
    let sender = SendWorker::new(a.clone(), offers.clone(), anchor).await?;
    let receives = offers
        .iter()
        .map(|o| {
            let mut c = config(o.spec.frames_per_packet);
            c.min_frames = u64::from(o.spec.frames_per_packet) * 2;
            c.initial_frames = c.min_frames;
            c.max_frames = c.min_frames;
            (o.clone(), c)
        })
        .collect();
    let receiver = ReceiveWorker::new(b.clone(), receives, anchor)?;
    let (send_stop, ss) = tokio::sync::watch::channel(false);
    let (recv_stop, rs) = tokio::sync::watch::channel(false);
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    let sender_task = tokio::spawn(sender.run(ss));
    let receiver_task = tokio::spawn(receiver.run(tx, rs));
    let mut delivered = [0u64; 2];
    let mut arrivals = [Vec::new(), Vec::new()];
    let mut retained = None;
    tokio::time::timeout(Duration::from_secs(3), async {
        while delivered.iter().any(|&n| n < 3) {
            let mut block = rx.recv().await.expect("worker output");
            let index = (block.offer.spec.stream - 1) as usize;
            let s = &streams[index];
            if block.playout.audio_frames > 0 {
                assert_eq!(block.playout.audio_frames, u64::from(s.frames_per_packet));
                assert_eq!(block.offer.spec, *s);
                let seq =
                    block.playout.first_sample_frame.unwrap() / u64::from(s.frames_per_packet);
                let mut bytes = vec![0xff; s.payload_bytes().unwrap()];
                assert!(block.copy_into(&mut bytes));
                assert_eq!(bytes, sample_bytes(s, seq as usize));
                delivered[index] += 1;
                arrivals[index].push(Instant::now());
                if index == 0 {
                    retained = Some(block);
                }
            }
        }
    })
    .await?;
    // Actual monotonic wall-time proof: several packets were not flushed as one burst.
    for times in &arrivals {
        assert!(times.last().unwrap().duration_since(times[0]) >= Duration::from_millis(30));
    }
    let latest = b.arrival_observation(1, 1).unwrap();
    assert!(latest.count >= 3);
    assert_eq!(latest.offer.spec, streams[0]);
    // Restart invalidates old output while sibling continues on its independent cadence.
    a.negotiate(offer(streams[0].clone(), 2)).await?;
    let mut stale = retained.take().unwrap();
    let mut bytes = vec![0xff; streams[0].payload_bytes()?];
    assert!(!stale.copy_into(&mut bytes));
    assert!(bytes.iter().all(|&x| x == 0));
    assert!(b.arrival_observation(1, 1).is_none());
    // Receive cancellation drops its exclusive handles and makes queued blocks unreadable.
    recv_stop.send(true)?;
    let received = receiver_task.await??;
    assert!(received.blocks >= 6);
    while let Ok(mut block) = rx.try_recv() {
        let mut bytes = vec![0xff; block.offer.spec.payload_bytes()?];
        assert!(!block.copy_into(&mut bytes));
        assert!(bytes.iter().all(|&x| x == 0));
    }
    // Sender cancellation is generation-scoped; old PCM cannot be replayed.
    let _ = send_stop.send(true);
    let sent = sender_task.await??;
    assert!(sent.sent_packets >= 6);
    assert!(a.enqueue(2, sample_bytes(&streams[1], 0)).await.is_err());
    a.enqueue(1, sample_bytes(&streams[0], 0)).await?; // New generation survives old worker cancellation.
    a.close();
    b.close();
    let _ = run_a.await?;
    let _ = run_b.await?;
    Ok(())
}

#[tokio::test]
async fn aborting_paced_sender_clears_only_owned_generation_without_replay() -> anyhow::Result<()> {
    let s = spec(1, "a", Codec::RawF32Le, 1000, &["mono"], 20);
    let (a, b, _ea, _eb) = pair(std::slice::from_ref(&s), &[]).await?;
    let run_a = tokio::spawn(a.clone().run());
    let run_b = tokio::spawn(b.clone().run());
    a.negotiate(offer(s.clone(), 1)).await?;
    assert!(
        a.flush_due(&[(offer(s.clone(), 1), 0), (offer(s.clone(), 1), 1)])
            .await
            .is_err()
    );
    let worker = SendWorker::new(a.clone(), vec![offer(s.clone(), 1)], Instant::now()).await?;
    let (_stop, stop) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(worker.run(stop));
    // Establish that the worker acquired session ownership before cancellation.
    tokio::task::yield_now().await;
    task.abort();
    let _ = task.await;
    assert!(a.enqueue(1, sample_bytes(&s, 0)).await.is_err());
    a.close();
    b.close();
    let _ = run_a.await?;
    let _ = run_b.await?;
    Ok(())
}

#[tokio::test]
async fn restart_and_cancel_keep_sibling_pacing_and_bounded_output_progress() -> anyhow::Result<()>
{
    let streams = [
        spec(1, "a", Codec::RawS32Le, 1000, &["one"], 20),
        spec(2, "a", Codec::RawF32Le, 1000, &["two"], 20),
    ];
    let (a, b, _ea, _eb) = pair(&streams, &[]).await?;
    let run_a = tokio::spawn(a.clone().run());
    let run_b = tokio::spawn(b.clone().run());
    for s in &streams {
        a.negotiate(offer(s.clone(), 1)).await?;
        for seq in 0..8 {
            a.enqueue(s.stream, sample_bytes(s, seq)).await?;
        }
    }
    let anchor = Instant::now() + Duration::from_millis(20);
    // Two independent owners on one session; cancelling one must not stop the other.
    let mut tasks = Vec::new();
    let mut stops = Vec::new();
    for s in &streams {
        let worker = SendWorker::new(a.clone(), vec![offer(s.clone(), 1)], anchor).await?;
        let (stop, signal) = tokio::sync::watch::channel(false);
        stops.push(stop);
        tasks.push(tokio::spawn(worker.run(signal)));
    }
    let mut c = config(20);
    c.min_frames = 40;
    c.initial_frames = 40;
    c.max_frames = 40;
    let receiver = ReceiveWorker::new(
        b.clone(),
        streams.iter().map(|s| (offer(s.clone(), 1), c)).collect(),
        anchor,
    )?;
    let (stop_rx, signal_rx) = tokio::sync::watch::channel(false);
    let (out, mut received) = tokio::sync::mpsc::channel(1);
    let receiving = tokio::spawn(receiver.run(out, signal_rx));
    let first = tokio::time::timeout(Duration::from_secs(1), received.recv())
        .await?
        .unwrap();
    drop(first);
    // Keep the one-block output full across several actual deadlines. Neither sender waits for it.
    tokio::time::sleep(Duration::from_millis(65)).await;
    let sibling_before = b.arrival_observation(2, 1).unwrap().sequence;
    // Cancelling stream one scrubs it, prevents re-enqueue and leaves stream two live.
    stops[0].send(true)?;
    tasks.remove(0).await??;
    assert!(a.enqueue(1, sample_bytes(&streams[0], 0)).await.is_err());
    a.negotiate(offer(streams[0].clone(), 2)).await?;
    a.enqueue(1, sample_bytes(&streams[0], 0)).await?;
    // A fresh generation receives an explicit new anchor; old worker cancellation cannot clear it.
    let replacement = SendWorker::new(
        a.clone(),
        vec![offer(streams[0].clone(), 2)],
        Instant::now(),
    )
    .await?;
    let (replacement_stop, replacement_signal) = tokio::sync::watch::channel(false);
    let replacement_task = tokio::spawn(replacement.run(replacement_signal));
    tokio::time::sleep(Duration::from_millis(65)).await;
    let sibling_after = b.arrival_observation(2, 1).unwrap();
    assert!(sibling_after.sequence >= sibling_before + 2);
    assert_eq!(sibling_after.offer.generation, 1);
    assert!(b.arrival_observation(1, 1).is_none());
    assert_eq!(b.arrival_observation(1, 2).unwrap().sequence, 0);
    // Legacy round must never steal from a paced sibling.
    assert_eq!(a.flush_round().await?, 0);
    stop_rx.send(true)?;
    let report = receiving.await??;
    assert!(report.output_drops >= 2);
    stops[1].send(true)?;
    tasks.remove(0).await??;
    replacement_stop.send(true)?;
    replacement_task.await??;
    // A canceled generation stays a tombstone: only a higher reliable generation can restart.
    assert!(a.negotiate(offer(streams[0].clone(), 2)).await.is_err());
    a.close();
    b.close();
    let _ = run_a.await?;
    let _ = run_b.await?;
    Ok(())
}

#[test]
fn reordered_raw_packets_and_a_loss_drive_counted_local_delay_increase() {
    let s = spec(1, "a", Codec::RawS24Le, 1000, &["FR", "FL", "aux"], 5);
    let mut rx = Receiver::new(
        permissions("b", "a", &[], std::slice::from_ref(&s)),
        limits(),
    )
    .unwrap();
    rx.admit(offer(s.clone(), 7)).unwrap();
    let anchor = Instant::now();
    let mut p = rx
        .create_playout(
            1,
            PlayoutConfig {
                anchor,
                target_buffer_frames: 0,
            },
        )
        .unwrap();
    // Packet two overtakes zero; packet one is lost. Lower unique positions cannot refresh the trend.
    for sequence in [2, 0] {
        rx.ingest(
            &Frame {
                stream: 1,
                generation: 7,
                sequence,
                sample_timestamp: sequence * 5,
                payload: sample_bytes(&s, sequence as usize),
            }
            .encode(1200)
            .unwrap(),
        );
    }
    let obs = rx.arrival_observation(1, 7).unwrap();
    assert_eq!((obs.sequence, obs.count), (2, 1));
    rx.service_playout(&mut p).unwrap();
    let bytes_per_packet = s.payload_bytes().unwrap();
    let mut bytes = vec![0xff; bytes_per_packet * 3];
    let report = p.render_into(anchor, &mut bytes).unwrap();
    assert_eq!((report.audio_frames, report.missing_frames), (10, 5));
    assert_eq!(&bytes[..bytes_per_packet], sample_bytes(&s, 0));
    assert!(
        bytes[bytes_per_packet..2 * bytes_per_packet]
            .iter()
            .all(|&x| x == 0)
    );
    assert_eq!(&bytes[2 * bytes_per_packet..], sample_bytes(&s, 2));
    let mut controller = Controller::new(1000, 5, 7, config(5), 40).unwrap();
    controller.observe(7, obs.first_frame, obs.arrived_at);
    controller.note_loss(report.missing_frames);
    let (previous, target) = controller.adjust(obs.arrived_at).unwrap();
    assert_eq!(target - previous, 5);
    p.set_target_buffer_frames(target).unwrap();
    let mut silence = vec![0xff; bytes_per_packet];
    let delayed = p
        .render_into(anchor + Duration::from_millis(15), &mut silence)
        .unwrap();
    assert_eq!(delayed.prebuffer_frames, 5);
    assert!(silence.iter().all(|&x| x == 0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_receive_constructor_preserves_new_generation_and_correct_owner()
-> anyhow::Result<()> {
    let s = spec(1, "a", Codec::RawF64Le, 1000, &["FR", "FL"], 20);
    let (a, b, _ea, _eb) = pair(std::slice::from_ref(&s), &[]).await?;
    let run_a = tokio::spawn(a.clone().run());
    let run_b = tokio::spawn(b.clone().run());
    a.negotiate(offer(s.clone(), 1)).await?;
    a.negotiate(offer(s.clone(), 2)).await?;
    a.enqueue(1, sample_bytes(&s, 0)).await?;
    a.flush_round().await?;
    wait_queued(&b, 1, 1).await?;
    let anchor = Instant::now();
    let jitter = config(s.frames_per_packet);
    assert!(ReceiveWorker::new(b.clone(), vec![(offer(s.clone(), 1), jitter)], anchor).is_err());
    let mut wrong = s.clone();
    wrong.channels.reverse();
    assert!(ReceiveWorker::new(b.clone(), vec![(offer(wrong, 2), jitter)], anchor).is_err());
    let mut invalid = jitter;
    invalid.max_frames = 1000;
    assert!(ReceiveWorker::new(b.clone(), vec![(offer(s.clone(), 2), invalid)], anchor).is_err());
    assert!(
        b.arrival_observation(1, 2).is_some(),
        "rejection must preserve current arrival state"
    );
    let correct = ReceiveWorker::new(b.clone(), vec![(offer(s, 2), jitter)], anchor)?;
    // Keep the valid owner alive until after inspection: dropping it deliberately
    // cancels its own generation, which the rejected constructors must not do.
    assert!(b.arrival_observation(1, 2).is_some());
    drop(correct);
    a.close();
    b.close();
    let _ = run_a.await?;
    let _ = run_b.await?;
    Ok(())
}

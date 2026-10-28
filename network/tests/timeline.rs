mod support;
use mixyard_network::{pcm::Outbox, protocol::HEADER, *};
use support::*;

fn frame(s: &Spec, generation: u64, sequence: u64) -> Vec<u8> {
    Frame {
        stream: s.stream,
        generation,
        sequence,
        sample_timestamp: sequence * u64::from(s.frames_per_packet),
        payload: sample_bytes(s, sequence as usize),
    }
    .encode(1200)
    .unwrap()
}

#[test]
fn loss_reorder_duplicates_late_and_restart_are_isolated() -> anyhow::Result<()> {
    let s = spec(1, "a", Codec::RawS16Le, 48_000, &["FL", "FR"], 48);
    let sibling = spec(
        2,
        "a",
        Codec::RawF64Le,
        192_000,
        &["FL", "FR", "FC", "LFE", "SL", "SR"],
        16,
    );
    let mut r = Receiver::new(
        permissions("b", "a", &[], &[s.clone(), sibling.clone()]),
        limits(),
    )?;
    for stream in [&s, &sibling] {
        r.admit(offer(stream.clone(), 1))?;
    }
    r.ingest(&frame(&s, 1, 2)); // out of order, sequence 1 deliberately lost
    r.ingest(&frame(&s, 1, 0));
    r.ingest(&frame(&s, 1, 0));
    r.ingest(&frame(&s, 1, 9)); // outside the bounded window
    for seq in 0..8 {
        r.ingest(&frame(&sibling, 1, seq));
    }
    for seq in 0..8 {
        assert_eq!(r.pull(2)?, sample_bytes(&sibling, seq));
    }
    assert_eq!(r.pull(1)?, sample_bytes(&s, 0));
    assert_eq!(r.pull(1)?, vec![0; 192]);
    assert_eq!(r.pull(1)?, sample_bytes(&s, 2));
    r.ingest(&frame(&s, 1, 1)); // cannot replay a packet after silence was consumed
    assert_eq!(r.counters(1)?.missing_silence, 1);
    assert_eq!(r.counters(1)?.duplicate, 1);
    assert_eq!(r.counters(1)?.late, 1);
    assert_eq!(r.counters(1)?.beyond_window, 1);
    assert_eq!(r.counters(2)?.missing_silence, 0);
    r.ingest(&frame(&s, 1, 3)); // pending payload must be flushed by restart
    r.admit(offer(s.clone(), 2))?;
    r.ingest(&frame(&s, 1, 0)); // stale generation cannot restart implicitly
    assert!(r.admit(offer(s.clone(), 1)).is_err());
    r.ingest(&frame(&s, 2, 0));
    assert_eq!(r.pull(1)?, sample_bytes(&s, 0));
    assert_eq!(r.counters(1)?.stale_generation, 1);
    r.ingest(&frame(&s, 2, 1));
    r.disconnect();
    assert_eq!(r.pull(1)?, vec![0; 192]);
    assert!(r.admit(offer(s, 3)).is_err());
    Ok(())
}

#[test]
fn raw_formats_preserve_precision_rate_and_explicit_channel_order() -> anyhow::Result<()> {
    // Includes odd/non-whitelisted rate and high rates. These are byte transport proofs, not device claims.
    let formats = [
        (Codec::RawS16Le, 44_100),
        (Codec::RawS24Le, 96_000),
        (Codec::RawS32Le, 384_000),
        (Codec::RawF32Le, 12_345),
        (Codec::RawF64Le, 768_000),
    ];
    for (codec, rate) in formats {
        let s = spec(
            7,
            "a",
            codec,
            rate,
            &["FL", "FR", "FC", "LFE", "SL", "SR", "BL", "BR"],
            4,
        );
        let mut budget = limits();
        budget.max_bytes_per_second = 128_000_000;
        let mut r = Receiver::new(permissions("b", "a", &[], std::slice::from_ref(&s)), budget)?;
        r.admit(offer(s.clone(), 1))?;
        r.ingest(&frame(&s, 1, 0));
        assert_eq!(r.pull(7)?, sample_bytes(&s, 0));
        assert_eq!(r.counters(7)?.accepted, 1);
    }
    Ok(())
}

#[test]
fn framing_rejects_malformed_oversize_unknown_and_inconsistent_timeline() -> anyhow::Result<()> {
    let s = spec(1, "a", Codec::RawS24Le, 96_000, &["FL", "FR"], 48);
    let mut r = Receiver::new(
        permissions("b", "a", &[], std::slice::from_ref(&s)),
        limits(),
    )?;
    r.admit(offer(s.clone(), 1))?;
    let valid = frame(&s, 1, 0);
    for length in 0..HEADER {
        r.ingest(&valid[..length]);
    }
    r.ingest(&vec![0; 1201]);
    for index in [0, 5, 7] {
        let mut bad = valid.clone();
        bad[index] ^= 1;
        r.ingest(&bad);
    }
    let mut bad_timestamp = valid.clone();
    bad_timestamp[39] = 1;
    r.ingest(&bad_timestamp);
    let mut unknown = s.clone();
    unknown.stream = 99;
    r.ingest(&frame(&unknown, 1, 0));
    let mut wrong_size = valid.clone();
    wrong_size.pop();
    let size = (wrong_size.len() - HEADER) as u16;
    wrong_size[6..8].copy_from_slice(&size.to_be_bytes());
    r.ingest(&wrong_size);
    assert_eq!(r.queued(1), 0);
    assert_eq!(r.counters(1)?.malformed, 2);
    assert_eq!(r.rejected.unadmitted, 1);
    assert_eq!(r.rejected.malformed, HEADER as u64 + 4);
    Ok(())
}

#[test]
fn admission_rejects_implicit_sources_loops_exclusions_codecs_and_resource_overcommit()
-> anyhow::Result<()> {
    let s = spec(1, "a", Codec::RawF64Le, 192_000, &["FL", "FR"], 16);
    let p = permissions("b", "a", &[], std::slice::from_ref(&s));
    let mut r = Receiver::new(p.clone(), limits())?;
    let mut changed = s.clone();
    changed.name = "Whole system".into();
    assert!(r.admit(offer(changed, 1)).is_err());
    let mut bad_version = offer(s.clone(), 1);
    bad_version.version = 99;
    assert!(r.admit(bad_version).is_err());
    assert!(r.admit(offer(s.clone(), 0)).is_err());
    let mutations: [fn(&mut Spec); 7] = [
        |s| s.codec = Codec::Opus,
        |s| s.provenance.visited_peers.insert(0, "b".into()),
        |s| s.provenance.sources.push(s.provenance.sources[0].clone()),
        |s| s.channels.push("FL".into()),
        |s| s.frames_per_packet = 1000,
        |s| s.sample_rate = 0,
        |s| s.provenance.sources.clear(),
    ];
    for change in mutations {
        let mut bad = s.clone();
        change(&mut bad);
        assert!(Receiver::new(permissions("b", "a", &[], &[bad]), limits()).is_err());
    }
    let mut private = s.clone();
    private.provenance.sources[0]
        .groups
        .push("private-call-return".into());
    let mut forbidden = permissions("b", "a", &[], &[private]);
    forbidden
        .excluded_receive_groups
        .insert("private-call-return".into());
    assert!(Receiver::new(forbidden, limits()).is_err());
    let mut budget = limits();
    budget.max_bytes_per_second = 1;
    assert!(Receiver::new(p.clone(), budget).is_err());
    let mut budget = limits();
    budget.max_streams = 0;
    assert!(Receiver::new(p, budget).is_err());
    // Even one sample frame must fit; no truncation/downmix to make it fit.
    let mut enormous = s;
    enormous.frames_per_packet = 1;
    enormous.channels = (0..151).map(|n| format!("channel-{n}")).collect();
    assert!(
        enormous
            .validate("a", "b", &limits())
            .unwrap_err()
            .to_string()
            .contains("datagram budget")
    );
    Ok(())
}

#[test]
fn sender_rounds_and_overflow_cannot_stall_a_sibling() -> anyhow::Result<()> {
    let a = spec(1, "a", Codec::RawS16Le, 48_000, &["FL", "FR"], 48);
    let b = spec(2, "a", Codec::RawS16Le, 48_000, &["FL", "FR"], 48);
    let mut outbox = Outbox::new(2);
    outbox.admit(offer(a.clone(), 1));
    outbox.admit(offer(b.clone(), 1));
    for i in 0..10 {
        outbox.enqueue(1, sample_bytes(&a, i))?;
    }
    outbox.enqueue(2, sample_bytes(&b, 0))?;
    let round = outbox.round();
    assert_eq!(
        round
            .iter()
            .map(|f| (f.stream, f.sequence))
            .collect::<Vec<_>>(),
        vec![(1, 8), (2, 0)]
    );
    assert_eq!(round[1].payload, sample_bytes(&b, 0));
    assert_eq!(outbox.overflows(1), 8);
    assert_eq!(outbox.round().len(), 1);
    Ok(())
}

#[test]
fn packetizer_preserves_whole_multichannel_frames_across_source_block_boundaries()
-> anyhow::Result<()> {
    let channels: Vec<_> = (0..16).map(|n| format!("ch-{n}")).collect();
    let mut s = spec(1, "a", Codec::RawF64Le, 768_000, &["FL"], 8);
    s.channels = channels;
    let mut budget = limits();
    budget.max_bytes_per_second = 128_000_000;
    s.validate("a", "b", &budget)?;
    let mut outbox = Outbox::new(4);
    outbox.admit(offer(s.clone(), 1));
    let mut receiver = Receiver::new(permissions("b", "a", &[], std::slice::from_ref(&s)), budget)?;
    receiver.admit(offer(s.clone(), 1))?;
    let source = (0..3).flat_map(|n| sample_bytes(&s, n)).collect::<Vec<_>>();
    assert!(outbox.enqueue_pcm(1, &source[..1]).is_err()); // partial sample frame
    assert!(
        outbox
            .enqueue_pcm(1, &vec![0; 5 * s.payload_bytes()?])
            .is_err()
    ); // bounded worker call
    assert_eq!(outbox.enqueue_pcm(1, &source[..384])?, 0); // three of eight sample frames
    assert_eq!(outbox.round().len(), 0); // no invented padding
    assert_eq!(outbox.enqueue_pcm(1, &source[384..])?, 3);
    for n in 0..3 {
        let packets = outbox.round();
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].sample_timestamp, n * 8);
        receiver.ingest(&packets[0].encode(1200)?);
        assert_eq!(receiver.pull(1)?, sample_bytes(&s, n as usize));
    }
    Ok(())
}

#[test]
fn timing_uses_each_negotiated_rate_and_separates_occupancy_holes_partial_and_capacity()
-> anyhow::Result<()> {
    let a = spec(1, "a", Codec::RawS24Le, 96_000, &["FR", "FL"], 48);
    let b = spec(2, "a", Codec::RawF64Le, 192_000, &["FC"], 24);
    let mut receiver = Receiver::new(
        permissions("b", "a", &[], &[a.clone(), b.clone()]),
        limits(),
    )?;
    let mut outbox = mixyard_network::pcm::Outbox::new(8);
    for s in [&a, &b] {
        receiver.admit(offer(s.clone(), 1))?;
        outbox.admit(offer(s.clone(), 1));
        outbox.enqueue(s.stream, sample_bytes(s, 0))?;
        outbox.enqueue(s.stream, sample_bytes(s, 1))?;
        // One sample frame pending, below a full packet, must be reported separately.
        outbox.enqueue_pcm(
            s.stream,
            &sample_bytes(s, 2)[..s.codec.sample_bytes()? * s.channels.len()],
        )?;
        receiver.ingest(&frame(s, 1, 0));
        receiver.ingest(&frame(s, 1, 2));
    }
    let send = outbox.timing_snapshot();
    let receive = receiver.timing_snapshot();
    assert_eq!(send[0].queued_sample_frames, 96);
    assert_eq!(send[0].queued_sample_ms, 1.0);
    assert_eq!(send[0].partial_sample_frames, 1);
    assert_eq!(send[0].partial_sample_ms, 1000.0 / 96_000.0);
    assert_eq!(send[1].queued_sample_frames, 48);
    assert_eq!(send[1].queued_sample_ms, 0.25);
    assert_eq!(send[1].partial_sample_ms, 1000.0 / 192_000.0);
    assert!(send[0].buffered_timeline_ms.is_none());
    assert_eq!(receive[0].queued_sample_ms, 1.0);
    assert_eq!(receive[0].buffered_timeline_sample_frames, Some(144));
    assert_eq!(receive[0].buffered_timeline_ms, Some(1.5));
    assert_eq!(receive[0].capacity_sample_ms, 4.0);
    assert_eq!(receive[1].queued_sample_ms, 0.25);
    assert_eq!(receive[1].buffered_timeline_ms, Some(0.375));
    assert_eq!(receive[1].capacity_sample_ms, 1.0);
    assert!(receive.iter().all(|s| s.consumer_prebuffer_ms.is_none()));
    receiver.pull(1)?;
    let after = receiver.timing_snapshot();
    assert_eq!(after[0].next_sample_frame, Some(48));
    assert_eq!(after[0].queued_sample_ms, 0.5);
    assert_eq!(after[0].buffered_timeline_ms, Some(1.0));
    assert_eq!(after[1].queued_sample_ms, receive[1].queued_sample_ms);
    receiver.admit(offer(a.clone(), 2))?;
    outbox.admit(offer(a, 2));
    let restarted = receiver.timing_snapshot();
    assert_eq!(restarted[0].generation, 2);
    assert_eq!(restarted[0].next_sample_frame, Some(0));
    assert_eq!(restarted[0].queued_sample_frames, 0);
    assert_eq!(restarted[0].buffered_timeline_sample_frames, Some(0));
    assert_eq!(restarted[1].generation, 1);
    assert_eq!(restarted[1].queued_sample_frames, 48);
    assert_eq!(outbox.timing_snapshot()[0].partial_sample_frames, 0);
    assert_eq!(outbox.timing_snapshot()[1].partial_sample_frames, 1);
    receiver.disconnect();
    assert!(
        receiver
            .timing_snapshot()
            .iter()
            .all(|s| s.queued_sample_frames == 0 && s.buffered_timeline_sample_frames == Some(0))
    );
    Ok(())
}

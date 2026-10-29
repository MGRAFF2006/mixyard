mod support;
use mixyard_network::*;
use std::time::{Duration, Instant};
use support::*;

fn receiver(specs: &[Spec]) -> Receiver {
    let mut rx = Receiver::new(permissions("b", "a", &[], specs), limits()).unwrap();
    for s in specs {
        rx.admit(offer(s.clone(), 1)).unwrap();
    }
    rx
}
fn at(anchor: Instant, frame: u64, rate: u32) -> Instant {
    let nanos = (u128::from(frame) * 1_000_000_000).div_ceil(u128::from(rate));
    anchor
        .checked_add(Duration::from_nanos(u64::try_from(nanos).unwrap()))
        .expect("bounded fixture time")
}
fn packet(rx: &mut Receiver, s: &Spec, generation: u64, sequence: u64) {
    rx.ingest(
        &Frame {
            stream: s.stream,
            generation,
            sequence,
            sample_timestamp: sequence * u64::from(s.frames_per_packet),
            payload: sample_bytes(s, sequence as usize),
        }
        .encode(1200)
        .unwrap(),
    );
}
fn make(rx: &mut Receiver, s: &Spec, anchor: Instant, target: u64) -> Playout {
    rx.create_playout(
        s.stream,
        PlayoutConfig {
            anchor,
            target_buffer_frames: target,
        },
    )
    .unwrap()
}

#[test]
fn exact_bits_irregular_rates_ordered_channels_and_packet_splits() {
    let anchor = Instant::now();
    for (codec, rate, channels) in [
        (Codec::RawS16Le, 44_117, 3),
        (Codec::RawS24Le, 96_000, 6),
        (Codec::RawS32Le, 1, 2),
        (Codec::RawF32Le, 384_000, 8),
        (Codec::RawF64Le, 768_000, 16),
        (Codec::RawF64Le, u32::MAX, 1),
    ] {
        let names: Vec<_> = (0..channels).map(|c| format!("ordered-{c}")).collect();
        let refs: Vec<_> = names.iter().map(String::as_str).collect();
        let s = spec(1, "a", codec, rate, &refs, 5);
        let mut budget = limits();
        budget.max_bytes_per_second = u64::MAX;
        let mut rx =
            Receiver::new(permissions("b", "a", &[], std::slice::from_ref(&s)), budget).unwrap();
        rx.admit(offer(s.clone(), 1)).unwrap();
        let mut p = make(&mut rx, &s, anchor, 7);
        assert_eq!(p.spec(), &s); // identity/order/provenance remain available to the consumer.
        for seq in [2, 0, 3, 1] {
            packet(&mut rx, &s, 1, seq);
        }
        rx.service_playout(&mut p).unwrap();
        let expected: Vec<u8> = (0..4).flat_map(|seq| sample_bytes(&s, seq)).collect();
        let frame_bytes = codec.sample_bytes().unwrap() * channels;
        let mut pre = vec![0x55; 4 * frame_bytes];
        assert_eq!(p.render_into(anchor, &mut pre).unwrap().prebuffer_frames, 4);
        assert!(pre.iter().all(|&b| b == 0));
        if rate == u32::MAX {
            // A nanosecond spans >4 frames at this legal rate. Exercise an exact
            // block crossing the target rather than invent sub-ns Instants.
            let mut block = vec![0x55; 27 * frame_bytes];
            let report = p.render_into(anchor, &mut block).unwrap();
            assert_eq!((report.prebuffer_frames, report.audio_frames), (7, 20));
            assert!(block[..7 * frame_bytes].iter().all(|&b| b == 0));
            assert_eq!(&block[7 * frame_bytes..], expected);
            continue;
        }
        let mut cursor = 0;
        for count in [2, 7, 1, 10] {
            let mut block = vec![0x55; count * frame_bytes];
            let report = p
                .render_into(at(anchor, 7 + cursor as u64, rate), &mut block)
                .unwrap();
            assert_eq!(report.audio_frames, count as u64);
            assert_eq!(
                block,
                expected[cursor * frame_bytes..(cursor + count) * frame_bytes]
            );
            cursor += count;
        }
        assert_eq!(p.snapshot().buffered_frames, 0);
        assert_eq!(p.snapshot().counters.audio_frames, 20);
        assert_eq!(p.snapshot().counters.missing_frames, 0);
    }
}

#[test]
fn prebuffer_crossing_overlap_overdue_holes_late_and_window_are_counted() {
    let s = spec(
        1,
        "a",
        Codec::RawS24Le,
        44_117,
        &["R", "L", "private-id"],
        4,
    );
    let mut rx = receiver(std::slice::from_ref(&s));
    let anchor = Instant::now();
    let mut p = make(&mut rx, &s, anchor, 3);
    for seq in [2, 0, 0, 8] {
        packet(&mut rx, &s, 1, seq);
    }
    assert_eq!(rx.counters(1).unwrap().duplicate, 1);
    assert_eq!(rx.counters(1).unwrap().beyond_window, 1);
    rx.service_playout(&mut p).unwrap();
    let width = 9;
    let mut block = vec![0x55; 5 * width];
    let r = p
        .render_into(at(anchor, 1, s.sample_rate), &mut block)
        .unwrap();
    assert_eq!((r.prebuffer_frames, r.audio_frames), (2, 3));
    assert_eq!(&block[..2 * width], &[0; 18]);
    assert_eq!(&block[2 * width..], &sample_bytes(&s, 0)[..3 * width]);
    // Start at sample 2 after sample 2 was already committed: one frame overlap.
    block.resize(4 * width, 0x55);
    let r = p
        .render_into(at(anchor, 5, s.sample_rate), &mut block)
        .unwrap();
    assert_eq!(
        (r.early_frames, r.audio_frames, r.missing_frames),
        (1, 1, 2)
    );
    assert_eq!(&block[..width], &[0; 9]);
    assert_eq!(&block[width..2 * width], &sample_bytes(&s, 0)[3 * width..]);
    assert!(block[2 * width..].iter().all(|&b| b == 0));
    // Packet 1 arrives after two frames of it were replaced by silence. Only
    // its unexpired suffix may be used; packet 0 is wholly late.
    packet(&mut rx, &s, 1, 1);
    packet(&mut rx, &s, 1, 0);
    rx.service_playout(&mut p).unwrap();
    assert_eq!(p.snapshot().counters.late_packets, 1);
    let r = p
        .render_into(at(anchor, 9, s.sample_rate), &mut block)
        .unwrap();
    assert_eq!(r.audio_frames, 4);
    let expected = [
        &sample_bytes(&s, 1)[2 * width..],
        &sample_bytes(&s, 2)[..2 * width],
    ]
    .concat();
    assert_eq!(block, expected);
    // A local pause skips samples 10..14; retained tail of packet 2 is discarded.
    let r = p
        .render_into(at(anchor, 18, s.sample_rate), &mut block)
        .unwrap();
    assert_eq!((r.skipped_frames, r.missing_frames), (5, 4));
    assert!(block.iter().all(|&b| b == 0));
    let c = p.snapshot().counters;
    assert_eq!(
        (
            c.prebuffer_frames,
            c.early_frames,
            c.missing_frames,
            c.skipped_frames
        ),
        (2, 1, 6, 5)
    );
    assert_eq!(c.discarded_buffered_frames, 2);
    assert_eq!(c.late_prefix_frames, 2);
    rx.service_playout(&mut p).unwrap();
    packet(&mut rx, &s, 1, 1);
    assert_eq!(rx.counters(1).unwrap().late, 1);
}

#[test]
fn generation_disconnect_revoke_and_receiver_drop_disable_retained_audio() {
    let specs = [
        spec(1, "a", Codec::RawF64Le, 192_000, &["FL", "FR"], 5),
        spec(2, "a", Codec::RawS16Le, 44_117, &["side"], 7),
        spec(3, "a", Codec::RawF32Le, 96_000, &["C", "L", "R"], 3),
    ];
    let mut rx = receiver(&specs);
    let anchor = Instant::now();
    let mut p: Vec<_> = specs.iter().map(|s| make(&mut rx, s, anchor, 0)).collect();
    for (s, p) in specs.iter().zip(&mut p) {
        packet(&mut rx, s, 1, 0);
        rx.service_playout(p).unwrap();
        assert_eq!(p.snapshot().buffered_frames, u64::from(s.frames_per_packet));
    }
    rx.admit(offer(specs[0].clone(), 2)).unwrap();
    rx.revoke(2).unwrap();
    assert!(rx.admit(offer(specs[1].clone(), 2)).is_err());
    for i in 0..2 {
        let mut out = vec![0xff; specs[i].payload_bytes().unwrap()];
        assert_eq!(
            p[i].render_into(anchor, &mut out).unwrap().inactive_frames,
            u64::from(specs[i].frames_per_packet)
        );
        assert!(out.iter().all(|&b| b == 0));
        assert_eq!(p[i].snapshot().buffered_frames, 0);
    }
    // Restarting one source and revoking another must not change the sibling.
    let mut out = vec![0; specs[2].payload_bytes().unwrap()];
    assert_eq!(p[2].render_into(anchor, &mut out).unwrap().audio_frames, 3);
    assert_eq!(out, sample_bytes(&specs[2], 0));
    let mut restarted = make(&mut rx, &specs[0], anchor, 0);
    packet(&mut rx, &specs[0], 1, 0); // Old generation cannot enter the new handle.
    assert_eq!(rx.counters(1).unwrap().stale_generation, 1);
    packet(&mut rx, &specs[0], 2, 0);
    rx.service_playout(&mut restarted).unwrap();
    rx.disconnect();
    out.resize(specs[0].payload_bytes().unwrap(), 0xff);
    assert_eq!(
        restarted
            .render_into(anchor, &mut out)
            .unwrap()
            .inactive_frames,
        5
    );
    assert!(out.iter().all(|&b| b == 0));
    let mut rx = receiver(&specs);
    let mut last = make(&mut rx, &specs[0], anchor, 0);
    packet(&mut rx, &specs[0], 1, 0);
    rx.service_playout(&mut last).unwrap();
    drop(rx);
    assert_eq!(
        last.render_into(anchor, &mut out).unwrap().inactive_frames,
        5
    );
    assert!(out.iter().all(|&b| b == 0));
}

#[test]
fn admission_and_invalid_blocks_fail_closed_without_consuming_pcm() {
    let s = spec(1, "a", Codec::RawF64Le, 48_000, &["L", "R"], 4);
    let mut rx = receiver(std::slice::from_ref(&s));
    let anchor = Instant::now();
    assert!(
        rx.create_playout(
            1,
            PlayoutConfig {
                anchor,
                target_buffer_frames: 33
            }
        )
        .is_err()
    );
    assert!(
        rx.create_playout(
            2,
            PlayoutConfig {
                anchor,
                target_buffer_frames: 0
            }
        )
        .is_err()
    );
    let mut p = make(&mut rx, &s, anchor, 0);
    assert!(rx.pull(1).is_err());
    assert!(
        rx.create_playout(
            1,
            PlayoutConfig {
                anchor,
                target_buffer_frames: 0
            }
        )
        .is_err()
    );
    let mut other = receiver(std::slice::from_ref(&s));
    assert!(other.service_playout(&mut p).is_err());
    packet(&mut rx, &s, 1, 0);
    rx.service_playout(&mut p).unwrap();
    let mut bad = [0xff; 17];
    assert_eq!(
        p.render_into(anchor, &mut bad),
        Err(PlayoutError::UnalignedBlock)
    );
    assert_eq!(bad, [0; 17]);
    let mut large = vec![0xff; 33 * 16];
    assert_eq!(
        p.render_into(anchor, &mut large),
        Err(PlayoutError::BlockTooLarge)
    );
    assert!(large.iter().all(|&b| b == 0));
    let mut valid = [0xff; 16];
    assert_eq!(p.render_into(anchor, &mut valid).unwrap().audio_frames, 1);
    assert_eq!(&valid, &sample_bytes(&s, 0)[..16]);
    assert_eq!(
        p.render_into(
            anchor
                .checked_sub(Duration::from_nanos(1))
                .expect("short fixture time"),
            &mut valid
        ),
        Err(PlayoutError::TimeWentBackwards)
    );
    assert_eq!(valid, [0; 16]);
    assert_eq!(p.snapshot().next_sample_frame, 1);
    assert_eq!(p.snapshot().counters.rejected_blocks, 3);
    assert_eq!(p.snapshot().counters.rejected_bytes, 17 + 33 * 16 + 16);
}

#[test]
fn sustained_bounded_window_with_reorder_loss_duplicates_and_worker_stalls() {
    let s = spec(1, "a", Codec::RawS32Le, 44_117, &["FR", "FL", "FC"], 5);
    let mut rx = receiver(std::slice::from_ref(&s));
    let anchor = Instant::now();
    let mut p = make(&mut rx, &s, anchor, 0);
    for round in 0..64 {
        let base = round * 8;
        rx.service_playout(&mut p).unwrap();
        for offset in [6, 0, 4, 2, 7, 1, 0] {
            packet(&mut rx, &s, 1, base + offset);
        }
        rx.service_playout(&mut p).unwrap();
        // Duplicate arrives after worker transfer; the adapter also rejects it.
        packet(&mut rx, &s, 1, base);
        rx.service_playout(&mut p).unwrap();
        let mut expected = Vec::new();
        for offset in 0..8 {
            if offset == 3 || offset == 5 {
                expected.extend(vec![0; s.payload_bytes().unwrap()]);
            } else {
                expected.extend(sample_bytes(&s, (base + offset) as usize));
            }
        }
        let mut actual = Vec::new();
        for (start, count) in [(0, 3), (3, 13), (16, 1), (17, 23)] {
            let mut out = vec![0xff; count * 12];
            p.render_into(at(anchor, base * 5 + start, s.sample_rate), &mut out)
                .unwrap();
            actual.extend(out);
            assert!(p.snapshot().buffered_frames <= 40);
            assert!(rx.queued(1) <= 8);
        }
        assert_eq!(actual, expected);
    }
    let c = p.snapshot().counters;
    assert_eq!(c.missing_frames, 64 * 10);
    assert_eq!(c.audio_frames, 64 * 30);
    assert_eq!(c.duplicate_packets, 64);
    assert_eq!(p.snapshot().capacity_payload_bytes, 8 * 5 * 12);
}

#[test]
fn revoke_before_admission_cannot_be_undone_by_later_offer() {
    let s = spec(1, "a", Codec::RawS16Le, 48_000, &["selected"], 4);
    let mut rx = Receiver::new(
        permissions("b", "a", &[], std::slice::from_ref(&s)),
        limits(),
    )
    .unwrap();
    assert_eq!(rx.revoke(1).unwrap().status(), RevocationStatus::Complete);
    assert!(rx.admit(offer(s, 1)).is_err());
    assert!(rx.revoke(999).is_err());
}

#[test]
fn nanosecond_granularity_at_extreme_rate_counts_skip_without_changing_rate() {
    let s = spec(1, "a", Codec::RawS16Le, u32::MAX, &["selected"], 4);
    let mut budget = limits();
    budget.max_bytes_per_second = u64::MAX;
    let mut rx =
        Receiver::new(permissions("b", "a", &[], std::slice::from_ref(&s)), budget).unwrap();
    rx.admit(offer(s.clone(), 1)).unwrap();
    let anchor = Instant::now();
    let mut p = make(&mut rx, &s, anchor, 0);
    packet(&mut rx, &s, 1, 0);
    packet(&mut rx, &s, 1, 1);
    rx.service_playout(&mut p).unwrap();
    let mut out = [0xff; 2];
    assert_eq!(p.render_into(anchor, &mut out).unwrap().audio_frames, 1);
    let r = p
        .render_into(
            anchor.checked_add(Duration::from_nanos(1)).unwrap(),
            &mut out,
        )
        .unwrap();
    assert_eq!(
        (r.first_sample_frame, r.skipped_frames, r.audio_frames),
        (Some(4), 3, 1)
    );
    assert_eq!(out, sample_bytes(&s, 1)[..2]);
    assert_eq!(p.spec().sample_rate, u32::MAX);
    let far = anchor
        .checked_add(Duration::from_secs(u64::MAX / u64::from(u32::MAX) + 1))
        .expect("bounded 137-year fixture time");
    assert_eq!(
        p.render_into(far, &mut out),
        Err(PlayoutError::TimelineExhausted)
    );
    assert_eq!(out, [0; 2]);
    assert_eq!(p.snapshot().next_sample_frame, 5);
}

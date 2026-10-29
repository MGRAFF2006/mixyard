// The same production module also builds without QUIC/serde for a small,
// dependency-free executable proof: rustc --edition=2024 --test this_file.rs.
#[path = "../src/fragmentation.rs"]
mod fragmentation;

use fragmentation::{Config, Descriptor, Error, Fragmenter, HEADER, Outcome, Reassembler};
use std::net::UdpSocket;
use std::time::{Duration, Instant};

fn config() -> Config {
    Config {
        max_block_bytes: 8192,
        max_datagram: 256,
        max_streams: 2,
        window: 3,
        max_buffered_bytes: 100_000,
        timeout: Duration::from_millis(100),
    }
}

fn descriptor(stream: u64, generation: u64, block_bytes: usize) -> Descriptor {
    Descriptor {
        stream,
        generation,
        block_bytes,
        frames_per_block: 1,
    }
}

fn payload(len: usize) -> Vec<u8> {
    (0..len)
        .map(|n| (n.wrapping_mul(73) ^ (n >> 3)) as u8)
        .collect()
}

fn fragments(config: &Config, descriptor: Descriptor, sequence: u64, data: &[u8]) -> Vec<Vec<u8>> {
    Fragmenter::new(config.clone(), descriptor)
        .unwrap()
        .fragments(sequence, data)
        .unwrap()
        .collect()
}

#[test]
fn one_f64_sample_frame_larger_than_mtu_roundtrips_without_conversion() {
    let config = Config {
        max_datagram: 1200,
        ..config()
    };
    // 160 ordered F64 channels, one sample frame. Includes signed zero, NaN
    // payload bits, subnormals, infinities and low bits that Float32 would lose.
    let words: [u64; 8] = [
        0x8000000000000000,
        0x7ff8000000001234,
        1,
        0x3ff0000000000001,
        0x7ff0000000000000,
        0xfff0000000000000,
        0xfff8000000000042,
        0x0010000000000001,
    ];
    let data: Vec<u8> = words
        .iter()
        .cycle()
        .take(160)
        .flat_map(|n| n.to_le_bytes())
        .collect();
    let desc = descriptor(42, 7, data.len());
    let packets = fragments(&config, desc, 0, &data);
    assert_eq!(packets.len(), 2);
    assert!(packets.iter().all(|p| p.len() <= 1200));
    let mut receiver = Reassembler::new(config).unwrap();
    receiver.admit(desc).unwrap();
    let now = Instant::now();
    assert_eq!(receiver.ingest(&packets[1], now), Ok(Outcome::Pending));
    assert_eq!(receiver.take(42, 7, 0, now).unwrap(), None);
    assert_eq!(
        receiver.ingest(&packets[0], now),
        Ok(Outcome::Ready {
            stream: 42,
            generation: 7,
            sequence: 0
        })
    );
    let block = receiver.take(42, 7, 0, now).unwrap().unwrap();
    assert_eq!(
        (
            block.stream,
            block.generation,
            block.sequence,
            block.sample_timestamp
        ),
        (42, 7, 0, 0)
    );
    assert_eq!(block.payload, data);
    assert_eq!(receiver.take(42, 7, 0, now).unwrap(), None);
    assert_eq!(receiver.ingest(&packets[0], now), Ok(Outcome::Duplicate));
    assert_eq!(receiver.buffered_bytes(), 0);
}

#[test]
fn raw_bit_patterns_and_nonstandard_sample_cadence_are_opaque() {
    for sample_bytes in [2, 3, 4, 8] {
        let config = config();
        // 73 frames at an arbitrary positive rate are described by the caller;
        // transport must only preserve bytes and the exact sample-frame offset.
        let desc = Descriptor {
            frames_per_block: 73,
            ..descriptor(9, 11, 73 * 7 * sample_bytes)
        };
        let data = payload(desc.block_bytes);
        let packets = fragments(&config, desc, 2, &data);
        let mut receiver = Reassembler::new(config).unwrap();
        receiver.admit(desc).unwrap();
        let now = Instant::now();
        for packet in packets.iter().rev() {
            receiver.ingest(packet, now).unwrap();
        }
        let block = receiver.take(9, 11, 2, now).unwrap().unwrap();
        assert_eq!(block.sample_timestamp, 146);
        assert_eq!(block.payload, data);
    }
}

#[test]
fn loss_expires_without_partial_output_or_resurrection_and_sibling_progresses() {
    let config = config();
    let desc = descriptor(1, 1, 700);
    let sibling = descriptor(2, 1, 700);
    let data = payload(700);
    let packets = fragments(&config, desc, 0, &data);
    let other = fragments(&config, sibling, 0, &data);
    let mut receiver = Reassembler::new(config.clone()).unwrap();
    receiver.admit(desc).unwrap();
    receiver.admit(sibling).unwrap();
    let now = Instant::now();
    receiver.ingest(&packets[0], now).unwrap();
    assert_eq!(receiver.take(1, 1, 0, now).unwrap(), None);
    let later = now + config.timeout;
    receiver.expire(later).unwrap();
    assert_eq!(receiver.buffered_bytes(), 0);
    for packet in &packets {
        assert_eq!(receiver.ingest(packet, later), Err(Error::Expired));
    }
    for packet in &other {
        receiver.ingest(packet, later).unwrap();
    }
    assert_eq!(
        receiver.take(2, 1, 0, later).unwrap().unwrap().payload,
        data
    );
    receiver.advance(1, 1, 1).unwrap();
    assert_eq!(receiver.ingest(&packets[0], later), Err(Error::Window));
    for packet in fragments(&config, desc, 1, &data) {
        receiver.ingest(&packet, later).unwrap();
    }
    assert_eq!(
        receiver.take(1, 1, 1, later).unwrap().unwrap().payload,
        data
    );
}

#[test]
fn completed_unconsumed_block_has_original_deadline_and_rollback_is_rejected() {
    let config = config();
    let desc = descriptor(1, 1, 400);
    let packets = fragments(&config, desc, 0, &payload(400));
    let mut receiver = Reassembler::new(config.clone()).unwrap();
    receiver.admit(desc).unwrap();
    let now = Instant::now();
    receiver.ingest(&packets[0], now).unwrap();
    let near_deadline = now + config.timeout - Duration::from_nanos(1);
    assert_eq!(
        receiver.ingest(&packets[1], near_deadline),
        Ok(Outcome::Ready {
            stream: 1,
            generation: 1,
            sequence: 0
        })
    );
    assert_eq!(receiver.take(1, 1, 0, now), Err(Error::TimeRollback));
    let deadline = now + config.timeout;
    assert_eq!(receiver.take(1, 1, 0, deadline), Err(Error::Expired));
    assert_eq!(receiver.ingest(&packets[0], deadline), Err(Error::Expired));
    assert_eq!(receiver.buffered_bytes(), 0);
}

#[test]
fn identical_duplicates_do_not_extend_deadline_and_conflict_poison_is_local() {
    let config = config();
    let desc = descriptor(1, 2, 400);
    let packets = fragments(&config, desc, 0, &payload(400));
    let mut receiver = Reassembler::new(config.clone()).unwrap();
    receiver.admit(desc).unwrap();
    let now = Instant::now();
    receiver.ingest(&packets[0], now).unwrap();
    assert_eq!(receiver.ingest(&packets[0], now), Ok(Outcome::Duplicate));
    let mut conflict = packets[0].clone();
    conflict[HEADER] ^= 1;
    assert_eq!(receiver.ingest(&conflict, now), Err(Error::Conflict));
    for packet in &packets {
        assert_eq!(receiver.ingest(packet, now), Err(Error::Conflict));
    }
    assert_eq!(receiver.buffered_bytes(), 0);
    let next = fragments(&config, desc, 1, &payload(400));
    receiver.ingest(&next[0], now).unwrap();
    receiver.ingest(&next[0], now + config.timeout / 2).unwrap();
    assert_eq!(
        receiver.ingest(&next[1], now + config.timeout),
        Err(Error::Expired)
    );
}

#[test]
fn canonical_headers_reject_overlap_truncation_and_untrusted_size_before_allocation() {
    let config = config();
    let desc = descriptor(1, 2, 700);
    let packets = fragments(&config, desc, 0, &payload(700));
    let now = Instant::now();
    let mut receiver = Reassembler::new(config.clone()).unwrap();
    receiver.admit(desc).unwrap();
    for len in 0..packets[0].len() {
        assert!(receiver.ingest(&packets[0][..len], now).is_err());
        assert_eq!(receiver.buffered_bytes(), 0);
    }
    let mut mutations = Vec::new();
    for offset in [0, 4, 6, 32] {
        let mut bad = packets[0].clone();
        bad[offset] ^= 1;
        mutations.push(bad);
    }
    for offset in [1u32, 207, 700, u32::MAX] {
        let mut bad = packets[0].clone();
        bad[44..48].copy_from_slice(&offset.to_be_bytes());
        mutations.push(bad);
    }
    let mut huge = packets[0].clone();
    huge[40..44].copy_from_slice(&u32::MAX.to_be_bytes());
    mutations.push(huge);
    let mut extra = packets[0].clone();
    extra.push(0);
    mutations.push(extra);
    // Length prefix adjusted to a shortened canonical chunk must still fail.
    let mut short = packets[0][..packets[0].len() - 1].to_vec();
    let short_payload_len = (short.len() - HEADER) as u16;
    short[6..8].copy_from_slice(&short_payload_len.to_be_bytes());
    mutations.push(short);
    for bad in mutations {
        assert_eq!(receiver.ingest(&bad, now), Err(Error::Malformed));
        assert_eq!(receiver.buffered_bytes(), 0);
    }
    // Malformed chunks do not damage an otherwise valid admitted block.
    for packet in &packets {
        receiver.ingest(packet, now).unwrap();
    }
    assert_eq!(
        receiver.take(1, 2, 0, now).unwrap().unwrap().payload,
        payload(700)
    );
}

#[test]
fn grants_restart_revoke_and_disconnect_do_not_authorize_datagrams_or_other_streams() {
    let config = config();
    let data = payload(500);
    let old = descriptor(1, 1, 500);
    let fresh = descriptor(1, 2, 500);
    let sibling = descriptor(2, 1, 500);
    let old_packets = fragments(&config, old, 0, &data);
    let fresh_packets = fragments(&config, fresh, 0, &data);
    let mut receiver = Reassembler::new(config.clone()).unwrap();
    let now = Instant::now();
    assert_eq!(
        receiver.ingest(&old_packets[0], now),
        Err(Error::Unadmitted)
    );
    assert_eq!(receiver.buffered_bytes(), 0);
    receiver.admit(old).unwrap();
    receiver.admit(sibling).unwrap();
    receiver.ingest(&old_packets[0], now).unwrap();
    receiver.admit(fresh).unwrap();
    assert_eq!(receiver.buffered_bytes(), 0);
    assert_eq!(receiver.admit(old), Err(Error::Generation));
    assert_eq!(
        receiver.ingest(&old_packets[0], now),
        Err(Error::Generation)
    );
    for packet in &fresh_packets {
        receiver.ingest(packet, now).unwrap();
    }
    receiver.revoke(1).unwrap();
    receiver.revoke(1).unwrap();
    assert_eq!(receiver.take(1, 2, 0, now), Err(Error::Revoked));
    assert_eq!(receiver.ingest(&fresh_packets[0], now), Err(Error::Revoked));
    assert_eq!(receiver.admit(descriptor(1, 3, 500)), Err(Error::Revoked));
    // Revoked identities occupy bounded tombstone slots.
    assert_eq!(receiver.admit(descriptor(3, 1, 500)), Err(Error::Budget));
    for packet in fragments(&config, sibling, 0, &data) {
        receiver.ingest(&packet, now).unwrap();
    }
    assert_eq!(receiver.take(2, 1, 0, now).unwrap().unwrap().payload, data);
    receiver.disconnect();
    assert_eq!(receiver.buffered_bytes(), 0);
    assert_eq!(receiver.reserved_bytes(), 0);
    assert_eq!(
        receiver.ingest(&fresh_packets[0], now),
        Err(Error::Disconnected)
    );
    assert_eq!(receiver.admit(fresh), Err(Error::Disconnected));
}

#[test]
fn aggregate_admission_reserves_sibling_capacity_and_window_is_consumer_owned() {
    // 400 payload + 2 presence bytes, times three blocks, times two streams.
    let config = Config {
        max_buffered_bytes: 2412,
        ..config()
    };
    let a = descriptor(1, 1, 400);
    let b = descriptor(2, 1, 400);
    let data = payload(400);
    let mut receiver = Reassembler::new(config.clone()).unwrap();
    receiver.admit(a).unwrap();
    assert_eq!(receiver.admit(descriptor(2, 1, 401)), Err(Error::Budget));
    receiver.admit(b).unwrap();
    assert_eq!(receiver.reserved_bytes(), 2412);
    let now = Instant::now();
    for desc in [a, b] {
        for sequence in 0..3 {
            for packet in fragments(&config, desc, sequence, &data) {
                receiver.ingest(&packet, now).unwrap();
            }
        }
    }
    assert_eq!(receiver.buffered_bytes(), 2412);
    let future = fragments(&config, a, 3, &data);
    assert_eq!(receiver.ingest(&future[0], now), Err(Error::Window));
    assert_eq!(receiver.buffered_bytes(), 2412);
    for sequence in 0..3 {
        assert_eq!(
            receiver.take(2, 1, sequence, now).unwrap().unwrap().payload,
            data
        );
    }
    receiver.advance(1, 1, 3).unwrap();
    assert_eq!(receiver.advance(1, 1, 2), Err(Error::Window));
    assert_eq!(receiver.advance(1, 2, 3), Err(Error::Generation));
    for packet in future {
        receiver.ingest(&packet, now).unwrap();
    }
    assert_eq!(receiver.take(1, 1, 3, now).unwrap().unwrap().payload, data);
}

#[test]
fn invalid_configuration_descriptor_and_sequence_never_panic_or_allocate() {
    for config in [
        Config {
            max_datagram: HEADER,
            ..config()
        },
        Config {
            max_datagram: 1201,
            ..config()
        },
        Config {
            window: 0,
            ..config()
        },
        Config {
            timeout: Duration::ZERO,
            ..config()
        },
        Config {
            max_streams: usize::MAX,
            window: 2,
            ..config()
        },
    ] {
        assert!(matches!(
            Reassembler::new(config),
            Err(Error::InvalidConfig)
        ));
    }
    let config = config();
    let mut receiver = Reassembler::new(config.clone()).unwrap();
    for desc in [
        descriptor(0, 1, 20),
        descriptor(1, 0, 20),
        descriptor(1, 1, 0),
        descriptor(1, 1, 8193),
        Descriptor {
            frames_per_block: 0,
            ..descriptor(1, 1, 20)
        },
    ] {
        assert_eq!(receiver.admit(desc), Err(Error::InvalidDescriptor));
    }
    let sender = Fragmenter::new(
        config,
        Descriptor {
            frames_per_block: 2,
            ..descriptor(1, 1, 20)
        },
    )
    .unwrap();
    assert!(matches!(
        sender.fragments(0, &[0; 19]),
        Err(Error::InvalidDescriptor)
    ));
    assert!(matches!(
        sender.fragments(u64::MAX, &[0; 20]),
        Err(Error::SequenceExhausted)
    ));
    assert_eq!(receiver.buffered_bytes(), 0);
    assert_eq!(receiver.reserved_bytes(), 0);
}

#[test]
fn canonical_tail_boundaries_and_single_byte_fragments_roundtrip() {
    for capacity in [1, 2, 208, 1152] {
        for len in [1, capacity, capacity + 1, capacity * 2] {
            let config = Config {
                max_datagram: HEADER + capacity,
                ..config()
            };
            let desc = descriptor(1, 1, len);
            let data = payload(len);
            let packets = fragments(&config, desc, 0, &data);
            let mut receiver = Reassembler::new(config.clone()).unwrap();
            receiver.admit(desc).unwrap();
            let now = Instant::now();
            for packet in packets.iter().rev() {
                assert!(packet.len() <= config.max_datagram);
                receiver.ingest(packet, now).unwrap();
            }
            assert_eq!(receiver.take(1, 1, 0, now).unwrap().unwrap().payload, data);
        }
    }
}

#[test]
fn failed_generation_admission_preserves_previous_grant_and_complete_block() {
    let config = Config {
        max_buffered_bytes: 2412,
        ..config()
    };
    let a = descriptor(1, 4, 400);
    let b = descriptor(2, 1, 400);
    let mut receiver = Reassembler::new(config.clone()).unwrap();
    receiver.admit(a).unwrap();
    receiver.admit(b).unwrap();
    let now = Instant::now();
    for packet in fragments(&config, a, 0, &payload(400)) {
        receiver.ingest(&packet, now).unwrap();
    }
    assert_eq!(receiver.admit(descriptor(1, 5, 401)), Err(Error::Budget));
    assert_eq!(receiver.reserved_bytes(), 2412);
    assert_eq!(
        receiver.take(1, 4, 0, now).unwrap().unwrap().payload,
        payload(400)
    );
    let future = fragments(&config, descriptor(1, 5, 400), 0, &payload(400));
    assert_eq!(receiver.ingest(&future[0], now), Err(Error::Generation));
    for packet in fragments(&config, b, 0, &payload(400)) {
        receiver.ingest(&packet, now).unwrap();
    }
    assert_eq!(
        receiver.take(2, 1, 0, now).unwrap().unwrap().payload,
        payload(400)
    );
}

#[test]
fn conflicting_complete_block_is_never_released_and_overflow_timestamp_is_rejected() {
    let config = config();
    let desc = Descriptor {
        frames_per_block: 2,
        ..descriptor(1, 1, 400)
    };
    let packets = fragments(&config, desc, 0, &payload(400));
    let mut receiver = Reassembler::new(config).unwrap();
    receiver.admit(desc).unwrap();
    let now = Instant::now();
    for packet in &packets {
        receiver.ingest(packet, now).unwrap();
    }
    let mut conflict = packets[1].clone();
    conflict[HEADER] ^= 1;
    assert_eq!(receiver.ingest(&conflict, now), Err(Error::Conflict));
    assert_eq!(receiver.take(1, 1, 0, now), Err(Error::Conflict));
    assert_eq!(receiver.buffered_bytes(), 0);
    receiver.advance(1, 1, u64::MAX).unwrap();
    let mut overflow = packets[0].clone();
    overflow[24..32].copy_from_slice(&u64::MAX.to_be_bytes());
    overflow[32..40].copy_from_slice(&(u64::MAX - 1).to_be_bytes());
    assert_eq!(receiver.ingest(&overflow, now), Err(Error::Malformed));
    assert_eq!(receiver.buffered_bytes(), 0);
}

#[test]
fn real_udp_loopback_reassembles_reordered_fragments_with_admission_and_duplicate() {
    // UDP only proves datagram IO/framing. Production caller uses authenticated
    // QUIC and reliable exact grants; this test does not claim UDP authentication.
    let config = Config {
        timeout: Duration::from_secs(5),
        ..config()
    };
    let desc = descriptor(17, 3, 1280);
    let data = payload(1280);
    let packets = fragments(&config, desc, 0, &data);
    let tx = UdpSocket::bind("127.0.0.1:0").unwrap();
    let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
    rx.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    tx.connect(rx.local_addr().unwrap()).unwrap();
    rx.connect(tx.local_addr().unwrap()).unwrap();
    for packet in packets.iter().rev().chain(packets.last()) {
        assert_eq!(tx.send(packet).unwrap(), packet.len());
    }
    let mut receiver = Reassembler::new(config).unwrap();
    receiver.admit(desc).unwrap();
    let mut buffer = [0; 1201];
    let mut ready = 0;
    let mut duplicates = 0;
    for _ in 0..packets.len() + 1 {
        let len = rx.recv(&mut buffer).unwrap();
        match receiver.ingest(&buffer[..len], Instant::now()).unwrap() {
            Outcome::Ready {
                stream,
                generation,
                sequence,
            } => {
                assert_eq!((stream, generation, sequence), (17, 3, 0));
                ready += 1;
            }
            Outcome::Duplicate => duplicates += 1,
            Outcome::Pending => (),
        }
    }
    assert_eq!((ready, duplicates), (1, 1));
    assert_eq!(
        receiver
            .take(17, 3, 0, Instant::now())
            .unwrap()
            .unwrap()
            .payload,
        data
    );
}

#[test]
fn variable_codec_geometry_reserves_maximum_and_rejects_length_switches_and_raw_relabeling() {
    let config = Config {
        max_block_bytes: 4096,
        max_datagram: 128,
        max_streams: 2,
        window: 2,
        max_buffered_bytes: 20_000,
        timeout: Duration::from_millis(50),
    };
    let d = Descriptor {
        stream: 1,
        generation: 1,
        block_bytes: 4096,
        frames_per_block: 960,
    };
    let mut receiver = Reassembler::new(config.clone()).unwrap();
    receiver.admit_variable(d).unwrap();
    let reserved = receiver.reserved_bytes();
    assert!(
        reserved >= 8192,
        "a tiny first packet cannot under-reserve its maximum window"
    );
    let sender = Fragmenter::variable(config.clone(), d).unwrap();
    let now = Instant::now();
    for (sequence, length) in [(0, 181), (1, 33)] {
        let expected: Vec<u8> = (0..length).map(|i| (i % 251) as u8).collect();
        let fragments: Vec<_> = sender.fragments(sequence, &expected).unwrap().collect();
        for fragment in fragments.iter().rev() {
            receiver.ingest(fragment, now).unwrap();
        }
        let actual = receiver.take(1, 1, sequence, now).unwrap().unwrap();
        assert_eq!(actual.payload, expected);
        assert_eq!(actual.sample_timestamp, sequence * 960);
        assert_eq!(receiver.reserved_bytes(), reserved);
    }
    let mut raw = Reassembler::new(config.clone()).unwrap();
    raw.admit(d).unwrap();
    let envelope = vec![7; 4096];
    let fragment = sender.fragments(0, &envelope).unwrap().next().unwrap();
    assert_eq!(
        raw.ingest(&fragment, now),
        Err(Error::Malformed),
        "MXAFv3 cannot masquerade as exact raw v2"
    );
    let mut lengths = Reassembler::new(config.clone()).unwrap();
    lengths.admit_variable(d).unwrap();
    let one = vec![1; 181];
    let two = vec![1; 100];
    let first = sender.fragments(0, &one).unwrap().next().unwrap();
    lengths.ingest(&first, now).unwrap();
    let switched = sender.fragments(0, &two).unwrap().nth(1).unwrap();
    assert_eq!(lengths.ingest(&switched, now), Err(Error::Conflict));
    assert_eq!(lengths.take(1, 1, 0, now), Err(Error::Conflict));
    let too_large = vec![0; 4097];
    assert!(sender.fragments(2, &too_large).is_err());
    let mut bomb = first;
    bomb[40..44].copy_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(lengths.ingest(&bomb, now), Err(Error::Malformed));
    assert!(lengths.buffered_bytes() <= lengths.reserved_bytes());
}

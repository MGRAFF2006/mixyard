use super::*;

fn format(sample: Sample) -> Format {
    Format {
        rate: 44117,
        sample,
        channels: vec!["FR".into(), "FL".into(), "AUX".into()],
        frames: 128,
    }
}
fn binding() -> Binding {
    Binding {
        connection: [7; 16],
        stream: 3,
        generation: 1,
    }
}
fn budget() -> Budget {
    Budget {
        max_block_bytes: MAX_BLOCK_BYTES,
        max_encoded_bytes: MAX_ENCODED_BYTES,
    }
}
fn selected(format: Format, encoding: Encoding) -> Negotiated {
    let p = Plan { format, encoding };
    negotiate(
        &p,
        std::slice::from_ref(&p),
        std::slice::from_ref(&p),
        true,
        budget(),
    )
    .unwrap()
}
fn bits(sample: Sample, len: usize) -> Vec<u8> {
    // Independent words include integer low bits, negatives, signed zero, signaling
    // and quiet NaNs with payloads, infinities and subnormals. Never float arithmetic.
    let words: Vec<u8> = match sample {
        Sample::S16Le => [0i16, 1, -1, i16::MIN, i16::MAX]
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect(),
        Sample::S24Le => vec![
            0, 0, 0, 1, 0, 0, 0xff, 0xff, 0xff, 0, 0, 0x80, 0xff, 0xff, 0x7f,
        ],
        Sample::S32Le => [0i32, 1, -1, i32::MIN, i32::MAX, 0x12345679]
            .into_iter()
            .flat_map(i32::to_le_bytes)
            .collect(),
        Sample::F32Le => [
            0u32, 0x80000000, 0x7fc01234, 0x7f800001, 0x7f800000, 0xff800000, 1,
        ]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect(),
        Sample::F64Le => [
            0u64,
            0x8000000000000000,
            0x7ff8000000001234,
            0x7ff0000000000001,
            0x7ff0000000000000,
            0xfff0000000000000,
            1,
        ]
        .into_iter()
        .flat_map(u64::to_le_bytes)
        .collect(),
    };
    words.into_iter().cycle().take(len).collect()
}

#[test]
fn exact_opaque_envelopes_all_raw_formats_and_lifecycles() {
    let mut encodings = vec![Encoding::Raw];
    if cfg!(feature = "system-zstd") {
        encodings.push(Encoding::Zstd { level: 3 });
    }
    for encoding in encodings {
        for sample in [
            Sample::S16Le,
            Sample::S24Le,
            Sample::S32Le,
            Sample::F32Le,
            Sample::F64Le,
        ] {
            let n = selected(format(sample), encoding);
            let canonical = (
                "original/source",
                vec!["private-excluded-already", "peer/a", "peer/b"],
            );
            let mut enc = Encoder::new(canonical.clone(), n.clone(), binding()).unwrap();
            let mut dec = Decoder::new(canonical.clone(), n.clone(), binding()).unwrap();
            let raw = bits(sample, n.block_bytes());
            let p0 = enc.encode(&raw).unwrap();
            let p1 = enc.encode(&raw).unwrap();
            // Stateless raw/Zstd can decode after bounded transport reorder admission.
            assert_eq!(dec.decode(1, &p1).unwrap(), raw);
            assert_eq!(dec.decode(0, &p0).unwrap(), raw);
            assert_eq!(dec.canonical(), &canonical);
            assert_eq!(dec.negotiated().plan(), n.plan());
            assert_eq!(dec.metrics().blocks, 2);
            assert!(dec.decode(4, &p0).is_err());
            for field in [4, 5, 8, 24, 32, 48, 52] {
                let mut bad = p0.clone();
                bad[field] ^= 1;
                assert!(dec.decode(0, &bad).is_err(), "field {field}");
            }
            let mut new_binding = binding();
            new_binding.generation = 2;
            let mut newer = Decoder::new(canonical.clone(), n.clone(), new_binding).unwrap();
            assert!(newer.decode(0, &p0).is_err());
            new_binding.connection[0] ^= 1;
            let mut reconnected = Decoder::new(canonical, n, new_binding).unwrap();
            assert!(reconnected.decode(0, &p0).is_err());
            dec.revocation().revoke();
            enc.revocation().revoke();
            assert_eq!(dec.decode(0, &p0), Err(Error::Inactive));
            assert_eq!(enc.encode(&raw), Err(Error::Inactive));
        }
    }
}

#[test]
fn selection_is_exact_explicit_and_destination_local() {
    let p = Plan {
        format: format(Sample::F64Le),
        encoding: Encoding::Raw,
    };
    let mut mismatch = p.clone();
    mismatch.format.channels.swap(0, 1);
    assert!(negotiate(&p, &[p.clone()], &[mismatch], false, budget()).is_err());
    let mut mismatch = p.clone();
    mismatch.format.rate = 48000;
    assert!(negotiate(&p, &[p.clone()], &[mismatch], false, budget()).is_err());
    assert!(
        negotiate(
            &p,
            &vec![p.clone(); MAX_PLANS + 1],
            &[p.clone()],
            false,
            budget()
        )
        .is_err()
    );
    let mut huge = p.clone();
    huge.format.frames = u32::MAX;
    assert!(negotiate(&huge, &[huge.clone()], &[huge.clone()], false, budget()).is_err());
    for rate in [1, 44117, 192000, u32::MAX] {
        let mut p = p.clone();
        p.format.rate = rate;
        assert_eq!(
            negotiate(&p, &[p.clone()], &[p.clone()], false, budget())
                .unwrap()
                .plan()
                .format
                .rate,
            rate
        );
    }
    let mut z = p.clone();
    z.encoding = Encoding::Zstd { level: 22 };
    assert!(negotiate(&z, &[z.clone()], &[z.clone()], false, budget()).is_err());
    let mut lossy = p.clone();
    lossy.encoding = Encoding::Opus(opus_parameters());
    assert!(negotiate(&lossy, &[lossy.clone()], &[lossy.clone()], false, budget()).is_err());
    // The independent sibling retains its exact raw source descriptor.
    assert_eq!(selected(p.format.clone(), Encoding::Raw).plan(), &p);
    if !cfg!(feature = "system-zstd") {
        z.encoding = Encoding::Zstd { level: 1 };
        assert_eq!(
            negotiate(&z, &[z.clone()], &[z.clone()], false, budget()).unwrap_err(),
            Error::Unavailable("system-zstd")
        );
    }
}

#[cfg(feature = "system-zstd")]
#[test]
fn zstd_rejects_expansion_unknown_sizes_windows_dictionaries_and_trailing_frames() {
    let n = selected(format(Sample::F64Le), Encoding::Zstd { level: 1 });
    let raw = vec![0; n.block_bytes()];
    let mut encoder = Encoder::new((), n.clone(), binding()).unwrap();
    let p = encoder.encode(&raw).unwrap();
    assert!(p.len() < raw.len());
    let payload = &p[HEADER_BYTES..];
    let mut engine = zstd::Engine::new(1).unwrap();
    assert!(engine.decode(payload, MAX_BLOCK_BYTES).is_err()); // forged expansion request
    for bad in [
        {
            let mut x = payload.to_vec();
            x[4] |= 1;
            x
        }, // dictionary ID
        {
            let mut x = payload.to_vec();
            x[4] &= !4;
            x
        }, // no checksum
        {
            let mut x = payload.to_vec();
            x[4] |= 8;
            x
        }, // reserved bit
        {
            let mut x = payload.to_vec();
            x.push(0);
            x
        },
        [payload, payload].concat(),
        payload[..payload.len() - 1].to_vec(),
        {
            let mut x = payload.to_vec();
            let last = x.len() - 1;
            x[last] ^= 1;
            x
        },
        // Non-single-segment with unknown content size and huge window.
        vec![0x28, 0xb5, 0x2f, 0xfd, 4, 0xff, 1, 0, 0, 0, 0, 0, 0],
        // Known content size 3072, oversized non-single-segment window.
        vec![
            0x28, 0xb5, 0x2f, 0xfd, 0x44, 0xff, 0, 11, 1, 0, 0, 0, 0, 0, 0,
        ],
    ] {
        assert!(engine.decode(&bad, n.block_bytes()).is_err());
    }
    let mut decoder = Decoder::new((), n.clone(), binding()).unwrap();
    assert!(
        decoder
            .decode(0, &vec![0; HEADER_BYTES + MAX_ENCODED_BYTES + 1])
            .is_err()
    );
    // Uncompressible input may expand: fail an explicit tight budget, never switch codec.
    let small = Budget {
        max_block_bytes: MAX_BLOCK_BYTES,
        max_encoded_bytes: 1,
    };
    let p = n.plan();
    let constrained = negotiate(
        p,
        std::slice::from_ref(p),
        std::slice::from_ref(p),
        false,
        small,
    )
    .unwrap();
    assert!(
        Encoder::new((), constrained, binding())
            .unwrap()
            .encode(&raw)
            .is_err()
    );
    // Largest admitted block, including high precision ordered channels.
    let f = Format {
        rate: 192000,
        sample: Sample::F64Le,
        channels: (0..128).map(|c| format!("C{c}")).collect(),
        frames: 256,
    };
    let n = selected(f, Encoding::Zstd { level: 3 });
    let raw = bits(Sample::F64Le, n.block_bytes());
    let p = Encoder::new((), n.clone(), binding())
        .unwrap()
        .encode(&raw)
        .unwrap();
    assert_eq!(
        Decoder::new((), n, binding())
            .unwrap()
            .decode(0, &p)
            .unwrap(),
        raw
    );
}

#[cfg(feature = "system-zstd")]
#[test]
fn lossless_entropy_expansion_and_sibling_streams_remain_independent() {
    let mut streams = Vec::new();
    for (stream, sample, rate, channels) in [
        (1, Sample::S24Le, 96000, 6),
        (2, Sample::S32Le, 192000, 16),
        (3, Sample::F64Le, 44117, 4),
    ] {
        let n = selected(
            Format {
                sample,
                rate,
                channels: (0..channels).map(|c| format!("C{c}")).collect(),
                frames: 256,
            },
            Encoding::Zstd { level: 3 },
        );
        let binding = Binding {
            stream,
            ..binding()
        };
        let mut state = 0xdead_beef_1234_5678u64;
        let raw = (0..n.block_bytes())
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect::<Vec<_>>();
        let mut enc = Encoder::new(n.plan().format.clone(), n.clone(), binding).unwrap();
        let mut dec = Decoder::new(n.plan().format.clone(), n, binding).unwrap();
        let packet = enc.encode(&raw).unwrap();
        // This deliberately incompressible fixture expands; the codec tag remains
        // explicitly Zstd, rather than silently substituting a raw/Opus payload.
        assert!(packet.len() > raw.len() + HEADER_BYTES);
        assert_eq!(packet[4], 1);
        assert_eq!(dec.decode(0, &packet).unwrap(), raw);
        streams.push((enc, dec, raw, packet));
    }
    streams[0].1.revocation().revoke();
    let (_, dec, _, packet) = &mut streams[0];
    assert_eq!(dec.decode(0, packet), Err(Error::Inactive));
    for (enc, dec, raw, old) in &mut streams[1..] {
        let mut wrong_stream = old.clone();
        wrong_stream[31] = 1;
        assert!(dec.decode(0, &wrong_stream).is_err());
        let next = enc.encode(raw).unwrap();
        assert_eq!(dec.decode(1, &next).unwrap(), *raw);
        assert!(dec.revocation().active());
    }
}

#[cfg(feature = "system-opus")]
#[test]
fn opus_long_high_bitrate_packet_is_not_silently_capped_to_1275_bytes() {
    let f = Format {
        rate: 48000,
        sample: Sample::F32Le,
        channels: vec!["FL".into(), "FR".into()],
        frames: 2880,
    };
    let p = OpusParameters {
        bitrate: 512000,
        complexity: 10,
        vbr: false,
        conversion: Conversion::NormalizedF32Clamp,
    };
    let n = selected(f, Encoding::Opus(p));
    let mut encoder = Encoder::new((), n.clone(), binding()).unwrap();
    let mut decoder = Decoder::new((), n.clone(), binding()).unwrap();
    let packet = encoder.encode(&vec![0; n.block_bytes()]).unwrap();
    assert!(packet.len() - HEADER_BYTES > 1275);
    assert!(packet.len() - HEADER_BYTES <= OPUS_MAX_PACKET_BYTES);
    assert_eq!(decoder.decode(0, &packet).unwrap().len(), n.block_bytes());
    let mut mismatch = n.plan().clone();
    if let Encoding::Opus(ref mut parameters) = mismatch.encoding {
        parameters.bitrate = 64000;
    }
    assert!(
        negotiate(
            n.plan(),
            std::slice::from_ref(n.plan()),
            &[mismatch],
            true,
            budget()
        )
        .is_err()
    );
}

#[cfg(feature = "system-opus")]
#[test]
fn opus_parameter_extremes_are_exact_or_reject_before_admission() {
    for channels in [vec!["M".into()], vec!["FL".into(), "FR".into()]] {
        for rate in [8000, 48000] {
            for bitrate in [6000, 512000] {
                for complexity in [0, 10] {
                    for vbr in [false, true] {
                        let f = Format {
                            rate,
                            sample: Sample::F32Le,
                            channels: channels.clone(),
                            frames: rate / 100,
                        };
                        let n = selected(
                            f,
                            Encoding::Opus(OpusParameters {
                                bitrate,
                                complexity,
                                vbr,
                                conversion: Conversion::NormalizedF32Clamp,
                            }),
                        );
                        match Encoder::new((), n.clone(), binding()) {
                            Ok(mut encoder) => {
                                let packet = encoder.encode(&vec![0; n.block_bytes()]).unwrap();
                                let mut decoder = Decoder::new((), n.clone(), binding()).unwrap();
                                assert_eq!(
                                    decoder.decode(0, &packet).unwrap().len(),
                                    n.block_bytes()
                                );
                            }
                            Err(error) => assert_eq!(
                                error,
                                Error::Invalid("Opus library changed requested target bitrate")
                            ),
                        }
                    }
                }
            }
        }
    }
}

fn opus_parameters() -> OpusParameters {
    OpusParameters {
        bitrate: 64000,
        complexity: 5,
        vbr: true,
        conversion: Conversion::NormalizedF32Clamp,
    }
}
#[cfg(feature = "system-opus")]
#[test]
fn opus_runs_all_rates_durations_types_and_both_layouts_with_explicit_loss() {
    for rate in [8000, 12000, 16000, 24000, 48000] {
        for duration in [1, 2, 4, 8, 16, 24] {
            for channels in [vec!["M".into()], vec!["FL".into(), "FR".into()]] {
                for sample in [
                    Sample::S16Le,
                    Sample::S24Le,
                    Sample::S32Le,
                    Sample::F32Le,
                    Sample::F64Le,
                ] {
                    let f = Format {
                        rate,
                        sample,
                        channels: channels.clone(),
                        frames: rate * duration / 400,
                    };
                    let n = selected(f, Encoding::Opus(opus_parameters()));
                    let mut encoder = Encoder::new("original", n.clone(), binding()).unwrap();
                    let mut decoder = Decoder::new("original", n.clone(), binding()).unwrap();
                    assert!(encoder.lookahead_frames() > 0);
                    let raw =
                        opus::from_f32(sample, &vec![0.125; n.block_bytes() / sample.bytes()])
                            .unwrap();
                    let p = encoder.encode(&raw).unwrap();
                    let decoded = decoder.decode(0, &p).unwrap();
                    assert_eq!(decoded.len(), raw.len());
                    assert!(
                        opus::to_f32(sample, &decoded)
                            .unwrap()
                            .iter()
                            .all(|s| s.is_finite())
                    );
                    assert_ne!(decoded, raw); // lookahead + quantization are actual lossy behavior
                    assert!(decoder.decode(0, &p).is_err());
                    let p1 = encoder.encode(&raw).unwrap();
                    assert!(decoder.decode(2, &p1).is_err());
                    assert!(decoder.decode(1, &p1).is_ok());
                }
            }
        }
    }
}

#[cfg(feature = "system-opus")]
#[test]
fn opus_conversions_have_independent_integer_extreme_oracles_and_reject_nonfinite() {
    for (sample, input, expected) in [
        (
            Sample::S16Le,
            vec![0, 0x80, 0xff, 0x7f],
            vec![-1.0, 32767.0 / 32768.0],
        ),
        (
            Sample::S24Le,
            vec![0, 0, 0x80, 0xff, 0xff, 0x7f],
            vec![-1.0, 8388607.0 / 8388608.0],
        ),
        (
            Sample::S32Le,
            vec![0, 0, 0, 0x80, 0xff, 0xff, 0xff, 0x7f],
            vec![-1.0, 1.0],
        ),
    ] {
        assert_eq!(opus::to_f32(sample, &input).unwrap(), expected);
        assert_eq!(opus::from_f32(sample, &[-1.0, 1.0]).unwrap(), input);
    }
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1.01] {
        let mut f = format(Sample::F64Le);
        f.rate = 48000;
        f.channels = vec!["M".into()];
        f.frames = 480;
        let n = selected(f, Encoding::Opus(opus_parameters()));
        let input = value
            .to_le_bytes()
            .into_iter()
            .cycle()
            .take(n.block_bytes())
            .collect::<Vec<_>>();
        let mut encoder = Encoder::new((), n, binding()).unwrap();
        assert!(encoder.encode(&input).is_err());
        assert!(!encoder.revocation().active());
    }
    for (rate, channels, frames) in [
        (44100, vec!["M".into()], 441),
        (48000, vec!["FR".into(), "FL".into()], 480),
        (48000, vec!["M".into()], 481),
    ] {
        let p = Plan {
            format: Format {
                rate,
                sample: Sample::F32Le,
                channels,
                frames,
            },
            encoding: Encoding::Opus(opus_parameters()),
        };
        assert!(negotiate(&p, &[p.clone()], &[p.clone()], true, budget()).is_err());
    }
}

#[cfg(feature = "system-opus")]
#[test]
fn opus_rejects_other_duration_layout_and_poisoned_predictor() {
    let f = Format {
        rate: 48000,
        sample: Sample::F32Le,
        channels: vec!["M".into()],
        frames: 480,
    };
    let n = selected(f.clone(), Encoding::Opus(opus_parameters()));
    let mut enc = Encoder::new((), n.clone(), binding()).unwrap();
    let mut p = enc.encode(&vec![0; n.block_bytes()]).unwrap();
    p[HEADER_BYTES] |= 4; // force stereo bit in TOC, conflicting with admitted mono
    let mut dec = Decoder::new((), n.clone(), binding()).unwrap();
    assert!(dec.decode(0, &p).is_err());
    assert!(!dec.revocation().active());
    let mut longer = f;
    longer.frames = 960;
    let longer = selected(longer, Encoding::Opus(opus_parameters()));
    let p = Encoder::new((), longer.clone(), binding())
        .unwrap()
        .encode(&vec![0; longer.block_bytes()])
        .unwrap();
    let mut forged = p.clone();
    forged[48..52].copy_from_slice(&(n.block_bytes() as u32).to_be_bytes());
    assert!(
        Decoder::new((), n, binding())
            .unwrap()
            .decode(0, &forged)
            .is_err()
    );
}

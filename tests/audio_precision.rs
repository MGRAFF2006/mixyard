#[path = "audio/support.rs"]
#[allow(dead_code)]
mod support;

use mixyard::audio::pcm::{decode, encode_exact, PcmError, SignedPcm};
use mixyard::audio::{Pcm, Precision, Processor, Processor64, RenderError};
use mixyard::{AudioFormat, Send};
use support::*;

fn pcm64(channels: u16, samples: &[f64]) -> Option<Pcm<'_, f64>> {
    Some(Pcm {
        format: AudioFormat {
            channels,
            sample_rate_hz: 48_000,
        },
        samples,
    })
}

#[test]
fn float64_unity_preserves_low_bits_signed_zero_subnormals_and_extremes() {
    let (config, inventory) = example();
    let mut p = Processor64::compile(&config, &inventory, 48_000, limits()).unwrap();
    assert_eq!(p.precision(), Precision::Float64);
    let samples = [
        0.0,
        -0.0,
        f64::from_bits(1),
        -f64::from_bits(1),
        f64::MIN_POSITIVE,
        -f64::MIN_POSITIVE,
        0.5 + 2f64.powi(-40),
        -0.5 - 2f64.powi(-40),
        1.0,
        -1.0,
        f64::MAX,
        -f64::MAX,
    ];
    let mut inputs = vec![None; p.plan().sources.len()];
    inputs[source(&p, "spotify")] = pcm64(2, &samples);
    p.render(6, &inputs).unwrap();
    for actual in [mix(&p, "desk-a"), output(&p, "speaker-a")] {
        for (value, expected) in actual.iter().zip(samples) {
            assert_eq!(value.to_bits(), expected.to_bits());
        }
    }
    // The old path rounds this difference away; precision selection stays explicit.
    let mut narrow = Processor::compile(&config, &inventory, 48_000, limits()).unwrap();
    assert_eq!(narrow.precision(), Precision::Float32);
    let mut narrow_inputs = vec![None; narrow.plan().sources.len()];
    let narrow_samples = [samples[6] as f32, samples[7] as f32];
    narrow_inputs[source(&narrow, "spotify")] = pcm(2, &narrow_samples);
    narrow.render(1, &narrow_inputs).unwrap();
    assert_eq!(output(&narrow, "speaker-a"), &[0.5, -0.5]);
    assert_ne!(output(&p, "speaker-a")[6], 0.5);
}

#[test]
fn high_precision_sends_masters_mutes_and_output_fanout_remain_independent() {
    let (mut config, inventory) = example();
    // Dyadic low bits survive every explicitly requested operation in this oracle.
    let d = 2f64.powi(-40);
    let music = [0.5 + d, -0.5 + d];
    let movie = [-0.25 + d, 0.25 + d];
    let render = |config: &mixyard::Config| {
        let mut p = Processor64::compile(config, &inventory, 48_000, limits()).unwrap();
        let mut inputs = vec![None; p.plan().sources.len()];
        inputs[source(&p, "mic")] = pcm64(1, &[0.5]);
        inputs[source(&p, "spotify")] = pcm64(2, &music);
        inputs[source(&p, "vlc")] = pcm64(2, &movie);
        p.render(1, &inputs).unwrap();
        p
    };
    let baseline = render(&config);
    assert_eq!(output(&baseline, "speaker-a"), &music);
    assert_eq!(
        output(&baseline, "speaker-c"),
        &[0.25 + d * 0.5, -0.25 + d * 0.5]
    );
    assert_eq!(output(&baseline, "party"), &[0.125 + d, -0.125 + d]);
    config
        .sends
        .iter_mut()
        .find(|s| s.source == "mic" && s.mix == "record")
        .unwrap()
        .mute = true;
    config
        .mixes
        .iter_mut()
        .find(|m| m.id == "monitor")
        .unwrap()
        .master
        .gain = 0.5;
    config
        .outputs
        .iter_mut()
        .find(|o| o.id == "speaker-c")
        .unwrap()
        .mute = true;
    let changed = render(&config);
    assert_eq!(
        mix(&changed, "record"),
        &[0.125 + d * 0.25, -0.125 + d * 0.25]
    );
    assert_eq!(
        mix(&changed, "monitor"),
        &[0.125 + d * 0.25, -0.125 + d * 0.25]
    );
    assert_eq!(output(&changed, "speaker-c"), &[0.0; 2]);
    for id in ["call-input", "speaker-a", "speaker-b", "party"] {
        assert_eq!(output(&changed, id), output(&baseline, id));
    }
    config
        .mixes
        .iter_mut()
        .find(|m| m.id == "record")
        .unwrap()
        .master
        .mute = true;
    let muted = render(&config);
    assert_eq!(mix(&muted, "record"), &[0.0; 2]);
    assert_eq!(output(&muted, "speaker-a"), &music);
}

#[test]
fn float64_channel_mapping_handles_extremes_without_overflow_and_retains_low_bits() {
    let (mut config, mut inventory) = example();
    config.sends.retain(|s| s.mix != "call");
    config.sends.push(Send {
        source: "spotify".into(),
        mix: "call".into(),
        gain: 1.0,
        mute: false,
    });
    let target = &config
        .outputs
        .iter()
        .find(|o| o.id == "speaker-a")
        .unwrap()
        .target;
    inventory
        .outputs
        .iter_mut()
        .find(|o| &o.target == target)
        .unwrap()
        .format
        .as_mut()
        .unwrap()
        .channels = 1;
    let mut p = Processor64::compile(&config, &inventory, 48_000, limits()).unwrap();
    let d = 2f64.powi(-40);
    let samples = [
        f64::MAX,
        f64::MAX,
        -f64::MAX,
        -f64::MAX,
        f64::MAX,
        -f64::MAX,
        0.5 + d,
        -0.25 + d,
        f64::from_bits(1),
        f64::from_bits(1),
    ];
    let expected = [f64::MAX, -f64::MAX, 0.0, 0.125 + d, f64::from_bits(1)];
    let mut inputs = vec![None; p.plan().sources.len()];
    inputs[source(&p, "spotify")] = pcm64(2, &samples);
    p.render(5, &inputs).unwrap();
    assert_eq!(mix(&p, "call"), &expected);
    assert_eq!(output(&p, "speaker-a"), &expected);
    assert_eq!(
        output(&p, "call-input"),
        &[
            f64::MAX,
            f64::MAX,
            -f64::MAX,
            -f64::MAX,
            0.0,
            0.0,
            0.125 + d,
            0.125 + d,
            f64::from_bits(1),
            f64::from_bits(1)
        ]
    );
}

#[test]
fn float64_rejects_malformed_blocks_clears_all_buffers_and_recovers_without_replay() {
    let (mut config, inventory) = example();
    // Even muted sends must validate supplied PCM.
    for send in &mut config.sends {
        send.mute = true;
    }
    let mut p = Processor64::compile(&config, &inventory, 48_000, limits()).unwrap();
    let slot = source(&p, "spotify");
    let mut inputs = vec![None; p.plan().sources.len()];
    let bad_samples = [
        [0.5, f64::NAN],
        [0.5, f64::INFINITY],
        [0.5, f64::NEG_INFINITY],
    ];
    for samples in &bad_samples {
        inputs[slot] = pcm64(2, samples);
        assert_eq!(
            p.render(1, &inputs),
            Err(RenderError::NonFinite {
                source: slot,
                sample: 1
            })
        );
        silent(&p);
        inputs[slot] = None;
        p.render(32, &inputs).unwrap();
        silent(&p);
    }
    inputs[slot] = pcm64(2, &[0.5]);
    assert_eq!(
        p.render(1, &inputs),
        Err(RenderError::SampleCount { source: slot })
    );
    silent(&p);
    inputs[slot] = pcm64(1, &[0.5]);
    assert_eq!(
        p.render(1, &inputs),
        Err(RenderError::FormatMismatch { source: slot })
    );
    silent(&p);
    inputs[slot] = pcm64(2, &[0.5, -0.5]);
    inputs[slot].as_mut().unwrap().format.sample_rate_hz = 96_000;
    assert_eq!(
        p.render(1, &inputs),
        Err(RenderError::FormatMismatch { source: slot })
    );
    silent(&p);
    assert_eq!(p.render(1, &[]), Err(RenderError::SourceCount));
    silent(&p);
    for frames in [0, 33, usize::MAX] {
        assert_eq!(p.render(frames, &inputs), Err(RenderError::FrameCount));
        silent(&p);
    }
    let mut inventory = inventory.clone();
    // Bind by selector rather than relying on the example observed-id spelling.
    inventory.sources.retain(|s| {
        s.selector
            != config
                .sources
                .iter()
                .find(|s| s.id == "spotify")
                .unwrap()
                .selector
    });
    let mut p = Processor64::compile(&config, &inventory, 48_000, limits()).unwrap();
    inputs[slot] = pcm64(2, &[0.5, -0.5]);
    assert_eq!(
        p.render(1, &inputs),
        Err(RenderError::DisconnectedSource { source: slot })
    );
    silent(&p);
}

#[test]
fn float64_nonempty_history_is_cleared_on_error_missing_blocks_and_unused_tails() {
    let (config, inventory) = example();
    let mut p = Processor64::compile(&config, &inventory, 48_000, limits()).unwrap();
    let mut inputs = vec![None; p.plan().sources.len()];
    let slot = source(&p, "spotify");
    inputs[slot] = pcm64(2, &[0.75; 64]);
    p.render(32, &inputs).unwrap();
    assert_eq!(output(&p, "speaker-a"), &[0.75; 64]);
    inputs[slot] = pcm64(2, &[0.5, f64::NAN]);
    assert!(p.render(1, &inputs).is_err());
    silent(&p);
    inputs.fill(None);
    p.render(32, &inputs).unwrap();
    silent(&p);
    inputs[slot] = pcm64(2, &[0.5, -0.5]);
    p.render(1, &inputs).unwrap();
    assert_eq!(output(&p, "speaker-a"), &[0.5, -0.5]);
    inputs.fill(None);
    p.render(32, &inputs).unwrap();
    silent(&p);
}

#[test]
fn precision_paths_admit_positive_rates_without_whitelisting_and_reject_mismatches() {
    for rate in [
        1,
        8_000,
        44_100,
        96_000,
        192_000,
        384_000,
        768_000,
        123_457,
        u32::MAX,
    ] {
        let (mut config, mut inventory) = example();
        for mix in &mut config.mixes {
            mix.sample_rate_hz = rate;
        }
        for source in &mut inventory.sources {
            if let Some(f) = &mut source.format {
                f.sample_rate_hz = rate;
            }
        }
        for output in &mut inventory.outputs {
            if let Some(f) = &mut output.format {
                f.sample_rate_hz = rate;
            }
        }
        let mut p = Processor64::compile(&config, &inventory, rate, limits()).unwrap();
        let mut inputs = vec![None; p.plan().sources.len()];
        let samples = [0.5 + 2f64.powi(-40), -0.5];
        inputs[source(&p, "spotify")] = Some(Pcm {
            format: AudioFormat {
                channels: 2,
                sample_rate_hz: rate,
            },
            samples: &samples,
        });
        p.render(1, &inputs).unwrap();
        assert_eq!(output(&p, "speaker-a"), &samples);
        Processor::compile(&config, &inventory, rate, limits()).unwrap();
        assert!(
            Processor64::compile(&config, &inventory, if rate == 1 { 2 } else { 1 }, limits())
                .unwrap_err()
                .contains("rate conversion")
        );
    }
    let (config, inventory) = example();
    assert!(Processor64::compile(&config, &inventory, 0, limits()).is_err());
    let mut budget = limits();
    budget.max_buffer_samples = 1152;
    Processor64::compile(&config, &inventory, 48_000, budget).unwrap();
    budget.max_buffer_samples -= 1;
    assert!(Processor64::compile(&config, &inventory, 48_000, budget)
        .unwrap_err()
        .contains("budget"));
}

#[test]
fn packed_pcm_endian_extremes_and_low_bits_survive_decode_unity_and_exact_encode() {
    use SignedPcm::*;
    // Literal wire bytes and expected integers are independent of codec loops.
    let cases: &[(SignedPcm, &[u8], &[i32], f64)] = &[
        (
            S16Le,
            &[0, 128, 255, 127, 1, 0, 255, 255, 0, 0, 1, 128],
            &[-32768, 32767, 1, -1, 0, -32767],
            32768.0,
        ),
        (
            S16Be,
            &[128, 0, 127, 255, 0, 1, 255, 255, 0, 0, 128, 1],
            &[-32768, 32767, 1, -1, 0, -32767],
            32768.0,
        ),
        (
            S24Le,
            &[
                0, 0, 128, 255, 255, 127, 1, 0, 0, 255, 255, 255, 0, 0, 0, 1, 0, 128,
            ],
            &[-8388608, 8388607, 1, -1, 0, -8388607],
            8388608.0,
        ),
        (
            S24Be,
            &[
                128, 0, 0, 127, 255, 255, 0, 0, 1, 255, 255, 255, 0, 0, 0, 128, 0, 1,
            ],
            &[-8388608, 8388607, 1, -1, 0, -8388607],
            8388608.0,
        ),
        (
            S32Le,
            &[
                0, 0, 0, 128, 255, 255, 255, 127, 1, 0, 0, 0, 255, 255, 255, 255, 0, 0, 0, 0, 1, 0,
                0, 128,
            ],
            &[i32::MIN, i32::MAX, 1, -1, 0, i32::MIN + 1],
            2147483648.0,
        ),
        (
            S32Be,
            &[
                128, 0, 0, 0, 127, 255, 255, 255, 0, 0, 0, 1, 255, 255, 255, 255, 0, 0, 0, 0, 128,
                0, 0, 1,
            ],
            &[i32::MIN, i32::MAX, 1, -1, 0, i32::MIN + 1],
            2147483648.0,
        ),
    ];
    let (config, inventory) = example();
    let mut p = Processor64::compile(&config, &inventory, 48_000, limits()).unwrap();
    for &(format, bytes, integers, scale) in cases {
        let mut samples = vec![99.0; integers.len()];
        decode(format, bytes, &mut samples).unwrap();
        for (sample, integer) in samples.iter().zip(integers) {
            assert_eq!(*sample, *integer as f64 / scale);
        }
        let mut inputs = vec![None; p.plan().sources.len()];
        inputs[source(&p, "spotify")] = pcm64(2, &samples);
        p.render(3, &inputs).unwrap();
        let mut encoded = vec![0xaa; bytes.len()];
        encode_exact(format, output(&p, "speaker-a"), &mut encoded).unwrap();
        assert_eq!(encoded, bytes);
    }
}

#[test]
fn signed32_precision_retains_adjacent_low_bits_near_both_full_scale_endpoints() {
    let integers = [
        i32::MAX,
        i32::MAX - 1,
        i32::MIN,
        i32::MIN + 1,
        16777216,
        16777217,
        -16777216,
        -16777217,
    ];
    let bytes: Vec<_> = integers.iter().flat_map(|v| v.to_le_bytes()).collect();
    let mut samples = [0.0; 8];
    decode(SignedPcm::S32Le, &bytes, &mut samples).unwrap();
    let (config, inventory) = example();
    let mut p = Processor64::compile(&config, &inventory, 48_000, limits()).unwrap();
    let mut inputs = vec![None; p.plan().sources.len()];
    inputs[source(&p, "spotify")] = pcm64(2, &samples);
    p.render(4, &inputs).unwrap();
    let actual = output(&p, "speaker-a");
    for (sample, integer) in actual.iter().zip(integers) {
        assert_eq!(*sample * 2147483648.0, integer as f64);
    }
    let mut encoded = [0; 32];
    encode_exact(SignedPcm::S32Le, actual, &mut encoded).unwrap();
    assert_eq!(encoded.as_slice(), bytes);
    // Deliberate half-gain fanout retains fractional integer precision in f64;
    // destination S32 encoding must reject the odd-integer samples, not truncate.
    assert_eq!(
        encode_exact(SignedPcm::S32Le, output(&p, "speaker-c"), &mut encoded),
        Err(PcmError::NotRepresentable { sample: 0 })
    );
    assert_eq!(encoded, [0; 32]);
    assert_eq!(output(&p, "speaker-a"), &samples);
}

#[test]
fn codecs_reject_nonfinite_range_fractional_and_malformed_without_partial_output() {
    use SignedPcm::*;
    for format in [S16Le, S16Be, S24Le, S24Be, S32Le, S32Be] {
        let mut bytes = vec![0xaa; 2 * format.bytes_per_sample()];
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            bytes.fill(0xaa);
            assert_eq!(
                encode_exact(format, &[0.5, value], &mut bytes),
                Err(PcmError::NonFinite { sample: 1 })
            );
            assert!(bytes.iter().all(|b| *b == 0));
        }
        for value in [-1.00001, 1.0, 2.0, f64::MAX] {
            assert_eq!(
                encode_exact(format, &[0.5, value], &mut bytes),
                Err(PcmError::OutOfRange { sample: 1 })
            );
            assert!(bytes.iter().all(|b| *b == 0));
        }
        assert_eq!(
            encode_exact(format, &[0.5, 2f64.powi(-40)], &mut bytes),
            Err(PcmError::NotRepresentable { sample: 1 })
        );
        assert!(bytes.iter().all(|b| *b == 0));
        bytes.fill(0xaa);
        assert_eq!(
            encode_exact(format, &[0.5], &mut bytes),
            Err(PcmError::Length)
        );
        assert!(bytes.iter().all(|b| *b == 0));
        let mut samples = [99.0; 3];
        assert_eq!(decode(format, &bytes, &mut samples), Err(PcmError::Length));
        assert_eq!(samples, [0.0; 3]);
        assert_eq!(
            decode(format, &bytes[..bytes.len() - 1], &mut samples[..2]),
            Err(PcmError::Length)
        );
        assert_eq!(samples, [0.0; 3]);
        decode(format, &[], &mut []).unwrap();
        encode_exact(format, &[], &mut []).unwrap();
        encode_exact(format, &[-0.0, 0.0], &mut bytes).unwrap();
        assert!(bytes.iter().all(|b| *b == 0));
    }
}

#[test]
fn finite_dsp_overflow_rejects_and_clears_every_mix_and_output() {
    let (config, inventory) = overflow_example();
    let mut p = Processor64::compile(&config, &inventory, 48_000, limits()).unwrap();
    let mut inputs = vec![None; p.plan().sources.len()];
    inputs[source(&p, "mic")] = pcm64(1, &[0.5]);
    for i in 0..17 {
        inputs[source(&p, &format!("overflow-{i:02}"))] = pcm64(2, &[f64::MAX; 2]);
    }
    assert_eq!(p.render(1, &inputs), Err(RenderError::NonFiniteResult));
    silent(&p);
    inputs.fill(None);
    p.render(32, &inputs).unwrap();
    silent(&p);
    inputs[source(&p, "mic")] = pcm64(1, &[0.5]);
    p.render(1, &inputs).unwrap();
    assert_eq!(output(&p, "call-input"), &[0.5; 2]);
}

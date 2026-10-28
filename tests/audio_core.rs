#[path = "audio/support.rs"]
mod support;
use mixyard::audio::{Processor, RenderError};
use mixyard::{
    AudioFormat, Gain, Mix, Origin, OutputObservation, OutputRoute, Send, Source, Target,
};
use support::*;

/// Runnable offline demo and independent literal PCM oracle, using the public
/// schema-2 example rather than a separate audio routing configuration.
#[test]
fn offline_render_demo() {
    let (config, inventory) = example();
    let mut p = Processor::compile(&config, &inventory, 48_000, limits()).unwrap();
    let mut inputs = vec![None; p.plan().sources.len()];
    inputs[source(&p, "mic")] = pcm(1, &[0.5, -0.25, 0.125]);
    inputs[source(&p, "spotify")] = pcm(2, &[0.25, -0.5, -0.125, 0.375, 0.5, 0.25]);
    inputs[source(&p, "vlc")] = pcm(2, &[-0.5, 0.125, 0.25, -0.25, 0.125, -0.5]);
    inputs[source(&p, "return")] = pcm(2, &[0.125, -0.25, 0.5, -0.125, -0.25, 0.5]);
    p.render(3, &inputs).unwrap();
    assert_eq!(mix(&p, "call"), &[0.5, -0.25, 0.125]);
    assert_eq!(
        output(&p, "call-input"),
        &[0.5, 0.5, -0.25, -0.25, 0.125, 0.125]
    );
    assert_eq!(
        mix(&p, "record"),
        &[0.34375, 0.0625, -0.03125, -0.0625, 0.125, 0.25]
    );
    assert_eq!(
        mix(&p, "monitor"),
        &[0.1875, -0.375, 0.1875, 0.125, 0.125, 0.375]
    );
    assert_eq!(output(&p, "record-input"), mix(&p, "record"));
    assert_eq!(output(&p, "headphones"), mix(&p, "monitor"));
    assert_eq!(
        output(&p, "speaker-a"),
        &[0.25, -0.5, -0.125, 0.375, 0.5, 0.25]
    );
    assert_eq!(
        output(&p, "speaker-b"),
        &[-0.5, 0.125, 0.25, -0.25, 0.125, -0.5]
    );
    assert_eq!(
        output(&p, "speaker-c"),
        &[0.125, -0.25, -0.0625, 0.1875, 0.25, 0.125]
    );
    assert_eq!(
        output(&p, "party"),
        &[-0.125, -0.1875, 0.0625, 0.0625, 0.3125, -0.125]
    );
    for (i, port) in p.plan().outputs.iter().enumerate() {
        println!("{}: {:?}", port.id, p.output_samples(i).unwrap());
    }
    println!(
        "PASS: actual offline core PCM only; no native consumer delivery or quality qualification"
    );
}

#[test]
fn independent_sends_master_mutes_and_fanout_do_not_change_siblings() {
    let (mut config, inventory) = example();
    let render = |config: &mixyard::Config| {
        let mut p = Processor::compile(config, &inventory, 48_000, limits()).unwrap();
        let mut inputs = vec![None; p.plan().sources.len()];
        inputs[source(&p, "mic")] = pcm(1, &[0.5, -0.25]);
        inputs[source(&p, "spotify")] = pcm(2, &[0.125, -0.375, 0.25, -0.5]);
        p.render(2, &inputs).unwrap();
        p
    };
    let baseline = render(&config);
    config
        .sends
        .iter_mut()
        .find(|s| s.source == "mic" && s.mix == "record")
        .unwrap()
        .mute = true;
    config
        .outputs
        .iter_mut()
        .find(|o| o.id == "speaker-c")
        .unwrap()
        .mute = true;
    config
        .mixes
        .iter_mut()
        .find(|m| m.id == "monitor")
        .unwrap()
        .master = Gain {
        gain: 0.5,
        mute: false,
    };
    let changed = render(&config);
    assert_eq!(
        mix(&changed, "record"),
        &[0.03125, -0.09375, 0.0625, -0.125]
    );
    assert_eq!(
        mix(&changed, "monitor"),
        &[0.03125, -0.09375, 0.0625, -0.125]
    );
    assert_eq!(output(&changed, "speaker-c"), &[0.0; 4]);
    for id in ["call-input", "speaker-a", "speaker-b", "party"] {
        assert_eq!(output(&changed, id), output(&baseline, id), "sibling {id}");
    }
    config
        .mixes
        .iter_mut()
        .find(|m| m.id == "record")
        .unwrap()
        .master
        .mute = true;
    let muted = render(&config);
    assert_eq!(mix(&muted, "record"), &[0.0; 4]);
    assert_eq!(
        output(&muted, "call-input"),
        output(&baseline, "call-input")
    );
}

#[test]
fn stereo_averaging_at_send_and_output_and_mono_duplication() {
    let (mut config, mut inventory) = example();
    config.sends.retain(|s| s.mix != "call");
    config.sends.push(Send {
        source: "spotify".into(),
        mix: "call".into(),
        gain: 1.0,
        mute: false,
    });
    let target = config
        .outputs
        .iter()
        .find(|o| o.id == "speaker-a")
        .unwrap()
        .target
        .clone();
    inventory
        .outputs
        .iter_mut()
        .find(|o| o.target == target)
        .unwrap()
        .format
        .as_mut()
        .unwrap()
        .channels = 1;
    let mut p = Processor::compile(&config, &inventory, 48_000, limits()).unwrap();
    let mut inputs = vec![None; p.plan().sources.len()];
    inputs[source(&p, "spotify")] = pcm(2, &[0.75, -0.25, -0.5, 0.25, f32::MAX, f32::MAX]);
    p.render(3, &inputs).unwrap();
    assert_eq!(mix(&p, "call"), &[0.25, -0.125, f32::MAX]);
    assert_eq!(
        output(&p, "call-input"),
        &[0.25, 0.25, -0.125, -0.125, f32::MAX, f32::MAX]
    );
    assert_eq!(output(&p, "speaker-a"), &[0.25, -0.125, f32::MAX]);
}

#[test]
fn unity_is_bit_transparent_including_zero_subnormals_and_extreme_finite_samples() {
    let (config, inventory) = example();
    let mut p = Processor::compile(&config, &inventory, 48_000, limits()).unwrap();
    let samples: Vec<f32> = [
        0, 0x80000000, 1, 0x80000001, 0x007fffff, 0x00800000, 0x3eaaaaab, 0xbeaaaaab, 0x3f800000,
        0xbf800000, 0x7f7fffff, 0xff7fffff,
    ]
    .into_iter()
    .map(f32::from_bits)
    .collect();
    let mut inputs = vec![None; p.plan().sources.len()];
    inputs[source(&p, "spotify")] = pcm(2, &samples);
    p.render(samples.len() / 2, &inputs).unwrap();
    let bits = |data: &[f32]| data.iter().map(|s| s.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(mix(&p, "desk-a")), bits(&samples));
    assert_eq!(bits(output(&p, "speaker-a")), bits(&samples));
}

#[test]
fn absent_inputs_and_recompiled_missing_or_ambiguous_ports_never_replay_or_fallback() {
    let (config, inventory) = example();
    let mut p = Processor::compile(&config, &inventory, 48_000, limits()).unwrap();
    let mut inputs = vec![None; p.plan().sources.len()];
    inputs[source(&p, "spotify")] = pcm(2, &[0.5, -0.25, 0.25, -0.5]);
    p.render(2, &inputs).unwrap();
    inputs.fill(None);
    p.render(1, &inputs).unwrap();
    silent(&p);
    p.render(2, &inputs).unwrap();
    silent(&p);
    for ambiguous in [false, true] {
        let mut inventory = inventory.clone();
        if ambiguous {
            let mut duplicate = inventory.sources[1].clone();
            duplicate.observed_id = "second-music".into();
            inventory.sources.push(duplicate);
            let mut duplicate = inventory.outputs[3].clone();
            duplicate.observed_id = "second-speaker".into();
            inventory.outputs.push(duplicate);
        } else {
            inventory.sources.remove(1);
            inventory.outputs.remove(3);
        }
        let mut p = Processor::compile(&config, &inventory, 48_000, limits()).unwrap();
        let mut inputs = vec![None; p.plan().sources.len()];
        inputs[source(&p, "mic")] = pcm(1, &[0.5]);
        inputs[source(&p, "vlc")] = pcm(2, &[0.25, -0.25]);
        p.render(1, &inputs).unwrap();
        assert_eq!(output(&p, "speaker-a"), &[0.0; 2]);
        assert_eq!(output(&p, "speaker-c"), &[0.0; 2]);
        assert_eq!(output(&p, "speaker-b"), &[0.25, -0.25]);
        assert_eq!(output(&p, "call-input"), &[0.5; 2]);
        inputs[source(&p, "spotify")] = pcm(2, &[0.75, 0.75]);
        assert!(matches!(
            p.render(1, &inputs),
            Err(RenderError::DisconnectedSource { .. })
        ));
        silent(&p);
    }
}

#[test]
fn missing_output_is_silent_while_same_mix_fanout_and_unselected_private_source_are_isolated() {
    let (mut config, mut inventory) = example();
    let private = inventory
        .sources
        .iter_mut()
        .find(|s| s.observed_id == "simulated:private")
        .unwrap();
    private.format = Some(AudioFormat {
        channels: 1,
        sample_rate_hz: 48_000,
    });
    config.sources.push(Source {
        id: "private".into(),
        selector: private.selector.clone(),
    });
    // This output disappears while its source and sibling fanout stay available.
    inventory
        .outputs
        .retain(|o| o.observed_id != "simulated:output:3");
    let mut p = Processor::compile(&config, &inventory, 48_000, limits()).unwrap();
    let mut inputs = vec![None; p.plan().sources.len()];
    inputs[source(&p, "spotify")] = pcm(2, &[0.5, -0.25]);
    let private_slot = source(&p, "private");
    inputs[private_slot] = pcm(1, &[0.875]);
    p.render(1, &inputs).unwrap();
    assert_eq!(mix(&p, "call"), &[0.0]);
    assert_eq!(mix(&p, "record"), &[0.125, -0.0625]);
    assert_eq!(mix(&p, "desk-a"), &[0.5, -0.25]);
    assert_eq!(output(&p, "speaker-a"), &[0.0; 2]);
    let missing = p
        .plan()
        .outputs
        .iter()
        .position(|o| o.id == "speaker-a")
        .unwrap();
    let selected = p
        .plan()
        .outputs
        .iter()
        .position(|o| o.id == "speaker-c")
        .unwrap();
    assert_eq!(p.output_format(missing), None);
    assert_eq!(
        p.output_format(selected),
        Some(AudioFormat {
            channels: 2,
            sample_rate_hz: 48_000
        })
    );
    assert_eq!(output(&p, "speaker-c"), &[0.25, -0.125]);
    inputs[private_slot] = pcm(1, &[f32::NAN]);
    assert_eq!(
        p.render(1, &inputs),
        Err(RenderError::NonFinite {
            source: private_slot,
            sample: 0
        })
    );
    silent(&p);
}

#[test]
fn invalid_blocks_fail_closed_before_any_output_and_recover_next_block() {
    let (config, inventory) = example();
    let mut p = Processor::compile(&config, &inventory, 48_000, limits()).unwrap();
    let mic = source(&p, "mic");
    let vlc = source(&p, "vlc");
    let mut inputs = vec![None; p.plan().sources.len()];
    inputs[mic] = pcm(1, &[0.5]);
    let bad_cases = [
        [0.0, f32::NAN],
        [0.0, f32::INFINITY],
        [0.0, f32::NEG_INFINITY],
    ];
    for bad_samples in &bad_cases {
        p.render(1, &inputs).unwrap();
        // Last source/last sample must poison the whole block.
        inputs[vlc] = pcm(2, bad_samples);
        assert_eq!(
            p.render(1, &inputs),
            Err(RenderError::NonFinite {
                source: vlc,
                sample: 1
            })
        );
        silent(&p);
        inputs[vlc] = None;
    }
    p.render(1, &inputs).unwrap();
    inputs[mic] = pcm(1, &[]);
    assert_eq!(
        p.render(1, &inputs),
        Err(RenderError::SampleCount { source: mic })
    );
    silent(&p);
    inputs[mic] = pcm(1, &[0.5, 0.25]);
    assert_eq!(
        p.render(1, &inputs),
        Err(RenderError::SampleCount { source: mic })
    );
    silent(&p);
    inputs[mic] = pcm(2, &[0.5, 0.25]);
    assert_eq!(
        p.render(1, &inputs),
        Err(RenderError::FormatMismatch { source: mic })
    );
    silent(&p);
    inputs[mic] = pcm(1, &[0.5]);
    inputs[mic].as_mut().unwrap().format.sample_rate_hz = 44_100;
    assert_eq!(
        p.render(1, &inputs),
        Err(RenderError::FormatMismatch { source: mic })
    );
    silent(&p);
    assert_eq!(p.render(1, &inputs[..1]), Err(RenderError::SourceCount));
    silent(&p);
    assert_eq!(p.render(33, &inputs), Err(RenderError::FrameCount));
    assert_eq!(p.frames(), 32);
    silent(&p);
    assert_eq!(p.render(usize::MAX, &inputs), Err(RenderError::FrameCount));
    silent(&p);
    assert_eq!(p.render(0, &inputs), Err(RenderError::FrameCount));
    inputs[mic] = pcm(1, &[0.5]);
    p.render(1, &inputs).unwrap();
    assert_eq!(output(&p, "call-input"), &[0.5; 2]);
}

#[test]
fn compiler_reuses_policy_for_exclusions_consent_recapture_headroom_and_capacity() {
    let (config, inventory) = example();
    let mut invalid = Vec::new();
    let mut c = config.clone();
    c.sends.push(Send {
        source: "return".into(),
        mix: "call".into(),
        gain: 0.0,
        mute: true,
    });
    invalid.push((c, inventory.clone()));
    let mut c = config.clone();
    c.outputs
        .iter_mut()
        .find(|o| o.id == "headphones")
        .unwrap()
        .allow_mic_monitor = false;
    invalid.push((c, inventory.clone()));
    let mut c = config.clone();
    c.sends
        .iter_mut()
        .find(|s| s.source == "mic" && s.mix == "record")
        .unwrap()
        .gain = 0.75;
    invalid.push((c, inventory.clone()));
    let mut i = inventory.clone();
    i.sources[0].origin = Origin::MixyardProducer;
    invalid.push((config.clone(), i));
    let mut i = inventory.clone();
    i.limits.max_exported_mixes = Some(1);
    invalid.push((config.clone(), i));
    let mut c = config.clone();
    c.schema_version = 1;
    invalid.push((c, inventory.clone()));
    for (config, inventory) in invalid {
        // Identical policy error proves this boundary did not substitute a policy.
        let policy = mixyard::plan(&config, &inventory).unwrap_err();
        assert_eq!(
            Processor::compile(&config, &inventory, 48_000, limits()).unwrap_err(),
            policy
        );
    }
}

#[test]
fn unsupported_or_unknown_formats_and_rate_conversion_are_explicit_compile_errors() {
    let (config, inventory) = example();
    for format in [
        None,
        Some(AudioFormat {
            channels: 6,
            sample_rate_hz: 48_000,
        }),
        Some(AudioFormat {
            channels: 2,
            sample_rate_hz: 44_100,
        }),
        Some(AudioFormat {
            channels: 2,
            sample_rate_hz: 0,
        }),
    ] {
        let mut i = inventory.clone();
        i.sources[1].format = format;
        assert!(Processor::compile(&config, &i, 48_000, limits()).is_err());
        let mut i = inventory.clone();
        i.outputs[3].format = format;
        assert!(Processor::compile(&config, &i, 48_000, limits()).is_err());
    }
    let mut c = config.clone();
    c.mixes[0].sample_rate_hz = 44_100;
    assert!(Processor::compile(&c, &inventory, 48_000, limits()).is_err());
    assert!(Processor::compile(&config, &inventory, 0, limits()).is_err());
    // Not tied to a hard-coded 48 kHz rate: equal explicit formats are supported.
    let mut c = config.clone();
    let mut i = inventory.clone();
    for m in &mut c.mixes {
        m.sample_rate_hz = 44_100;
    }
    for s in &mut i.sources {
        if let Some(f) = &mut s.format {
            f.sample_rate_hz = 44_100;
        }
    }
    for o in &mut i.outputs {
        if let Some(f) = &mut o.format {
            f.sample_rate_hz = 44_100;
        }
    }
    Processor::compile(&c, &i, 44_100, limits()).unwrap();
}

#[test]
fn explicit_resource_budgets_reject_without_truncation_and_allow_many_custom_mixes() {
    let (mut config, mut inventory) = example();
    for i in 0..40 {
        let id = format!("custom-{i}");
        let target = Target::VirtualInput {
            endpoint_id: id.clone(),
        };
        config.mixes.push(Mix {
            id: id.clone(),
            name: "Café / duplicate label".into(),
            channels: 2,
            sample_rate_hz: 48_000,
            master: Gain::default(),
            exclude_groups: vec![],
        });
        config.sends.push(Send {
            source: "spotify".into(),
            mix: id.clone(),
            gain: 1.0,
            mute: false,
        });
        config.outputs.push(OutputRoute {
            id: id.clone(),
            mix: id.clone(),
            target: target.clone(),
            gain: 1.0,
            mute: false,
            allow_mic_monitor: false,
        });
        inventory.outputs.push(OutputObservation {
            observed_id: format!("endpoint-{i}"),
            target,
            format: Some(AudioFormat {
                channels: 2,
                sample_rate_hz: 48_000,
            }),
        });
    }
    let mut p = Processor::compile(&config, &inventory, 48_000, limits()).unwrap();
    let mut inputs = vec![None; p.plan().sources.len()];
    inputs[source(&p, "spotify")] = pcm(2, &[0.25, -0.5]);
    p.render(1, &inputs).unwrap();
    for i in 0..40 {
        assert_eq!(output(&p, &format!("custom-{i}")), &[0.25, -0.5]);
    }
    for kind in 0..7 {
        let mut limits = limits();
        match kind {
            0 => limits.max_frames = 0,
            1 => limits.max_sources = config.sources.len() - 1,
            2 => limits.max_mixes = config.mixes.len() - 1,
            3 => limits.max_sends = config.sends.len() - 1,
            4 => limits.max_outputs = config.outputs.len() - 1,
            5 => limits.max_buffer_samples = 1,
            _ => {
                limits.max_frames = usize::MAX;
                limits.max_buffer_samples = usize::MAX;
            }
        }
        assert!(Processor::compile(&config, &inventory, 48_000, limits).is_err());
    }
}

#[test]
fn storage_budget_accounts_for_accumulators_and_independent_output_copies() {
    let (config, inventory) = example();
    let mut limits = limits();
    // Example: 11 mix channels with f64 + f32 storage, seven stereo outputs,
    // 32 frames. These fixture counts are independent of compiled allocations.
    limits.max_buffer_samples = 1152;
    Processor::compile(&config, &inventory, 48_000, limits).unwrap();
    limits.max_buffer_samples = 1151;
    assert!(Processor::compile(&config, &inventory, 48_000, limits)
        .unwrap_err()
        .contains("budget"));
    limits.max_frames = isize::MAX as usize / 8;
    limits.max_buffer_samples = usize::MAX;
    assert!(Processor::compile(&config, &inventory, 48_000, limits).is_err());
}

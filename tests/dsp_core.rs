#[path = "dsp/support.rs"]
mod support;
use mixyard::dsp::*;
use support::*;

#[test]
fn dynamics_do_not_erase_representable_extreme_amplitudes() {
    let mut p = dynamics();
    p.threshold_db = -6000.0;
    let mut compressor = chain(vec![Effect::Compressor { parameters: p }], 2, 48000, 1);
    compressor.process(1, &[1e300, -1e300]).unwrap();
    // Static output law: -6000 + (6000 - -6000)/4 = -3000 dB.
    for (actual, expected) in compressor.output().iter().zip([1e-150, -1e-150]) {
        assert!(
            (actual / expected - 1.0).abs() < 1e-12,
            "{actual:e} != {expected:e}"
        );
    }
    p.threshold_db = -2600.0;
    let mut subnormal_gain = chain(vec![Effect::Compressor { parameters: p }], 1, 48000, 1);
    subnormal_gain.process(1, &[1e300]).unwrap();
    // Output is -450 dB even though the intermediate linear gain is subnormal.
    assert!((subnormal_gain.output()[0] / 3.162277660168379e-23 - 1.0).abs() < 1e-12);
    p.threshold_db = 0.0;
    p.ratio = 2.0;
    p.range_db = 6000.0;
    p.makeup_db = 6000.0;
    let mut expander = chain(vec![Effect::Expander { parameters: p }], 2, 48000, 1);
    expander.process(1, &[1e-300, -1e-300]).unwrap();
    // -6000 dB attenuation is exactly cancelled by explicit +6000 dB makeup.
    assert_eq!(
        expander
            .output()
            .iter()
            .map(|x| x.to_bits())
            .collect::<Vec<_>>(),
        [1e-300f64, -1e-300].map(f64::to_bits)
    );
    p.makeup_db = 5900.0;
    let mut expander = chain(vec![Effect::Expander { parameters: p }], 1, 48000, 1);
    expander.process(1, &[1e-300]).unwrap();
    assert!((expander.output()[0] / 1e-305 - 1.0).abs() < 1e-12);
}

#[test]
fn exact_disabled_empty_and_chain_bypass_preserve_float64_bits() {
    let pcm = [
        -0.0,
        0.0,
        0.5 + 2f64.powi(-50),
        -2f64.powi(-1000),
        f64::MIN_POSITIVE,
        f64::MAX,
    ];
    let mut cfg = config(
        vec![
            Effect::Gain {
                gain: 2.0,
                invert_polarity: true,
            },
            Effect::Fir {
                coefficients: vec![0.5, 0.25],
            },
        ],
        1,
        48000,
    );
    for e in &mut cfg.effects {
        e.enabled = false;
    }
    let mut disabled = Chain::compile(cfg, limits(6)).unwrap();
    let mut empty = chain(vec![], 1, 48000, 6);
    let mut bypass = chain(
        vec![Effect::Gain {
            gain: f64::MAX,
            invert_polarity: false,
        }],
        1,
        48000,
        6,
    );
    bypass.set_bypassed(true);
    let mut unity = chain(
        vec![Effect::Gain {
            gain: 1.0,
            invert_polarity: false,
        }],
        1,
        48000,
        6,
    );
    for c in [&mut disabled, &mut empty, &mut bypass, &mut unity] {
        c.process(6, &pcm).unwrap();
        assert_eq!(
            c.output().iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
            pcm.map(f64::to_bits)
        );
        assert!(c.meters()[0].rms.is_finite());
    }
}

#[test]
fn gain_polarity_headroom_and_stable_rms_are_explicit() {
    let mut c = chain(
        vec![Effect::Gain {
            gain: 4.0,
            invert_polarity: true,
        }],
        2,
        48000,
        2,
    );
    let report = c.process(2, &[0.5, -0.25, -0.5, 0.0]).unwrap();
    assert_eq!(c.output(), &[-2.0, 1.0, 2.0, -0.0]);
    assert_eq!(report.over_full_scale_samples, 2);
    close(c.meters()[0].rms, 2.0, 0.0);
    close(c.meters()[1].rms, std::f64::consts::FRAC_1_SQRT_2, 1e-15);
    let mut c = chain(vec![], 1, 48000, 2);
    c.process(2, &[f64::MAX, -f64::MAX]).unwrap();
    assert_eq!(c.meters()[0].peak, f64::MAX);
    assert_eq!(c.meters()[0].rms, f64::MAX);
}

#[test]
fn ordered_gain_and_compressor_produce_different_correct_outputs() {
    let p = dynamics();
    let gain = Effect::Gain {
        gain: 2.0,
        invert_polarity: false,
    };
    let comp = Effect::Compressor { parameters: p };
    let mut before = chain(vec![gain.clone(), comp.clone()], 1, 48000, 1);
    let mut after = chain(vec![comp, gain], 1, 48000, 1);
    before.process(1, &[0.5]).unwrap();
    after.process(1, &[0.5]).unwrap();
    // Static law: level out = threshold + (level in - threshold)/ratio.
    let at_one = 10f64.powf((-20.0 + 20.0 / p.ratio) / 20.0);
    let at_half = 10f64.powf((-20.0 + (20.0 * 0.5f64.log10() + 20.0) / p.ratio) / 20.0);
    close(before.output()[0], at_one, 1e-15);
    close(after.output()[0], 2.0 * at_half, 1e-15);
    assert!(after.output()[0] > before.output()[0]);
}

#[test]
fn delay_impulse_feedback_and_fir_sequences_cross_block_boundaries() {
    let delay = Effect::Delay {
        frames: 2,
        wet: 0.5,
        dry: 0.25,
        feedback: 0.5,
    };
    let mut c = chain(vec![delay.clone()], 1, 48000, 8);
    let input = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let expected = [0.25, 0.0, 0.5, 0.0, 0.25, 0.0, 0.125, 0.0];
    assert_eq!(render(&mut c, &input, 3), expected);
    assert_eq!(c.admission().tail, Tail::Infinite);
    let mut full = chain(vec![delay], 1, 48000, 8);
    assert_eq!(render(&mut full, &input, 8), expected);
    let fir = Effect::Fir {
        coefficients: vec![1.0, -0.5, 0.25],
    };
    for block in [1, 2, 4, 6] {
        let mut c = chain(vec![fir.clone()], 1, 48000, 6);
        assert_eq!(
            render(&mut c, &[1.0, 2.0, 3.0, 4.0, 0.0, 0.0], block),
            [1.0, 1.5, 2.25, 3.0, -1.25, 1.0]
        );
        assert_eq!(c.admission().tail, Tail::Finite { frames: 2 });
    }
}

#[test]
fn reset_loss_errors_and_bypass_never_replay_retained_tails() {
    for effect in [
        Effect::Delay {
            frames: 2,
            wet: 1.0,
            dry: 0.0,
            feedback: 0.75,
        },
        Effect::Fir {
            coefficients: vec![0.0, 0.0, 1.0],
        },
        filter(Filter::LowPass, 1000.0, 0.0),
    ] {
        let mut c = chain(vec![effect], 1, 48000, 4);
        c.process(1, &[1.0]).unwrap();
        c.process(4, &[0.0; 4]).unwrap();
        assert!(
            c.output().iter().any(|x| *x != 0.0),
            "normal tails should be rendered"
        );
        for error in [
            ProcessError::FrameCount,
            ProcessError::SampleCount,
            ProcessError::NonFiniteInput { sample: 0 },
        ] {
            c.process(1, &[1.0]).unwrap();
            let result = match error {
                ProcessError::FrameCount => c.process(5, &[0.0; 5]),
                ProcessError::SampleCount => c.process(2, &[0.0]),
                _ => c.process(1, &[f64::NAN]),
            };
            assert_eq!(result, Err(error));
            assert!(c.output().is_empty());
            assert!(c.meters().iter().all(|m| m.peak == 0.0 && m.rms == 0.0));
            c.process(4, &[0.0; 4]).unwrap();
            assert_eq!(c.output(), &[0.0; 4]);
        }
        c.process(1, &[1.0]).unwrap();
        c.reset();
        assert!(c.output().is_empty());
        c.process(4, &[0.0; 4]).unwrap();
        assert_eq!(c.output(), &[0.0; 4]);
        c.process(1, &[1.0]).unwrap();
        c.set_bypassed(true);
        c.process(1, &[-0.0]).unwrap();
        assert_eq!(c.output()[0].to_bits(), (-0.0f64).to_bits());
        c.set_bypassed(false);
        c.process(4, &[0.0; 4]).unwrap();
        assert_eq!(c.output(), &[0.0; 4]);
    }
}

#[test]
fn nonfinite_result_discards_earlier_stages_partial_block_and_hidden_feedback() {
    let delay = Effect::Delay {
        frames: 1,
        wet: 1.0,
        dry: 1.0,
        feedback: 0.5,
    };
    let mut c = chain(
        vec![
            delay.clone(),
            Effect::Gain {
                gain: 4.0,
                invert_polarity: false,
            },
        ],
        1,
        48000,
        3,
    );
    assert_eq!(
        c.process(3, &[1.0, 1.0, f64::MAX]),
        Err(ProcessError::NonFiniteResult { effect: 1 })
    );
    assert!(c.output().is_empty());
    c.process(3, &[0.0; 3]).unwrap();
    assert_eq!(c.output(), &[0.0; 3]);
    // The inaudible feedback storage must also be checked, even with wet=0.
    let mut c = chain(
        vec![Effect::Delay {
            frames: 1,
            wet: 0.0,
            dry: 1.0,
            feedback: 0.5,
        }],
        1,
        48000,
        3,
    );
    assert_eq!(
        c.process(3, &[f64::MAX; 3]),
        Err(ProcessError::NonFiniteResult { effect: 0 })
    );
    assert!(c.output().is_empty());
    let mut c = chain(
        vec![Effect::Fir {
            coefficients: vec![f64::MAX, f64::MAX],
        }],
        1,
        48000,
        2,
    );
    assert_eq!(
        c.process(2, &[1.0, 1.0]),
        Err(ProcessError::NonFiniteResult { effect: 0 })
    );
    c.process(2, &[0.0; 2]).unwrap();
    assert_eq!(c.output(), &[0.0; 2]);
}

#[test]
fn arbitrary_ordered_channels_have_independent_filter_delay_fir_and_dynamics_state() {
    for effect in [
        filter(Filter::LowPass, 2000.0, 0.0),
        Effect::Delay {
            frames: 3,
            wet: 0.8,
            dry: 0.2,
            feedback: 0.5,
        },
        Effect::Fir {
            coefficients: vec![0.5, -0.3, 0.2],
        },
        Effect::Compressor {
            parameters: dynamics(),
        },
        Effect::Expander {
            parameters: dynamics(),
        },
    ] {
        let mut cfg = config(vec![effect.clone()], 8, 48000);
        cfg.format.channels = [
            "Z",
            "rear-right",
            "mic:3",
            "LFE",
            "ambisonic-W",
            "other",
            "first",
            "last",
        ]
        .map(str::to_owned)
        .to_vec();
        let expected_ids = cfg.format.channels.clone();
        let mut multi = Chain::compile(cfg, limits(16)).unwrap();
        let mut mono = chain(vec![effect], 1, 48000, 16);
        let mut input = [0.0; 128];
        input[2] = 0.5;
        let mut single = [0.0; 16];
        single[0] = 0.5;
        let actual = render(&mut multi, &input, 3);
        let expected = render(&mut mono, &single, 3);
        for frame in 0..16 {
            for ch in 0..8 {
                close(
                    actual[frame * 8 + ch],
                    if ch == 2 { expected[frame] } else { 0.0 },
                    0.0,
                );
            }
        }
        assert_eq!(multi.config().format.channels, expected_ids);
    }
    // Hot left-channel input must not link a quiet right-channel compressor.
    let effect = Effect::Compressor {
        parameters: dynamics(),
    };
    let mut c = chain(vec![effect], 2, 48000, 1);
    c.process(1, &[1.0, 0.01]).unwrap();
    close(c.output()[1], 0.01, 0.0);
}

#[test]
fn block_partitioning_preserves_all_stateful_processing() {
    let mut p = dynamics();
    p.attack_ms = 3.0;
    p.release_ms = 20.0;
    p.knee_db = 6.0;
    let effects = vec![
        filter(Filter::Peaking, 3000.0, 4.0),
        Effect::Compressor { parameters: p },
        Effect::Expander { parameters: p },
        Effect::Delay {
            frames: 7,
            wet: 0.3,
            dry: 0.7,
            feedback: -0.3,
        },
        Effect::Fir {
            coefficients: vec![1.0, 0.5, -0.25, 0.125],
        },
    ];
    let input: Vec<f64> = (0..1024).map(|i| (i as f64 * 0.13).sin() * 0.5).collect();
    let mut a = chain(effects.clone(), 2, 44117, 512);
    let mut b = chain(effects, 2, 44117, 512);
    assert_eq!(render(&mut a, &input, 512), render(&mut b, &input, 7));
}

#[test]
fn lowpass_quarter_rate_impulse_has_known_butterworth_response() {
    let mut c = chain(vec![filter(Filter::LowPass, 12000.0, 0.0)], 1, 48000, 8);
    c.process(8, &[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])
        .unwrap();
    let b = (2.0 - 2f64.sqrt()) / 2.0;
    let pole = (2f64.sqrt() - 1.0).powi(2);
    for (actual, expected) in c.output().iter().zip([
        b,
        2.0 * b,
        b * (1.0 - pole),
        -2.0 * b * pole,
        -b * pole * (1.0 - pole),
        2.0 * b * pole * pole,
        b * pole * pole * (1.0 - pole),
        -2.0 * b * pole.powi(3),
    ]) {
        close(*actual, expected, 1e-15);
    }
}

fn steady_gain(effect: Effect, frequency: f64) -> f64 {
    let input: Vec<_> = (0..16384)
        .map(|i| (2.0 * std::f64::consts::PI * frequency * i as f64 / 48000.0).sin())
        .collect();
    let mut c = chain(vec![effect], 1, 48000, 128);
    let out = render(&mut c, &input, 128);
    let energy = |xs: &[f64]| xs.iter().map(|x| x * x).sum::<f64>();
    (energy(&out[8192..]) / energy(&input[8192..])).sqrt()
}

#[test]
fn filters_match_independent_dc_nyquist_and_steady_tone_targets() {
    let center = 3000.0;
    for (kind, db, expected) in [
        (Filter::LowPass, 0.0, std::f64::consts::FRAC_1_SQRT_2),
        (Filter::HighPass, 0.0, std::f64::consts::FRAC_1_SQRT_2),
        (Filter::BandPass, 0.0, 1.0),
        (Filter::Notch, 0.0, 0.0),
        (Filter::Peaking, 6.0, 10f64.powf(6.0 / 20.0)),
        (Filter::AllPass, 0.0, 1.0),
    ] {
        close(
            steady_gain(filter(kind, center, db), center),
            expected,
            1e-10,
        );
    }
    for frequency in [750.0, 6000.0, 12000.0] {
        close(
            steady_gain(filter(Filter::AllPass, center, 0.0), frequency),
            1.0,
            1e-10,
        );
    }
    for (kind, db, dc, nyquist) in [
        (Filter::LowPass, 0.0, 1.0, 0.0),
        (Filter::HighPass, 0.0, 0.0, 1.0),
        (Filter::LowShelf, 20.0 * 2f64.log10(), 2.0, 1.0),
        (Filter::HighShelf, 20.0 * 2f64.log10(), 1.0, 2.0),
    ] {
        let mut c = chain(vec![filter(kind, center, db)], 1, 48000, 4096);
        c.process(4096, &[1.0; 4096]).unwrap();
        close(c.output()[4095], dc, 1e-12);
        c.reset();
        let input: Vec<_> = (0..4096)
            .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        c.process(4096, &input).unwrap();
        close(c.output()[4095], -nyquist, 1e-12);
    }
}

#[test]
fn compressor_attack_and_release_follow_amplitude_time_constant() {
    let mut p = dynamics();
    p.ratio = 2.0;
    p.attack_ms = 10.0;
    p.release_ms = 20.0;
    let mut c = chain(vec![Effect::Compressor { parameters: p }], 1, 1000, 20);
    c.process(10, &[1.0; 10]).unwrap();
    // At one attack time constant a unit step's detector is 1-exp(-1).
    let envelope = 1.0 - (-1.0f64).exp();
    close(c.output()[9], (0.1 / envelope).sqrt(), 1e-14);
    // Quiet input after the hot step uses release, retaining compression.
    c.process(20, &[0.01; 20]).unwrap();
    let released = 0.01 + (envelope - 0.01) * (-1.0f64).exp();
    close(c.output()[19], 0.01 * (0.1 / released).sqrt(), 1e-14);
    assert!(c.output()[0] < c.output()[19]);
    c.reset();
    c.process(1, &[0.01]).unwrap();
    close(c.output()[0], 0.01, 0.0);
}

#[test]
fn soft_knees_and_expander_gate_range_have_stated_static_laws() {
    let mut p = dynamics();
    p.ratio = 2.0;
    p.knee_db = 8.0;
    let mut c = chain(vec![Effect::Compressor { parameters: p }], 1, 48000, 1);
    c.process(1, &[0.1]).unwrap();
    // At the threshold the quadratic knee attenuates (1-1/R)*W/8 = 0.5dB.
    close(c.output()[0], 0.1 * 10f64.powf(-0.5 / 20.0), 1e-15);
    p.knee_db = 0.0;
    p.range_db = 30.0;
    let mut c = chain(vec![Effect::Expander { parameters: p }], 1, 48000, 3);
    c.process(3, &[0.01, 0.00001, 0.5]).unwrap();
    close(c.output()[0], 0.001, 1e-17); // -40dB -> -60dB at ratio 2 below -20dB.
    close(c.output()[1], 0.00001 * 10f64.powf(-30.0 / 20.0), 1e-20);
    close(c.output()[2], 0.5, 0.0);
    p.ratio = 1000.0;
    p.knee_db = 2.0;
    p.range_db = 80.0;
    let mut c = chain(vec![Effect::Expander { parameters: p }], 1, 48000, 2);
    c.process(2, &[0.01, 0.5]).unwrap();
    close(c.output()[0], 0.000001, 1e-20);
    close(c.output()[1], 0.5, 0.0);
}

#[test]
fn positive_negotiated_rates_are_not_whitelisted() {
    for rate in [1, 8000, 11025, 44117, 96000, 192000, 768000, u32::MAX] {
        let mut c = chain(
            vec![filter(Filter::LowPass, rate as f64 / 4.0, 0.0)],
            3,
            rate,
            1,
        );
        c.process(1, &[1.0, 0.0, 0.0]).unwrap();
        close(c.output()[0], (2.0 - 2f64.sqrt()) / 2.0, 1e-15);
        assert_eq!(c.config().format.sample_rate_hz, rate);
    }
}

#[test]
fn resource_budgets_and_arithmetic_are_checked_before_state_allocation() {
    let cfg = config(
        vec![
            Effect::Delay {
                frames: 500,
                wet: 1.0,
                dry: 0.0,
                feedback: 0.0,
            },
            Effect::Fir {
                coefficients: vec![0.5; 100],
            },
        ],
        8,
        48000,
    );
    let c = Chain::compile(cfg.clone(), limits(128)).unwrap();
    let admission = c.admission();
    for (resource, limit) in [
        ("channels", 7),
        ("effects", 1),
        ("memoryBytes", admission.memory_bytes - 1),
        ("workUnitsPerBlock", admission.work_units_per_block - 1),
    ] {
        let mut l = limits(128);
        match resource {
            "channels" => l.max_channels = limit,
            "effects" => l.max_effects = limit,
            "memoryBytes" => l.max_memory_bytes = limit,
            _ => l.max_work_units_per_block = limit,
        }
        assert!(
            matches!(Chain::compile(cfg.clone(),l),Err(CompileError::Budget { resource:r,.. }) if r==resource)
        );
    }
    let mut l = limits(usize::MAX);
    l.max_memory_bytes = usize::MAX;
    l.max_work_units_per_block = usize::MAX;
    assert!(matches!(
        Chain::compile(config(vec![], 2, 48000), l),
        Err(CompileError::ResourceOverflow)
    ));
    let huge = config(
        vec![Effect::Delay {
            frames: usize::MAX,
            wet: 1.0,
            dry: 0.0,
            feedback: 0.0,
        }],
        2,
        48000,
    );
    assert!(matches!(
        Chain::compile(huge, limits(1)),
        Err(CompileError::ResourceOverflow)
    ));
}

#[test]
fn invalid_formats_and_parameters_reject_even_when_disabled() {
    for mut cfg in [
        config(vec![], 0, 48000),
        config(vec![], 1, 0),
        config(vec![], 2, 48000),
    ] {
        if cfg.format.channels.len() == 2 {
            cfg.format.channels[1] = cfg.format.channels[0].clone();
        }
        assert!(matches!(
            Chain::compile(cfg, limits(1)),
            Err(CompileError::InvalidFormat)
        ));
    }
    let mut invalid_dynamics = dynamics();
    invalid_dynamics.attack_ms = -1.0;
    for e in [
        Effect::Gain {
            gain: f64::NAN,
            invert_polarity: false,
        },
        filter(Filter::LowPass, 0.0, 0.0),
        filter(Filter::LowPass, 24000.0, 0.0),
        Effect::Biquad {
            filter: Filter::Notch,
            frequency_hz: 1.0,
            q: 0.0,
            gain_db: 0.0,
        },
        Effect::Delay {
            frames: 0,
            wet: 0.0,
            dry: 1.0,
            feedback: 0.0,
        },
        Effect::Delay {
            frames: 1,
            wet: 1.0,
            dry: 0.0,
            feedback: 1.0,
        },
        Effect::Fir {
            coefficients: vec![],
        },
        Effect::Fir {
            coefficients: vec![f64::INFINITY],
        },
        Effect::Compressor {
            parameters: invalid_dynamics,
        },
    ] {
        let mut cfg = config(vec![e], 1, 48000);
        cfg.effects[0].enabled = false;
        assert!(matches!(
            Chain::compile(cfg, limits(1)),
            Err(CompileError::InvalidParameter { effect: 0, .. })
        ));
    }
}

#[test]
fn json_contract_roundtrips_and_rejects_unknown_or_incomplete_fields() {
    let text = r#"{"name":"correction","format":{"sampleRateHz":96000,"channels":["R","L","other"]},"effects":[{"name":"polarity","enabled":true,"effect":{"kind":"gain","gain":1.0,"invertPolarity":true}}]}"#;
    let cfg: ChainConfig = serde_json::from_str(text).unwrap();
    let value = serde_json::to_value(&cfg).unwrap();
    assert_eq!(value["effects"][0]["effect"]["invertPolarity"], true);
    let mut c = Chain::compile(cfg, limits(1)).unwrap();
    c.process(1, &[0.5, -0.5, 0.0]).unwrap();
    assert_eq!(c.output(), &[-0.5, 0.5, -0.0]);
    assert!(serde_json::from_str::<ChainConfig>(
        &text.replace("\"gain\":1.0", "\"gain\":1.0,\"automaticLimiter\":true")
    )
    .is_err());
    assert!(serde_json::from_str::<ChainConfig>(&text.replace("\"enabled\":true,", "")).is_err());
}

#[test]
fn expander_detector_attack_opens_and_release_closes_with_known_time_constants() {
    let mut p = dynamics();
    p.ratio = 2.0;
    p.attack_ms = 10.0;
    p.release_ms = 20.0;
    let mut c = chain(vec![Effect::Expander { parameters: p }], 1, 1000, 100);
    c.process(10, &[1.0; 10]).unwrap();
    let first_envelope = 1.0 - (-0.1f64).exp();
    close(c.output()[0], first_envelope / 0.1, 1e-14);
    close(c.output()[9], 1.0, 0.0);
    let hot_envelope = 1.0 - (-1.0f64).exp();
    c.process(100, &[0.01; 100]).unwrap();
    let released = 0.01 + (hot_envelope - 0.01) * (-5.0f64).exp();
    close(c.output()[0], 0.01, 0.0);
    close(c.output()[99], 0.01 * released / 0.1, 1e-15);
}

#[test]
fn bypass_snapshot_transition_is_immediate_resets_and_is_not_click_free() {
    let mut c = chain(
        vec![Effect::Gain {
            gain: 2.0,
            invert_polarity: false,
        }],
        1,
        48000,
        1,
    );
    c.process(1, &[0.25]).unwrap();
    let before = c.output()[0];
    c.set_bypassed(true);
    assert!(c.output().is_empty());
    c.process(1, &[0.25]).unwrap();
    close(before - c.output()[0], 0.25, 0.0);
    c.set_bypassed(false);
    c.process(1, &[0.25]).unwrap();
    close(c.output()[0], 0.5, 0.0);
}

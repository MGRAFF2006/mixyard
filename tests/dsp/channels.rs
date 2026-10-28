use super::*;

fn gain(value: f64) -> Effect {
    Effect::Gain {
        gain: value,
        invert_polarity: false,
    }
}
fn select(config: &mut ChainConfig, index: usize, lanes: &[usize]) {
    config.effects[index].channels = Some(
        lanes
            .iter()
            .map(|&i| config.format.channels[i].clone())
            .collect(),
    );
}
fn bits(xs: &[f64]) -> Vec<u64> {
    xs.iter().map(|x| x.to_bits()).collect()
}

#[test]
fn matrix_identity_swap_copy_mix_polarity_and_silence_use_original_format_lanes() {
    let input = [
        -0.0,
        f64::from_bits(1),
        0.5 + 2f64.powi(-50),
        -0.25,
        0.125,
        -0.5,
        -2f64.powi(-1000),
        0.75,
    ];
    for routes in [
        matrix(&[
            ("opaque-0", &[("opaque-0", 1.0)]),
            ("opaque-1", &[("opaque-1", 1.0)]),
            ("opaque-2", &[("opaque-2", 1.0)]),
            ("opaque-3", &[("opaque-3", 1.0)]),
        ]),
        // Out-of-order declarations must not reorder the interleaved format.
        matrix(&[
            ("opaque-3", &[("opaque-3", 1.0)]),
            ("opaque-0", &[("opaque-1", 1.0)]),
            ("opaque-2", &[("opaque-2", 1.0)]),
            ("opaque-1", &[("opaque-0", 1.0)]),
        ]),
    ] {
        let swap =
            matches!(&routes, Effect::ChannelMatrix { routes } if routes[0].output == "opaque-3");
        let mut c = chain(vec![routes], 4, 44117, 2);
        c.process(2, &input).unwrap();
        let expected = if swap {
            [
                input[1], input[0], input[2], input[3], input[5], input[4], input[6], input[7],
            ]
        } else {
            input
        };
        assert_eq!(bits(c.output()), bits(&expected));
        assert_eq!(
            c.config().format.channels,
            config(vec![], 4, 44117).format.channels
        );
    }
    let mut c = chain(
        vec![matrix(&[
            ("opaque-0", &[("opaque-3", 1.0)]), // copy original lane 3
            ("opaque-1", &[("opaque-3", -1.0)]),
            (
                "opaque-2",
                &[("opaque-0", 0.5), ("opaque-1", 2.0), ("opaque-2", -0.25)],
            ),
            ("opaque-3", &[]),
        ])],
        4,
        48000,
        3,
    );
    c.process(
        3,
        &[
            1.0, 2.0, 4.0, -8.0, 2.0, -1.0, 8.0, 0.5, -4.0, 0.25, -2.0, 16.0,
        ],
    )
    .unwrap();
    assert_eq!(
        c.output(),
        &[-8.0, 8.0, 3.5, 0.0, 0.5, -0.5, -3.0, 0.0, 16.0, -16.0, -1.0, 0.0]
    );
    // Listed-order Float64 summation: (1e16 + -1e16) + 1 = 1.
    let mut c = chain(
        vec![matrix(&[
            (
                "opaque-0",
                &[("opaque-0", 1.0), ("opaque-1", 1.0), ("opaque-2", 1.0)],
            ),
            ("opaque-1", &[]),
            ("opaque-2", &[]),
        ])],
        3,
        48000,
        1,
    );
    c.process(1, &[1e16, -1e16, 1.0]).unwrap();
    assert_eq!(c.output(), &[1.0, 0.0, 0.0]);
}

#[test]
fn every_existing_effect_selects_ordered_independent_lanes_and_leaves_other_bits_untouched() {
    let mut p = dynamics();
    p.attack_ms = 0.03;
    p.release_ms = 0.1;
    let effects = [
        gain(2.0),
        filter(Filter::LowPass, 12000.0, 0.0),
        Effect::Delay {
            frames: 2,
            wet: 0.75,
            dry: 0.25,
            feedback: -0.5,
        },
        Effect::Fir {
            coefficients: vec![0.5, -0.25, 0.125],
        },
        Effect::Compressor { parameters: p },
        Effect::Expander { parameters: p },
    ];
    let a = [1.0, -0.25, 0.5, 0.0, 0.125, -0.75, 0.0, 0.25];
    let b = [-0.125, 0.75, 0.0, 0.5, -1.0, 0.0, 0.25, -0.5];
    let untouched = [
        -0.0,
        f64::from_bits(1),
        -f64::from_bits(19),
        0.5 + 2f64.powi(-50),
        -2f64.powi(-1000),
        0.0,
        f64::MIN_POSITIVE,
        f64::MAX,
    ];
    let input: Vec<_> = (0..8)
        .flat_map(|i| [a[i], untouched[i], b[i], untouched[7 - i]])
        .collect();
    for effect in effects {
        let mut cfg = config(vec![effect.clone()], 4, 48000);
        select(&mut cfg, 0, &[2, 0]);
        let mut c = Chain::compile(cfg.clone(), limits(8)).unwrap();
        let mut full = Chain::compile(cfg, limits(8)).unwrap();
        let actual = render(&mut c, &input, 3);
        assert_eq!(bits(&actual), bits(&render(&mut full, &input, 8)));
        let expected_a = render(&mut chain(vec![effect.clone()], 1, 48000, 8), &a, 2);
        let expected_b = render(&mut chain(vec![effect], 1, 48000, 8), &b, 5);
        for i in 0..8 {
            assert_eq!(
                bits(&actual[i * 4..i * 4 + 4]),
                bits(&[expected_a[i], untouched[i], expected_b[i], untouched[7 - i]])
            );
        }
        for transition in 0..4 {
            c.process(1, &[1.0, -0.0, -0.5, f64::from_bits(1)]).unwrap();
            match transition {
                0 => c.reset(),
                1 => {
                    c.set_bypassed(true);
                    c.process(1, &[-0.0; 4]).unwrap();
                    assert_eq!(bits(c.output()), bits(&[-0.0; 4]));
                    c.set_bypassed(false);
                }
                2 => {
                    assert_eq!(
                        c.process(1, &[f64::NAN; 4]),
                        Err(ProcessError::NonFiniteInput { sample: 0 })
                    );
                }
                _ => {
                    assert_eq!(c.process(0, &[]), Err(ProcessError::FrameCount));
                }
            }
            assert!(c.output().is_empty());
            c.process(8, &[0.0; 32]).unwrap();
            assert!(c.output().iter().all(|x| *x == 0.0));
        }
    }
}

#[test]
fn ordered_instances_allow_different_delay_feedback_fir_filter_and_dynamics_parameters() {
    let pairs = [
        (gain(2.0), gain(0.25)),
        (
            Effect::Delay {
                frames: 1,
                wet: 1.0,
                dry: 0.0,
                feedback: 0.5,
            },
            Effect::Delay {
                frames: 3,
                wet: 0.5,
                dry: 0.5,
                feedback: -0.25,
            },
        ),
        (
            Effect::Fir {
                coefficients: vec![1.0, 0.5],
            },
            Effect::Fir {
                coefficients: vec![0.25, 0.0, -0.75],
            },
        ),
        (
            filter(Filter::LowPass, 12000.0, 0.0),
            filter(Filter::HighPass, 6000.0, 0.0),
        ),
        (
            Effect::Compressor {
                parameters: dynamics(),
            },
            Effect::Compressor {
                parameters: Dynamics {
                    threshold_db: -6.0,
                    ratio: 2.0,
                    ..dynamics()
                },
            },
        ),
        (
            Effect::Expander {
                parameters: dynamics(),
            },
            Effect::Expander {
                parameters: Dynamics {
                    threshold_db: -3.0,
                    ratio: 2.0,
                    ..dynamics()
                },
            },
        ),
    ];
    let a = [1.0, -0.5, 0.125, 0.0, 0.0, 0.0, 0.0, 0.0];
    let b = [0.25, 0.5, -1.0, 0.25, 0.0, 0.0, 0.0, 0.0];
    let input: Vec<_> = (0..8).flat_map(|i| [a[i], b[i], -0.0]).collect();
    for (left, right) in pairs {
        let mut cfg = config(vec![left.clone(), right.clone()], 3, 48000);
        select(&mut cfg, 0, &[0]);
        select(&mut cfg, 1, &[1]);
        let mut c = Chain::compile(cfg, limits(8)).unwrap();
        let out = render(&mut c, &input, 3);
        let ea = render(&mut chain(vec![left], 1, 48000, 8), &a, 8);
        let eb = render(&mut chain(vec![right], 1, 48000, 8), &b, 8);
        for i in 0..8 {
            assert_eq!(bits(&out[i * 3..i * 3 + 3]), bits(&[ea[i], eb[i], -0.0]));
        }
    }
    // Independent delays are not serial on one lane: metadata deliberately sums.
    let mut cfg = config(
        vec![
            Effect::Delay {
                frames: 1,
                wet: 1.0,
                dry: 0.0,
                feedback: 0.0,
            },
            Effect::Delay {
                frames: 3,
                wet: 1.0,
                dry: 0.0,
                feedback: 0.0,
            },
        ],
        2,
        48000,
    );
    select(&mut cfg, 0, &[0]);
    select(&mut cfg, 1, &[1]);
    let mut c = Chain::compile(cfg, limits(4)).unwrap();
    c.process(4, &[1.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])
        .unwrap();
    assert_eq!(c.output(), &[0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 2.0]);
    assert_eq!(c.admission().explicit_delay_frames, 4);
    assert_eq!(c.admission().tail, Tail::Finite { frames: 4 });
}

#[test]
fn selectors_and_matrix_routes_reject_malformed_configs_even_disabled() {
    let good = matrix(&[("opaque-0", &[("opaque-1", 1.0)]), ("opaque-1", &[])]);
    let mut invalid = Vec::new();
    for ids in [vec![], vec!["missing"], vec!["opaque-0", "opaque-0"]] {
        let mut cfg = config(vec![gain(1.0)], 2, 48000);
        cfg.effects[0].channels = Some(ids.into_iter().map(str::to_owned).collect());
        invalid.push(cfg);
    }
    let mut cfg = config(vec![good.clone()], 2, 48000);
    select(&mut cfg, 0, &[0]);
    invalid.push(cfg);
    for e in [
        matrix(&[]),
        matrix(&[("opaque-0", &[])]),
        matrix(&[("opaque-0", &[]), ("opaque-0", &[])]),
        matrix(&[("opaque-0", &[]), ("missing", &[])]),
        matrix(&[("opaque-0", &[("missing", 1.0)]), ("opaque-1", &[])]),
        matrix(&[
            ("opaque-0", &[("opaque-1", 1.0), ("opaque-1", 2.0)]),
            ("opaque-1", &[]),
        ]),
        matrix(&[
            ("opaque-0", &[("opaque-1", f64::INFINITY)]),
            ("opaque-1", &[]),
        ]),
        matrix(&[("opaque-0", &[("opaque-1", f64::NAN)]), ("opaque-1", &[])]),
    ] {
        invalid.push(config(vec![e], 2, 48000));
    }
    for cfg in invalid {
        for enabled in [true, false] {
            let mut cfg = cfg.clone();
            cfg.effects[0].enabled = enabled;
            assert!(matches!(
                Chain::compile(cfg, limits(2)),
                Err(CompileError::InvalidParameter { effect: 0, .. })
            ));
        }
    }
}

#[test]
fn matrix_overflow_is_atomic_and_resets_all_selected_history() {
    for inputs in [
        vec![("opaque-0", 2.0)],
        vec![("opaque-0", 1.0), ("opaque-1", 1.0)],
        vec![("opaque-0", 2.0), ("opaque-1", -2.0)],
    ] {
        let mut cfg = config(
            vec![
                Effect::Delay {
                    frames: 1,
                    wet: 1.0,
                    dry: 1.0,
                    feedback: 0.5,
                },
                matrix(&[("opaque-1", &[("opaque-1", 1.0)]), ("opaque-0", &inputs)]),
            ],
            2,
            48000,
        );
        select(&mut cfg, 0, &[0]);
        let mut c = Chain::compile(cfg, limits(3)).unwrap();
        assert_eq!(
            c.process(3, &[0.25, 0.5, 0.5, 0.25, f64::MAX, f64::MAX]),
            Err(ProcessError::NonFiniteResult { effect: 1 })
        );
        assert!(c.output().is_empty());
        assert!(c.meters().iter().all(|m| m.peak == 0.0 && m.rms == 0.0));
        c.process(3, &[0.0; 6]).unwrap();
        assert_eq!(c.output(), &[0.0; 6]);
    }
}

#[test]
fn selection_and_matrix_json_are_explicit_and_legacy_json_stays_compatible() {
    let legacy = r#"{"name":"legacy","format":{"sampleRateHz":48000,"channels":["opaque-0","opaque-1"]},"effects":[{"name":"gain","enabled":true,"effect":{"kind":"gain","gain":2.0,"invertPolarity":false}}]}"#;
    let cfg: ChainConfig = serde_json::from_str(legacy).unwrap();
    assert!(cfg.effects[0].channels.is_none());
    assert_eq!(
        serde_json::to_value(&cfg).unwrap(),
        serde_json::from_str::<serde_json::Value>(legacy).unwrap()
    );
    let mut c = Chain::compile(cfg, limits(1)).unwrap();
    c.process(1, &[0.25, -0.5]).unwrap();
    assert_eq!(c.output(), &[0.5, -1.0]);
    let mut cfg = config(
        vec![
            gain(0.5),
            matrix(&[
                ("opaque-1", &[("opaque-0", 1.0)]),
                ("opaque-0", &[("opaque-1", -1.0)]),
            ]),
        ],
        2,
        44117,
    );
    select(&mut cfg, 0, &[1]);
    let value = serde_json::to_value(&cfg).unwrap();
    assert_eq!(
        value["effects"][0]["channels"],
        serde_json::json!(["opaque-1"])
    );
    assert!(value["effects"][1].get("channels").is_none());
    let decoded: ChainConfig = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(&decoded).unwrap(), value);
    let mut c = Chain::compile(decoded, limits(1)).unwrap();
    c.process(1, &[0.25, -0.5]).unwrap();
    assert_eq!(c.output(), &[0.25, 0.25]);
    let null = legacy.replace("\"enabled\":true,", "\"enabled\":true,\"channels\":null,");
    assert!(
        serde_json::from_str::<ChainConfig>(&null).unwrap().effects[0]
            .channels
            .is_none()
    );
    let unknown = serde_json::to_string(&cfg)
        .unwrap()
        .replace("\"gain\":-1.0", "\"gain\":-1.0,\"normalize\":true");
    assert!(serde_json::from_str::<ChainConfig>(&unknown).is_err());
}

#[test]
fn admission_counts_selected_state_and_retained_selector_and_matrix_capacities() {
    let delay = Effect::Delay {
        frames: 500,
        wet: 1.0,
        dry: 0.0,
        feedback: 0.0,
    };
    let all = Chain::compile(config(vec![delay.clone()], 4, 48000), limits(2))
        .unwrap()
        .admission();
    let mut cfg = config(vec![delay], 4, 48000);
    select(&mut cfg, 0, &[2]);
    let selected = Chain::compile(cfg, limits(2)).unwrap().admission();
    assert!(all.memory_bytes > selected.memory_bytes + 10000);
    assert!(all.work_units_per_block > selected.work_units_per_block);

    for enabled in [true, false] {
        let mut cfg = config(
            vec![
                gain(1.0),
                matrix(&[("opaque-0", &[("opaque-1", 0.5)]), ("opaque-1", &[])]),
            ],
            2,
            48000,
        );
        select(&mut cfg, 0, &[1]);
        for e in &mut cfg.effects {
            e.enabled = enabled;
        }
        // Normalize clone capacities before measuring the explicit reserve delta.
        let mut cfg = cfg.clone();
        let base = Chain::compile(cfg.clone(), limits(2)).unwrap().admission();
        let mut extra = 0;
        let ids = cfg.effects[0].channels.as_mut().unwrap();
        let old = ids.capacity();
        ids.reserve_exact(17);
        extra += (ids.capacity() - old) * std::mem::size_of::<String>();
        let old = ids[0].capacity();
        ids[0].reserve_exact(21);
        extra += ids[0].capacity() - old;
        if let Effect::ChannelMatrix { routes } = &mut cfg.effects[1].effect {
            let old = routes.capacity();
            routes.reserve_exact(13);
            extra += (routes.capacity() - old) * std::mem::size_of::<MatrixRoute>();
            let r = &mut routes[0];
            let old = r.output.capacity();
            r.output.reserve_exact(19);
            extra += r.output.capacity() - old;
            let old = r.inputs.capacity();
            r.inputs.reserve_exact(11);
            extra += (r.inputs.capacity() - old) * std::mem::size_of::<MatrixInput>();
            let old = r.inputs[0].channel.capacity();
            r.inputs[0].channel.reserve_exact(23);
            extra += r.inputs[0].channel.capacity() - old;
        }
        let admitted = Chain::compile(cfg, limits(2)).unwrap().admission();
        assert_eq!(admitted.memory_bytes, base.memory_bytes + extra);
        assert_eq!(admitted.work_units_per_block, base.work_units_per_block);
        for resource in ["memoryBytes", "workUnitsPerBlock"] {
            let mut cfg = config(
                vec![
                    gain(1.0),
                    matrix(&[("opaque-0", &[("opaque-1", 0.5)]), ("opaque-1", &[])]),
                ],
                2,
                48000,
            );
            select(&mut cfg, 0, &[1]);
            for e in &mut cfg.effects {
                e.enabled = enabled;
            }
            let mut l = limits(2);
            if resource == "memoryBytes" {
                l.max_memory_bytes = base.memory_bytes - 1;
            } else {
                l.max_work_units_per_block = base.work_units_per_block - 1;
            }
            assert!(
                matches!(Chain::compile(cfg,l),Err(CompileError::Budget{resource:r,..}) if r == resource)
            );
        }
    }
}

#[test]
fn explicit_unity_disabled_matrix_and_arbitrary_identity_selectors_preserve_finite_bits() {
    let mut cfg = config(
        vec![
            gain(1.0),
            matrix(&[("opaque-0", &[("opaque-1", -3.0)]), ("opaque-1", &[])]),
        ],
        2,
        48000,
    );
    cfg.format.channels = vec!["Ambisonic:W/1".into(), "rear left (raw)".into()];
    cfg.effects[0].channels = Some(vec!["rear left (raw)".into()]);
    cfg.effects[1].effect = matrix(&[
        ("rear left (raw)", &[]),
        ("Ambisonic:W/1", &[("rear left (raw)", -3.0)]),
    ]);
    cfg.effects[1].enabled = false;
    let mut c = Chain::compile(cfg, limits(3)).unwrap();
    let input = [
        f64::from_bits(1),
        -0.0,
        0.5 + 2f64.powi(-50),
        -f64::from_bits(11),
        f64::MAX,
        -2f64.powi(-1000),
    ];
    c.process(3, &input).unwrap();
    assert_eq!(bits(c.output()), bits(&input));
}

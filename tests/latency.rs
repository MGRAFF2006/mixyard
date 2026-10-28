use mixyard::{
    dsp::{Chain, ChainConfig},
    latency::{self, Error, Feature, Probe},
};
use serde_json::json;

fn chain(effects: serde_json::Value) -> Chain {
    let config: ChainConfig = serde_json::from_value(json!({"name":"Measured chain",
        "format":{"sampleRateHz":48000,"channels":[" left ","aux"]},"effects":effects}))
    .unwrap();
    Chain::compile(
        config,
        mixyard::dsp::Limits {
            max_block_frames: 4,
            max_channels: 8,
            max_effects: 16,
            max_memory_bytes: 1024 * 1024,
            max_work_units_per_block: 1_000_000,
        },
    )
    .unwrap()
}
fn probe() -> Probe {
    Probe {
        block_frames: 4,
        observation_frames: 16,
        reference_channels: vec![" left ".into(), "aux".into()],
        amplitude: 1.0,
        detection_threshold: 0.0,
        bypassed: false,
    }
}
fn limits() -> latency::Limits {
    latency::Limits {
        max_observation_frames: 1024,
        max_work_units: 1_000_000,
        max_buffer_bytes: 1024 * 1024,
    }
}
fn delays() -> serde_json::Value {
    json!([
        {"name":"Left alignment","enabled":true,"channels":[" left "],"effect":{"kind":"delay","frames":3,"wet":1,"dry":0,"feedback":0}},
        {"name":"Aux alignment","enabled":true,"channels":["aux"],"effect":{"kind":"delay","frames":7,"wet":1,"dry":0,"feedback":0}}
    ])
}

#[test]
fn actual_impulse_meter_and_applied_compensation_align_independent_lanes() {
    let report = latency::measure(chain(delays()), probe(), limits()).unwrap();
    assert!(report.response_window_complete);
    assert_eq!(report.channels[0].first_response_frame, Some(3));
    assert_eq!(report.channels[1].strongest_peak_frame, Some(7));
    let selected = vec![" left ".into(), "aux".into()];
    let plan = latency::align(&report, &selected, Feature::FirstResponse, 100).unwrap();
    assert_eq!(plan.target_frame, 7);
    assert_eq!(
        plan.delays.iter().map(|d| d.frames).collect::<Vec<_>>(),
        [4, 0]
    );
    let mut effects = delays();
    effects
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"Apply measured alignment","enabled":true,
        "channels":[" left "],"effect":{"kind":"delay","frames":4,"wet":1,"dry":0,"feedback":0}}));
    let aligned = latency::measure(chain(effects), probe(), limits()).unwrap();
    assert!(aligned
        .channels
        .iter()
        .all(|c| c.first_response_frame == Some(7)));
    assert!(aligned
        .channels
        .iter()
        .all(|c| c.strongest_peak_frame == Some(7)));
}

#[test]
fn dry_wet_onset_peak_and_feedback_tail_are_different_observations() {
    let report = latency::measure(chain(json!([
        {"name":"Dry and echo","enabled":true,"effect":{"kind":"delay","frames":5,"wet":0.8,"dry":0.2,"feedback":0.5}}
    ])), probe(), limits()).unwrap();
    assert!(!report.response_window_complete);
    for channel in &report.channels {
        assert_eq!(channel.first_response_frame, Some(0));
        assert_eq!(channel.strongest_peak_frame, Some(5));
        assert_eq!(channel.peak_magnitude, 0.8);
    }
    let ids = vec![" left ".into(), "aux".into()];
    assert!(latency::align(&report, &ids, Feature::FirstResponse, 100).is_ok());
    assert_eq!(
        latency::align(&report, &ids, Feature::StrongestPeak, 100).unwrap_err(),
        Error::IncompletePeakWindow
    );
}

#[test]
fn finite_fir_response_and_observation_boundary_are_not_tail_truncation_claims() {
    let effects = json!([{"name":"Delayed IR","enabled":true,"effect":{"kind":"fir","coefficients":[0,0,0,0,0,0,0,1]}}]);
    let mut p = probe();
    p.observation_frames = 7;
    let partial = latency::measure(chain(effects.clone()), p, limits()).unwrap();
    assert!(!partial.response_window_complete);
    assert_eq!(partial.channels[0].first_response_frame, None);
    let full = latency::measure(chain(effects), probe(), limits()).unwrap();
    assert!(full.response_window_complete);
    assert_eq!(full.channels[0].first_response_frame, Some(7));
}

#[test]
fn explicit_reference_silence_threshold_and_bypass_do_not_invent_zero_latency() {
    let mut p = probe();
    p.reference_channels = vec!["aux".into()];
    let report = latency::measure(chain(json!([])), p, limits()).unwrap();
    assert_eq!(report.channels[0].first_response_frame, None);
    assert_eq!(report.channels[1].first_response_frame, Some(0));
    assert!(matches!(
        latency::align(
            &report,
            &[" left ".into(), "aux".into()],
            Feature::FirstResponse,
            100
        ),
        Err(Error::NoResponse { .. })
    ));
    let mut p = probe();
    p.detection_threshold = 2.0;
    let quiet = latency::measure(chain(json!([])), p, limits()).unwrap();
    assert_eq!(quiet.channels[0].first_response_frame, None);
    assert_eq!(quiet.channels[0].peak_magnitude, 1.0);
    let mut p = probe();
    p.bypassed = true;
    let bypass = latency::measure(chain(delays()), p, limits()).unwrap();
    assert!(bypass
        .channels
        .iter()
        .all(|c| c.first_response_frame == Some(0)));
}

#[test]
fn bad_probe_resource_limits_and_alignment_selection_fail_explicitly() {
    for p in [
        Probe {
            observation_frames: 0,
            ..probe()
        },
        Probe {
            amplitude: f64::NAN,
            ..probe()
        },
        Probe {
            reference_channels: vec!["missing".into()],
            ..probe()
        },
        Probe {
            reference_channels: vec!["aux".into(), "aux".into()],
            ..probe()
        },
    ] {
        assert!(matches!(
            latency::measure(chain(json!([])), p, limits()),
            Err(Error::InvalidProbe)
        ));
    }
    assert!(matches!(
        latency::measure(
            chain(json!([])),
            probe(),
            latency::Limits {
                max_buffer_bytes: 0,
                ..limits()
            }
        ),
        Err(Error::Budget)
    ));
    assert!(matches!(
        latency::measure(
            chain(json!([])),
            probe(),
            latency::Limits {
                max_work_units: 0,
                ..limits()
            }
        ),
        Err(Error::Budget)
    ));
    let report = latency::measure(chain(delays()), probe(), limits()).unwrap();
    assert_eq!(
        latency::align(
            &report,
            &["aux".into(), "aux".into()],
            Feature::FirstResponse,
            10
        )
        .unwrap_err(),
        Error::InvalidSelection
    );
    assert_eq!(
        latency::align(
            &report,
            &["aux".into(), " left ".into()],
            Feature::FirstResponse,
            2
        )
        .unwrap_err(),
        Error::AlignmentBudget
    );
}

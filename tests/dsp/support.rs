use mixyard::dsp::*;

pub fn limits(frames: usize) -> Limits {
    Limits {
        max_block_frames: frames,
        max_channels: 64,
        max_effects: 100,
        max_memory_bytes: 64 * 1024 * 1024,
        max_work_units_per_block: 100_000_000,
    }
}
pub fn config(effects: Vec<Effect>, channels: usize, rate: u32) -> ChainConfig {
    ChainConfig {
        name: "test".into(),
        format: Format {
            sample_rate_hz: rate,
            channels: (0..channels).map(|i| format!("opaque-{i}")).collect(),
        },
        effects: effects
            .into_iter()
            .enumerate()
            .map(|(i, effect)| EffectConfig {
                name: format!("effect-{i}"),
                enabled: true,
                effect,
            })
            .collect(),
    }
}
pub fn chain(effects: Vec<Effect>, channels: usize, rate: u32, frames: usize) -> Chain {
    Chain::compile(config(effects, channels, rate), limits(frames)).unwrap()
}
pub fn dynamics() -> Dynamics {
    Dynamics {
        threshold_db: -20.0,
        ratio: 4.0,
        knee_db: 0.0,
        attack_ms: 0.0,
        release_ms: 0.0,
        makeup_db: 0.0,
        range_db: 80.0,
    }
}
pub fn filter(kind: Filter, frequency: f64, gain: f64) -> Effect {
    Effect::Biquad {
        filter: kind,
        frequency_hz: frequency,
        q: std::f64::consts::FRAC_1_SQRT_2,
        gain_db: gain,
    }
}
pub fn close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "actual {actual:e}, expected {expected:e}, tolerance {tolerance:e}"
    );
}
pub fn render(chain: &mut Chain, input: &[f64], block: usize) -> Vec<f64> {
    let channels = chain.config().format.channels.len();
    let mut output = Vec::new();
    for chunk in input.chunks(block * channels) {
        chain.process(chunk.len() / channels, chunk).unwrap();
        output.extend_from_slice(chain.output());
    }
    output
}

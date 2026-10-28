use mixyard::audio::{Limits, Pcm, Sample, TypedProcessor};
use mixyard::{AudioFormat, Config, Inventory};

pub fn example() -> (Config, Inventory) {
    (
        serde_json::from_str(include_str!("../../examples/config.json")).unwrap(),
        serde_json::from_str(include_str!("../../examples/inventory.json")).unwrap(),
    )
}

pub fn limits() -> Limits {
    Limits {
        max_frames: 32,
        max_sources: 128,
        max_mixes: 128,
        max_sends: 512,
        max_outputs: 256,
        max_buffer_samples: 32 * 2 * (128 * 2 + 256),
    }
}

pub fn pcm(channels: u16, samples: &[f32]) -> Option<Pcm<'_>> {
    Some(Pcm {
        format: AudioFormat {
            channels,
            sample_rate_hz: 48_000,
        },
        samples,
    })
}

pub fn source<S: Sample>(p: &TypedProcessor<S>, id: &str) -> usize {
    p.plan()
        .sources
        .iter()
        .position(|s| s.source == id)
        .unwrap()
}

pub fn mix<'a, S: Sample>(p: &'a TypedProcessor<S>, id: &str) -> &'a [S] {
    p.mix_samples(p.plan().mixes.iter().position(|m| m.id == id).unwrap())
        .unwrap()
}

pub fn output<'a, S: Sample>(p: &'a TypedProcessor<S>, id: &str) -> &'a [S] {
    p.output_samples(p.plan().outputs.iter().position(|o| o.id == id).unwrap())
        .unwrap()
}

pub fn silent<S: Sample>(p: &TypedProcessor<S>) {
    for i in 0..p.plan().mixes.len() {
        assert!(p
            .mix_samples(i)
            .unwrap()
            .iter()
            .all(|s| s.widen().to_bits() == 0));
    }
    for i in 0..p.plan().outputs.len() {
        assert!(p
            .output_samples(i)
            .unwrap()
            .iter()
            .all(|s| s.widen().to_bits() == 0));
    }
}

/// A policy-admitted extreme block that can overflow Float64 accumulation.
#[allow(dead_code)]
pub fn overflow_example() -> (Config, Inventory) {
    use mixyard::{Observation, Origin, Selector, Send, Source};
    let (mut config, mut inventory) = example();
    // Seventeen 1/17 sends pass the planner's headroom check, but rounding their
    // maximum-finite contributions can overflow f64. No infinity may escape.
    config.sends.retain(|s| s.mix != "desk-a");
    for i in 0..17 {
        let id = format!("overflow-{i:02}");
        let selector = Selector::App {
            application_id: id.clone(),
        };
        config.sources.push(Source {
            id: id.clone(),
            selector: selector.clone(),
        });
        inventory.sources.push(Observation {
            observed_id: id.clone(),
            selector,
            origin: Origin::External,
            format: Some(AudioFormat {
                channels: 2,
                sample_rate_hz: 48_000,
            }),
        });
        config.sends.push(Send {
            source: id,
            mix: "desk-a".into(),
            gain: 1.0 / 17.0,
            mute: false,
        });
    }
    (config, inventory)
}

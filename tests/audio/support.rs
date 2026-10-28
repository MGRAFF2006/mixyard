use mixyard::audio::{Limits, Pcm, Processor};
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

pub fn source(p: &Processor, id: &str) -> usize {
    p.plan()
        .sources
        .iter()
        .position(|s| s.source == id)
        .unwrap()
}

pub fn mix<'a>(p: &'a Processor, id: &str) -> &'a [f32] {
    p.mix_samples(p.plan().mixes.iter().position(|m| m.id == id).unwrap())
        .unwrap()
}

pub fn output<'a>(p: &'a Processor, id: &str) -> &'a [f32] {
    p.output_samples(p.plan().outputs.iter().position(|o| o.id == id).unwrap())
        .unwrap()
}

pub fn silent(p: &Processor) {
    for i in 0..p.plan().mixes.len() {
        assert!(p.mix_samples(i).unwrap().iter().all(|s| s.to_bits() == 0));
    }
    for i in 0..p.plan().outputs.len() {
        assert!(p
            .output_samples(i)
            .unwrap()
            .iter()
            .all(|s| s.to_bits() == 0));
    }
}

#[path = "audio/support.rs"]
#[allow(dead_code)]
mod support;
use mixyard::audio::{Pcm, Processor64, RenderError};
use mixyard::AudioFormat;
use support::*;

#[test]
fn owned_source_bank_and_mix_stage_reuse_authoritative_output_gains_and_exclusions() {
    let (config, inventory) = example();
    let mut p = Processor64::compile(&config, &inventory, 48_000, limits()).unwrap();
    let mut bank = p.input_bank(256).unwrap();
    bank.set(source(&p, "mic"), 1, Some(&[0.5])).unwrap();
    bank.set(source(&p, "spotify"), 1, Some(&[0.25, -0.5]))
        .unwrap();
    bank.set(source(&p, "vlc"), 1, Some(&[-0.5, 0.125]))
        .unwrap();
    bank.set(source(&p, "return"), 1, Some(&[0.125, -0.25]))
        .unwrap();
    p.render_bank(1, &bank).unwrap();
    assert_eq!(output(&p, "call-input"), [0.5, 0.5]);
    assert_eq!(output(&p, "record-input"), [0.34375, 0.0625]);
    let call = p.plan().mixes.iter().position(|m| m.id == "call").unwrap();
    let mut mixes = p.mix_bank(512).unwrap();
    mixes.set(call, 1, Some(&[0.125])).unwrap();
    p.apply_mix_bank(&mixes).unwrap();
    assert_eq!(output(&p, "call-input"), [0.125, 0.125]);
    assert_eq!(output(&p, "record-input"), [0.34375, 0.0625]);
    assert_eq!(p.apply_mix_bank(&mixes), Err(RenderError::MixStageClosed));
    silent(&p);
}

#[test]
fn bad_bank_or_mix_payload_discards_old_sources_and_all_partial_outputs() {
    let (config, inventory) = example();
    let mut p = Processor64::compile(&config, &inventory, 48_000, limits()).unwrap();
    assert!(p.input_bank(1).is_err());
    assert!(p.mix_bank(1).is_err());
    let mut bank = p.input_bank(256).unwrap();
    bank.set(source(&p, "mic"), 1, Some(&[0.5])).unwrap();
    p.render_bank(1, &bank).unwrap();
    assert_eq!(output(&p, "call-input"), [0.5, 0.5]);
    assert!(bank
        .set(source(&p, "spotify"), 1, Some(&[f64::NAN, 0.0]))
        .is_err());
    p.render_bank(1, &bank).unwrap();
    silent(&p);
    bank.set(source(&p, "mic"), 1, Some(&[0.5])).unwrap();
    p.render_bank(1, &bank).unwrap();
    let mut replacements = vec![None; p.plan().mixes.len()];
    let call = p.plan().mixes.iter().position(|m| m.id == "call").unwrap();
    replacements[call] = Some(Pcm {
        format: AudioFormat {
            channels: 1,
            sample_rate_hz: 48_000,
        },
        samples: &[f64::NAN],
    });
    assert_eq!(
        p.apply_mix_results(&replacements),
        Err(RenderError::MixNonFinite {
            mix: call,
            sample: 0
        })
    );
    silent(&p);
    p.render_bank(1, &bank).unwrap();
    assert_eq!(output(&p, "call-input"), [0.5, 0.5]);
}

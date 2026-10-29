//! Synthetic common-clock host integration; never opens a native endpoint.
#[path = "audio/support.rs"]
#[allow(dead_code)]
mod support;

use mixyard::audio::{clock, Pcm, Precision, Processor64};
use mixyard::dsp::{self, live};
use mixyard::runtime::worker;
use mixyard::AudioFormat;

#[test]
fn admitted_clock_core_dsp_and_consumers_keep_mix_minus_and_siblings_separate() {
    let (config, inventory) = support::example();
    let mut processor =
        Processor64::compile(&config, &inventory, 48_000, support::limits()).unwrap();
    let channels = vec!["FL".to_owned(), "FR".to_owned()];
    let source = [0.25 + 2f64.powi(-40), -0.5, -0.125, 0.375];
    let binding = clock::Binding {
        generation: 8,
        revision: config.revision,
    };
    let format = clock::Format {
        sample_rate_hz: 48_000,
        channels: channels.clone(),
    };
    // This fixture deliberately uses one synthetic clock. Equal nominal rates
    // on physical devices would not authorize this bypass or engine mapping.
    let mut adapter = clock::RateAdapter::<f64>::new(
        format.clone(),
        format,
        binding,
        7,
        7,
        clock::Method::Bypass,
        clock::Limits {
            max_channels: 2,
            max_channel_label_bytes: 8,
            max_input_frames: 2,
            max_output_frames: 2,
            capacity_frames: 4,
            max_buffer_bytes: 1024,
            max_correction_ppm: 0,
        },
    )
    .unwrap();
    adapter
        .push(
            100,
            clock::Stamp {
                binding,
                clock_id: 7,
                sequence: 0,
                first_frame: 0,
                captured_ns: 100,
                deadline_ns: 200,
            },
            2,
            &source,
        )
        .unwrap();
    let aligned = adapter.render(100, binding, 2).unwrap();
    let mut inputs = vec![None; processor.plan().sources.len()];
    inputs[support::source(&processor, "spotify")] = Some(Pcm {
        format: AudioFormat {
            channels: 2,
            sample_rate_hz: 48_000,
        },
        samples: adapter.output(),
    });
    // A call-return block is present, but the authoritative policy's Call
    // exclusions still prevent it and the unshared application entering Call.
    inputs[support::source(&processor, "return")] = Some(Pcm {
        format: AudioFormat {
            channels: 2,
            sample_rate_hz: 48_000,
        },
        samples: &[0.75; 4],
    });
    processor.render(2, &inputs).unwrap();
    assert_eq!(support::output(&processor, "call-input"), &[0.0; 4]);

    let dsp_format = dsp::Format {
        sample_rate_hz: 48_000,
        channels: channels.clone(),
    };
    let mut effect = live::LiveChain::compile(
        dsp::ChainConfig {
            name: "destination-only fade".into(),
            format: dsp_format.clone(),
            effects: vec![],
        },
        dsp::Limits {
            max_block_frames: 2,
            max_channels: 2,
            max_effects: 1,
            max_memory_bytes: 8192,
            max_work_units_per_block: 8192,
        },
        2,
        live::Binding {
            attachment: live::Attachment::Output(3),
            generation: 8,
            revision: 1,
        },
        live::Controls::UNITY,
    )
    .unwrap();
    let dsp_revision = effect
        .update(
            effect.binding(),
            2,
            live::Controls {
                gain: 0.0,
                wet: 1.0,
            },
            2,
        )
        .unwrap();
    effect
        .process(
            dsp_revision,
            &dsp_format,
            2,
            support::output(&processor, "speaker-c"),
        )
        .unwrap();

    let worker_binding = worker::Binding {
        generation: binding.generation,
        revision: binding.revision,
    };
    let worker_format = worker::Format {
        sample_rate_hz: 48_000,
        channels,
        precision: Precision::Float64,
    };
    let mut raw =
        worker::Queue::<f64>::new(worker_format.clone(), worker_binding, 7, 1, 2, 128).unwrap();
    let mut processed =
        worker::Queue::<f64>::new(worker_format, worker_binding, 7, 1, 2, 128).unwrap();
    let stamp = worker::Stamp {
        binding: worker_binding,
        clock_id: aligned.stamp.clock_id,
        sequence: aligned.stamp.sequence,
        first_frame: aligned.stamp.first_frame,
        captured_ns: aligned.stamp.captured_ns,
        deadline_ns: aligned.stamp.deadline_ns,
    };
    raw.push(100, stamp, 2, support::output(&processor, "speaker-a"))
        .unwrap();
    processed.push(100, stamp, 2, effect.output()).unwrap();
    let mut destination = [9.0; 4];
    raw.pop(101, worker_binding, &mut destination).unwrap();
    assert_eq!(destination.map(f64::to_bits), source.map(f64::to_bits));
    processed
        .pop(101, worker_binding, &mut destination)
        .unwrap();
    assert_eq!(destination, [source[0] * 0.25, -0.125, 0.0, 0.0]);
    processed.revoke();
    assert!(
        raw.active(),
        "a destination fade/revoke must not affect its sibling"
    );
    assert_eq!(
        processed.pop(102, worker_binding, &mut destination),
        Err(worker::Error::Inactive)
    );
    assert_eq!(destination, [0.0; 4]);
}

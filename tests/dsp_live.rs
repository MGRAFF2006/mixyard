#[path = "dsp/support.rs"]
#[allow(dead_code)]
mod support;

use mixyard::dsp::live::{Attachment, Binding, Controls, Error, LiveChain};
use mixyard::dsp::{ChainConfig, CompileError, Effect, Format, Limits, ProcessError};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::Instant;
use support::*;

struct Tracked;
thread_local! { static HEAP: Cell<Option<usize>> = const { Cell::new(None) }; }
fn record() {
    let _ = HEAP.try_with(|n| {
        if let Some(count) = n.get() {
            n.set(Some(count + 1));
        }
    });
}
unsafe impl GlobalAlloc for Tracked {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record();
        System.alloc(layout)
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record();
        System.alloc_zeroed(layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record();
        System.realloc(ptr, layout, size)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        record();
        System.dealloc(ptr, layout)
    }
}
#[global_allocator]
static ALLOCATOR: Tracked = Tracked;

fn binding(attachment: Attachment) -> Binding {
    Binding {
        attachment,
        generation: 8,
        revision: 1,
    }
}
fn host(config: ChainConfig, attachment: Attachment, frames: usize) -> LiveChain {
    LiveChain::compile(
        config,
        limits(frames),
        1024,
        binding(attachment),
        Controls::UNITY,
    )
    .unwrap()
}
fn gain(value: f64) -> Effect {
    Effect::Gain {
        gain: value,
        invert_polarity: false,
    }
}
fn delay() -> Effect {
    Effect::Delay {
        frames: 2,
        dry: 0.0,
        wet: 1.0,
        feedback: 0.0,
    }
}

#[test]
fn source_mix_and_per_output_processing_preserve_independent_paths() {
    let format = config(vec![], 2, 192000).format;
    let mut source = host(config(vec![gain(2.0)], 2, 192000), Attachment::Source(9), 4);
    let mut mix = host(config(vec![gain(0.5)], 2, 192000), Attachment::Mix(9), 4);
    let mut correction = config(vec![gain(0.5)], 2, 192000);
    correction.effects[0].channels = Some(vec!["opaque-1".into()]);
    let mut a = host(correction, Attachment::Output(1), 4);
    let mut b = host(config(vec![], 2, 192000), Attachment::Output(2), 4);
    source
        .process(source.binding(), &format, 1, &[0.25, 0.5])
        .unwrap();
    // The actual graph host supplies the independently authorized summed sends.
    let sum = [source.output()[0] + 0.5, source.output()[1]];
    mix.process(mix.binding(), &format, 1, &sum).unwrap();
    a.process(a.binding(), &format, 1, mix.output()).unwrap();
    b.process(b.binding(), &format, 1, mix.output()).unwrap();
    assert_eq!(source.output(), &[0.5, 1.0]);
    assert_eq!(mix.output(), &[0.5, 0.5]);
    assert_eq!(a.output(), &[0.5, 0.25]);
    assert_eq!(b.output(), &[0.5, 0.5]);

    let mut a = host(config(vec![delay()], 2, 192000), Attachment::Output(1), 4);
    let mut b = host(config(vec![delay()], 2, 192000), Attachment::Output(2), 4);
    a.process(a.binding(), &format, 1, &[0.5, 0.25]).unwrap();
    b.process(b.binding(), &format, 1, &[0.0; 2]).unwrap();
    a.process(a.binding(), &format, 2, &[0.0; 4]).unwrap();
    b.process(b.binding(), &format, 2, &[0.0; 4]).unwrap();
    assert_eq!(a.output(), &[0.0, 0.0, 0.5, 0.25]);
    assert_eq!(b.output(), &[0.0; 4]);
    a.revoke();
    assert!(a.output().is_empty());
    b.process(b.binding(), &format, 1, &[0.0; 2]).unwrap();
    assert!(b.is_active());
}

#[test]
fn ramps_follow_sample_time_and_retarget_from_the_current_value() {
    let format = config(vec![], 2, 96000).format;
    let mut whole = host(config(vec![], 2, 96000), Attachment::Source(1), 4);
    let mut split = host(config(vec![], 2, 96000), Attachment::Source(1), 4);
    let controls = Controls {
        gain: 0.0,
        wet: 1.0,
    };
    let w = whole.update(whole.binding(), 2, controls, 4).unwrap();
    let s = split.update(split.binding(), 2, controls, 4).unwrap();
    let report = whole.process(w, &format, 4, &[0.5; 8]).unwrap();
    let expected = [0.375, 0.375, 0.25, 0.25, 0.125, 0.125, 0.0, 0.0];
    assert_eq!(whole.output(), &expected);
    split.process(s, &format, 1, &[0.5; 2]).unwrap();
    assert_eq!(split.output(), &expected[..2]);
    split.process(s, &format, 3, &[0.5; 6]).unwrap();
    assert_eq!(split.output(), &expected[2..]);
    assert_eq!(report.controls.gain, 0.0);
    assert_eq!(report.transition_remaining_frames, 0);

    let mut h = host(config(vec![], 2, 96000), Attachment::Source(1), 4);
    let rev2 = h.update(h.binding(), 2, controls, 4).unwrap();
    h.process(rev2, &format, 2, &[1.0; 4]).unwrap();
    assert_eq!(h.output(), &[0.75, 0.75, 0.5, 0.5]);
    let rev3 = h.update(rev2, 3, Controls::UNITY, 2).unwrap();
    assert!(h.output().is_empty());
    h.process(rev3, &format, 2, &[1.0; 4]).unwrap();
    assert_eq!(h.output(), &[0.75, 0.75, 1.0, 1.0]);
}

#[test]
fn wet_ramp_uses_actual_processed_signal_and_keeps_delay_warm() {
    let format = config(vec![], 1, 48000).format;
    let mut h = host(config(vec![gain(3.0)], 1, 48000), Attachment::Mix(2), 4);
    let b = h
        .update(
            h.binding(),
            2,
            Controls {
                gain: 1.0,
                wet: 0.0,
            },
            4,
        )
        .unwrap();
    let report = h.process(b, &format, 4, &[1.0; 4]).unwrap();
    assert_eq!(h.output(), &[2.5, 2.0, 1.5, 1.0]);
    assert_eq!(report.over_full_scale_samples, 3);
    let mut d = host(config(vec![delay()], 1, 48000), Attachment::Output(4), 4);
    let dry = d
        .update(
            d.binding(),
            2,
            Controls {
                gain: 1.0,
                wet: 0.0,
            },
            0,
        )
        .unwrap();
    d.process(dry, &format, 1, &[0.5]).unwrap();
    assert_eq!(d.output(), &[0.5]);
    let wet = d.update(dry, 3, Controls::UNITY, 0).unwrap();
    d.process(wet, &format, 2, &[0.0; 2]).unwrap();
    assert_eq!(d.output(), &[0.0, 0.5]);
}

#[test]
fn unity_and_dry_endpoints_preserve_float64_bits_without_clipping() {
    let format = config(vec![], 3, 352800).format;
    let mut h = host(config(vec![], 3, 352800), Attachment::Output(0), 2);
    let samples = [-0.0, f64::from_bits(1), 1.0 + f64::EPSILON];
    let report = h.process(h.binding(), &format, 1, &samples).unwrap();
    assert_eq!(report.over_full_scale_samples, 1);
    for (actual, expected) in h.output().iter().zip(samples) {
        assert_eq!(actual.to_bits(), expected.to_bits());
    }
    let mut dry = host(config(vec![gain(2.0)], 3, 352800), Attachment::Output(2), 2);
    let b = dry
        .update(
            dry.binding(),
            2,
            Controls {
                gain: 1.0,
                wet: 0.0,
            },
            0,
        )
        .unwrap();
    dry.process(b, &format, 1, &samples).unwrap();
    for (actual, expected) in dry.output().iter().zip(samples) {
        assert_eq!(actual.to_bits(), expected.to_bits());
    }
    let b = h
        .update(
            h.binding(),
            2,
            Controls {
                gain: 1.0,
                wet: 0.5,
            },
            0,
        )
        .unwrap();
    h.process(b, &format, 1, &samples).unwrap();
    for (actual, expected) in h.output().iter().zip(samples) {
        assert_eq!(actual.to_bits(), expected.to_bits());
    }

    // Mixing opposite finite full-range values must not first subtract them:
    // that difference overflows even though the requested output is finite.
    let polarity = Effect::Gain {
        gain: 1.0,
        invert_polarity: true,
    };
    let mut opposite = host(config(vec![polarity], 3, 352800), Attachment::Mix(3), 2);
    let b = opposite
        .update(
            opposite.binding(),
            2,
            Controls {
                gain: 1.0,
                wet: 0.5,
            },
            0,
        )
        .unwrap();
    opposite
        .process(b, &format, 1, &[f64::MAX, -f64::MAX, f64::MAX / 2.0])
        .unwrap();
    assert_eq!(opposite.output(), &[0.0; 3]);
}

#[test]
fn wrong_attachment_generation_revision_or_format_terminally_clears_tails() {
    let format = config(vec![], 2, 96000).format;
    let correct = binding(Attachment::Output(7));
    let wrong = [
        Binding {
            attachment: Attachment::Source(7),
            ..correct
        },
        Binding {
            attachment: Attachment::Output(8),
            ..correct
        },
        Binding {
            generation: 9,
            ..correct
        },
        Binding {
            revision: 2,
            ..correct
        },
    ];
    for token in wrong {
        let mut h = host(config(vec![delay()], 2, 96000), correct.attachment, 4);
        h.process(correct, &format, 1, &[0.5; 2]).unwrap();
        assert_eq!(
            h.process(token, &format, 1, &[0.0; 2]),
            Err(Error::BindingMismatch)
        );
        assert!(h.output().is_empty());
        assert!(!h.is_active());
        assert_eq!(
            h.process(correct, &format, 4, &[0.0; 8]),
            Err(Error::Inactive)
        );
        assert_eq!(
            h.update(correct, 2, Controls::UNITY, 0),
            Err(Error::Inactive)
        );
    }
    let mut reordered = format.clone();
    reordered.channels.swap(0, 1);
    let mut rate = format.clone();
    rate.sample_rate_hz = 48000;
    for wrong_format in [reordered, rate] {
        let mut h = host(config(vec![delay()], 2, 96000), correct.attachment, 4);
        h.process(correct, &format, 1, &[0.5; 2]).unwrap();
        assert_eq!(
            h.process(correct, &wrong_format, 1, &[0.0; 2]),
            Err(Error::FormatMismatch)
        );
        assert!(h.output().is_empty());
        assert_eq!(
            h.process(correct, &format, 4, &[0.0; 8]),
            Err(Error::Inactive)
        );
    }
    let mut h = host(config(vec![delay()], 2, 96000), correct.attachment, 4);
    h.process(correct, &format, 1, &[0.5; 2]).unwrap();
    h.revoke();
    assert_eq!(
        h.process(correct, &format, 1, &[0.0; 2]),
        Err(Error::Inactive)
    );
    // A separately compiled, explicitly rebound generation contains no old tail.
    let next = Binding {
        generation: 9,
        ..correct
    };
    let mut restarted = LiveChain::compile(
        config(vec![delay()], 2, 96000),
        limits(4),
        0,
        next,
        Controls::UNITY,
    )
    .unwrap();
    restarted.process(next, &format, 4, &[0.0; 8]).unwrap();
    assert_eq!(restarted.output(), &[0.0; 8]);
}

#[test]
fn stale_control_and_invalid_commands_cannot_resume_or_expose_earlier_output() {
    let format = config(vec![], 1, 48000).format;
    let mut h = host(config(vec![delay()], 1, 48000), Attachment::Source(1), 4);
    let old = h.binding();
    let new = h.update(old, 2, Controls::UNITY, 1).unwrap();
    h.process(new, &format, 1, &[0.5]).unwrap();
    assert_eq!(
        h.update(old, 3, Controls::UNITY, 1),
        Err(Error::BindingMismatch)
    );
    assert!(h.output().is_empty());
    for (revision, controls, frames, error) in [
        (1, Controls::UNITY, 0, Error::Revision),
        (3, Controls::UNITY, 0, Error::Revision),
        (
            2,
            Controls {
                gain: f64::INFINITY,
                wet: 1.0,
            },
            0,
            Error::InvalidControls,
        ),
        (
            2,
            Controls {
                gain: -1.0,
                wet: 1.0,
            },
            0,
            Error::InvalidControls,
        ),
        (
            2,
            Controls {
                gain: 1.0,
                wet: f64::NAN,
            },
            0,
            Error::InvalidControls,
        ),
        (
            2,
            Controls {
                gain: 1.0,
                wet: 1.01,
            },
            0,
            Error::InvalidControls,
        ),
        (2, Controls::UNITY, 1025, Error::TransitionFrames),
    ] {
        let mut h = host(config(vec![], 1, 48000), Attachment::Mix(1), 4);
        h.process(h.binding(), &format, 1, &[0.5]).unwrap();
        assert_eq!(
            h.update(h.binding(), revision, controls, frames),
            Err(error)
        );
        assert!(h.output().is_empty());
        assert!(!h.is_active());
    }
    let max = Binding {
        revision: u64::MAX,
        ..binding(Attachment::Source(0))
    };
    let mut h =
        LiveChain::compile(config(vec![], 1, 48000), limits(4), 0, max, Controls::UNITY).unwrap();
    assert_eq!(h.update(max, 0, Controls::UNITY, 0), Err(Error::Revision));
}

#[test]
fn processing_errors_and_final_gain_overflow_discard_whole_blocks() {
    let format = config(vec![], 1, 48000).format;
    for (frames, samples, expected) in [
        (0, &[0.0][..], Error::Process(ProcessError::FrameCount)),
        (5, &[0.0; 5][..], Error::Process(ProcessError::FrameCount)),
        (2, &[0.0][..], Error::Process(ProcessError::SampleCount)),
        (
            1,
            &[f64::NAN][..],
            Error::Process(ProcessError::NonFiniteInput { sample: 0 }),
        ),
    ] {
        let mut h = host(config(vec![delay()], 1, 48000), Attachment::Source(1), 4);
        h.process(h.binding(), &format, 1, &[0.5]).unwrap();
        assert_eq!(
            h.process(h.binding(), &format, frames, samples),
            Err(expected)
        );
        assert!(h.output().is_empty());
        assert_eq!(
            h.process(h.binding(), &format, 1, &[0.0]),
            Err(Error::Inactive)
        );
    }
    let mut h = host(config(vec![], 1, 48000), Attachment::Output(1), 4);
    let b = h
        .update(
            h.binding(),
            2,
            Controls {
                gain: 2.0,
                wet: 1.0,
            },
            0,
        )
        .unwrap();
    assert_eq!(
        h.process(b, &format, 2, &[0.25, f64::MAX]),
        Err(Error::NonFiniteResult { sample: 1 })
    );
    assert!(h.output().is_empty());
    assert!(!h.is_active());
    let mut h = host(config(vec![gain(2.0)], 1, 48000), Attachment::Output(1), 4);
    assert_eq!(
        h.process(h.binding(), &format, 2, &[0.25, f64::MAX]),
        Err(Error::Process(ProcessError::NonFiniteResult { effect: 0 }))
    );
    assert!(h.output().is_empty());
    assert!(!h.is_active());
}

#[test]
fn combined_admission_rejects_excess_before_retaining_a_partial_host() {
    let build = |limits: Limits| {
        LiveChain::compile(
            config(vec![delay()], 2, 96000),
            limits,
            64,
            binding(Attachment::Mix(1)),
            Controls::UNITY,
        )
    };
    let admitted = build(limits(4)).unwrap().admission();
    let mut exact = limits(4);
    exact.max_memory_bytes = admitted.memory_bytes;
    exact.max_work_units_per_block = admitted.work_units_per_block;
    assert!(build(exact).is_ok());
    let mut memory = exact;
    memory.max_memory_bytes -= 1;
    assert!(
        matches!(build(memory), Err(Error::Compile(CompileError::Budget { resource: "memoryBytes", required, limit })) if required == admitted.memory_bytes && limit == admitted.memory_bytes - 1)
    );
    let mut work = exact;
    work.max_work_units_per_block -= 1;
    assert!(
        matches!(build(work), Err(Error::Compile(CompileError::Budget { resource: "workUnitsPerBlock", required, limit })) if required == admitted.work_units_per_block && limit == admitted.work_units_per_block - 1)
    );
    let mut overflow = limits(usize::MAX);
    overflow.max_channels = usize::MAX;
    assert!(matches!(
        build(overflow),
        Err(Error::Compile(CompileError::ResourceOverflow))
    ));
    for bad in [
        Binding {
            generation: 0,
            ..binding(Attachment::Source(1))
        },
        Binding {
            revision: 0,
            ..binding(Attachment::Source(1))
        },
    ] {
        assert!(matches!(
            LiveChain::compile(config(vec![], 1, 48000), limits(4), 0, bad, Controls::UNITY),
            Err(Error::InvalidBinding)
        ));
    }
}

#[test]
fn processing_smoothing_updates_revoke_and_errors_perform_zero_heap_operations() {
    let format = config(vec![], 2, 96000).format;
    let effects = vec![
        gain(0.5),
        filter(mixyard::dsp::Filter::Peaking, 2000.0, 3.0),
        delay(),
        Effect::Fir {
            coefficients: vec![1.0, 0.25, -0.1],
        },
        Effect::Compressor {
            parameters: dynamics(),
        },
    ];
    let mut h = host(config(effects, 2, 96000), Attachment::Output(1), 128);
    let mut stale = host(config(vec![delay()], 2, 96000), Attachment::Output(1), 128);
    let mut bad = host(config(vec![], 2, 96000), Attachment::Output(2), 128);
    let mut overflow = host(config(vec![], 2, 96000), Attachment::Output(3), 128);
    let samples = [0.25; 256];
    HEAP.with(|n| n.set(Some(0)));
    let mut b = h.binding();
    for revision in 2..100 {
        b = h
            .update(
                b,
                revision,
                Controls {
                    gain: 0.75,
                    wet: 0.5,
                },
                256,
            )
            .unwrap();
        h.process(b, &format, 128, &samples).unwrap();
        h.process(b, &format, 1, &samples[..2]).unwrap();
    }
    stale
        .process(stale.binding(), &format, 1, &samples[..2])
        .unwrap();
    let wrong = Binding {
        generation: 9,
        ..stale.binding()
    };
    assert_eq!(
        stale.process(wrong, &format, 1, &samples[..2]),
        Err(Error::BindingMismatch)
    );
    assert_eq!(
        bad.process(bad.binding(), &format, 1, &[f64::NAN; 2]),
        Err(Error::Process(ProcessError::NonFiniteInput { sample: 0 }))
    );
    let b = overflow
        .update(
            overflow.binding(),
            2,
            Controls {
                gain: 2.0,
                wet: 1.0,
            },
            0,
        )
        .unwrap();
    assert_eq!(
        overflow.process(b, &format, 1, &[f64::MAX; 2]),
        Err(Error::NonFiniteResult { sample: 0 })
    );
    h.revoke();
    assert!(h.output().is_empty());
    let operations = HEAP.with(|n| n.replace(None).unwrap());
    assert_eq!(
        operations, 0,
        "callback paths must not allocate, reallocate or free"
    );
}

#[test]
fn observe_processing_budget_without_degrading_or_claiming_native_delivery() {
    let format: Format = config(vec![], 2, 96000).format;
    let effects = vec![
        gain(0.8),
        filter(mixyard::dsp::Filter::Peaking, 3000.0, 3.0),
        delay(),
        Effect::Fir {
            coefficients: vec![1.0, 0.5, -0.25, 0.1],
        },
        Effect::Compressor {
            parameters: dynamics(),
        },
    ];
    let mut h = host(config(effects, 2, 96000), Attachment::Output(1), 128);
    let samples = [0.25; 256];
    for _ in 0..64 {
        h.process(h.binding(), &format, 128, &samples).unwrap();
    }
    let mut total = std::time::Duration::ZERO;
    let mut max = std::time::Duration::ZERO;
    let mut misses = 0;
    let budget = std::time::Duration::from_secs_f64(128.0 / 96000.0);
    for _ in 0..512 {
        let start = Instant::now();
        h.process(h.binding(), &format, 128, &samples).unwrap();
        let elapsed = start.elapsed();
        total += elapsed;
        max = max.max(elapsed);
        misses += usize::from(elapsed > budget);
        assert_eq!(h.output().len(), samples.len());
    }
    eprintln!("OBSERVATION ONLY: OS={} arch={} debug_assertions={} Float64 rate=96000 channels=2 frames=128 blocks=512 total_ns={} max_ns={} block_budget_ns={} observed_misses={}; native/hard-realtime/endurance unqualified", std::env::consts::OS, std::env::consts::ARCH, cfg!(debug_assertions), total.as_nanos(), max.as_nanos(), budget.as_nanos(), misses);
}

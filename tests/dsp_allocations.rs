#[path = "dsp/support.rs"]
#[allow(dead_code)]
mod support;
use mixyard::dsp::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use support::*;

struct Tracked;
thread_local! { static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) }; }
fn record_allocation() {
    let _ = ALLOCATIONS.try_with(|n| {
        if let Some(count) = n.get() {
            n.set(Some(count + 1));
        }
    });
}
thread_local! { static OPERATIONS: Cell<Option<usize>> = const { Cell::new(None) }; }
fn record() {
    let _ = OPERATIONS.try_with(|n| {
        if let Some(count) = n.get() {
            n.set(Some(count + 1));
        }
    });
}
unsafe impl GlobalAlloc for Tracked {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record();
        record_allocation();
        System.alloc(layout)
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record();
        record_allocation();
        System.alloc_zeroed(layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record();
        record_allocation();
        System.realloc(ptr, layout, size)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        record();
        System.dealloc(ptr, layout)
    }
}
#[global_allocator]
static ALLOCATOR: Tracked = Tracked;

#[test]
fn actual_effects_errors_metering_resets_and_bypass_have_zero_heap_operations() {
    let effects = vec![
        Effect::Gain {
            gain: 1.5,
            invert_polarity: true,
        },
        filter(Filter::Peaking, 3000.0, 3.0),
        Effect::Delay {
            frames: 17,
            wet: 0.3,
            dry: 0.7,
            feedback: 0.4,
        },
        Effect::Compressor {
            parameters: dynamics(),
        },
        Effect::Expander {
            parameters: dynamics(),
        },
        Effect::Fir {
            coefficients: vec![1.0, 0.5, 0.25, -0.1],
        },
    ];
    let mut cfg = config(effects, 8, 96000);
    for e in &mut cfg.effects {
        e.channels = Some(vec![
            "opaque-6".into(),
            "opaque-2".into(),
            "opaque-0".into(),
        ]);
    }
    cfg.effects.push(EffectConfig {
        name: "matrix".into(),
        enabled: true,
        channels: None,
        effect: Effect::ChannelMatrix {
            routes: (0..8)
                .map(|i| MatrixRoute {
                    output: format!("opaque-{i}"),
                    inputs: vec![
                        MatrixInput {
                            channel: format!("opaque-{}", (i + 1) % 8),
                            gain: 0.5,
                        },
                        MatrixInput {
                            channel: format!("opaque-{i}"),
                            gain: -0.25,
                        },
                    ],
                })
                .collect(),
        },
    });
    let mut c = Chain::compile(cfg, limits(64)).unwrap();
    let mut matrix_error = chain(
        vec![matrix(&[
            ("opaque-0", &[("opaque-0", 2.0), ("opaque-1", -2.0)]),
            ("opaque-1", &[]),
        ])],
        2,
        48000,
        2,
    );
    let mut overflow = chain(
        vec![Effect::Fir {
            coefficients: vec![2.0, 0.5],
        }],
        1,
        48000,
        2,
    );
    let input = [0.25; 512];
    let zeros = [0.0; 512];
    let invalid = [f64::NAN; 512];
    let mut p = dynamics();
    p.threshold_db = -6000.0;
    let mut extreme = chain(vec![Effect::Compressor { parameters: p }], 1, 48000, 1);
    OPERATIONS.with(|n| n.set(Some(0)));
    for _ in 0..256 {
        extreme.process(1, &[1e300]).unwrap();
        assert!((extreme.output()[0] / 1e-150 - 1.0).abs() < 1e-12);
        c.process(64, &input).unwrap();
        assert_eq!(c.output().len(), 512);
        c.process(1, &input[..8]).unwrap();
        c.process(64, &zeros).unwrap();
        assert_eq!(
            c.process(64, &invalid),
            Err(ProcessError::NonFiniteInput { sample: 0 })
        );
        assert_eq!(c.process(65, &input), Err(ProcessError::FrameCount));
        assert_eq!(c.process(0, &[]), Err(ProcessError::FrameCount));
        assert_eq!(c.process(1, &input[..7]), Err(ProcessError::SampleCount));
        assert_eq!(
            overflow.process(2, &[1.0, f64::MAX]),
            Err(ProcessError::NonFiniteResult { effect: 0 })
        );
        assert_eq!(
            matrix_error.process(2, &[0.25, 0.5, f64::MAX, f64::MAX]),
            Err(ProcessError::NonFiniteResult { effect: 0 })
        );
        assert!(matrix_error.output().is_empty());
        matrix_error.reset();
        matrix_error.set_bypassed(true);
        matrix_error.process(1, &[-0.0, f64::from_bits(1)]).unwrap();
        matrix_error.set_bypassed(false);
        matrix_error.process(2, &[0.0; 4]).unwrap();
        c.set_bypassed(true);
        c.process(64, &input).unwrap();
        c.set_bypassed(false);
        c.reset();
        assert!(c.output().is_empty());
        assert!(c.meters().iter().all(|m| m.peak == 0.0));
    }
    let operations = OPERATIONS.with(|n| n.replace(None).unwrap());
    assert_eq!(
        operations, 0,
        "allocation/reallocation/deallocation during actual processing"
    );
}

#[test]
fn matrix_and_selector_budgets_reject_before_any_compile_allocation() {
    for resource in ["memoryBytes", "workUnitsPerBlock"] {
        let mut cfg = config(
            vec![
                Effect::Delay {
                    frames: 10,
                    wet: 1.0,
                    dry: 0.0,
                    feedback: 0.5,
                },
                matrix(&[
                    ("opaque-0", &[("opaque-1", 1.0)]),
                    ("opaque-1", &[("opaque-0", -1.0)]),
                ]),
            ],
            2,
            48000,
        );
        cfg.effects[0].channels = Some(vec!["opaque-1".into()]);
        let admission = Chain::compile(cfg.clone(), limits(8)).unwrap().admission();
        let mut l = limits(8);
        if resource == "memoryBytes" {
            l.max_memory_bytes = admission.memory_bytes - 1;
        } else {
            l.max_work_units_per_block = admission.work_units_per_block - 1;
        }
        ALLOCATIONS.with(|n| n.set(Some(0)));
        let result = Chain::compile(cfg, l);
        let allocations = ALLOCATIONS.with(|n| n.replace(None).unwrap());
        assert!(matches!(result,Err(CompileError::Budget{resource:r,..}) if r==resource));
        assert_eq!(
            allocations, 0,
            "budget rejection allocated prepared storage"
        );
    }
    let mut cfg = config(
        vec![Effect::Delay {
            frames: usize::MAX,
            wet: 1.0,
            dry: 0.0,
            feedback: 0.0,
        }],
        2,
        48000,
    );
    cfg.effects[0].channels = Some(vec!["opaque-1".into()]);
    ALLOCATIONS.with(|n| n.set(Some(0)));
    let result = Chain::compile(cfg, limits(1));
    let allocations = ALLOCATIONS.with(|n| n.replace(None).unwrap());
    assert!(matches!(result, Err(CompileError::ResourceOverflow)));
    assert_eq!(allocations, 0);
}

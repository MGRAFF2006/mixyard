#[path = "dsp/support.rs"]
#[allow(dead_code)]
mod support;
use mixyard::dsp::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use support::*;

struct Tracked;
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
    let mut c = chain(effects, 8, 96000, 64);
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
    OPERATIONS.with(|n| n.set(Some(0)));
    for _ in 0..256 {
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

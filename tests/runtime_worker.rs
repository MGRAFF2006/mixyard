use mixyard::audio::Precision;
use mixyard::runtime::worker::{Binding, Error, Format, Queue, Stamp};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Allocator;
thread_local! { static HEAP: Cell<Option<usize>> = const { Cell::new(None) }; }
fn record() {
    let _ = HEAP.try_with(|n| {
        if let Some(v) = n.get() {
            n.set(Some(v + 1));
        }
    });
}
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        record();
        System.alloc(l)
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        record();
        System.alloc_zeroed(l)
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        record();
        System.realloc(p, l, n)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        record();
        System.dealloc(p, l)
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

const BINDING: Binding = Binding {
    generation: 8,
    revision: 12,
};
fn format(channels: usize) -> Format {
    Format {
        sample_rate_hz: 96_000,
        channels: (0..channels).map(|n| format!("lane-{n}")).collect(),
        precision: Precision::Float64,
    }
}
fn queue(channels: usize, slots: usize) -> Queue<f64> {
    Queue::new(format(channels), BINDING, 3, slots, 2, 8192).unwrap()
}
fn stamp(sequence: u64, first_frame: u64) -> Stamp {
    Stamp {
        binding: BINDING,
        clock_id: 3,
        sequence,
        first_frame,
        captured_ns: 10,
        deadline_ns: 30,
    }
}

#[test]
fn exact_multichannel_low_bits_order_and_signed_zero_survive_once() {
    let mut q = queue(4, 2);
    let source = [
        -0.0,
        0.5 + 2f64.powi(-40),
        f64::from_bits(1),
        -0.125,
        0.25,
        0.75,
        -0.5,
        0.125,
    ];
    let mut output = [9.0; 12];
    q.push(10, stamp(0, 100), 2, &source).unwrap();
    let block = q.pop(11, BINDING, &mut output).unwrap();
    assert_eq!(block.frames, 2);
    assert_eq!(block.stamp.first_frame, 100);
    for (actual, expected) in output[..8].iter().zip(source) {
        assert_eq!(actual.to_bits(), expected.to_bits());
    }
    assert_eq!(&output[8..], &[0.0; 4]);
    assert_eq!(q.pop(12, BINDING, &mut output), Err(Error::Empty));
    assert_eq!(output, [0.0; 12]);
}

#[test]
fn overflow_or_expiry_of_one_output_does_not_stall_or_corrupt_sibling() {
    let mut slow = queue(1, 1);
    let mut fast = queue(1, 1);
    slow.push(10, stamp(0, 0), 2, &[0.25, 0.5]).unwrap();
    fast.push(10, stamp(0, 0), 2, &[0.25, 0.5]).unwrap();
    let mut out = [9.0; 2];
    fast.pop(11, BINDING, &mut out).unwrap();
    assert_eq!(
        slow.push(12, stamp(1, 2), 2, &[0.75, 1.0]),
        Err(Error::Full)
    );
    fast.push(12, stamp(1, 2), 2, &[0.75, 1.0]).unwrap();
    assert_eq!(slow.pop(13, BINDING, &mut out), Err(Error::Inactive));
    assert_eq!(out, [0.0; 2]);
    fast.pop(13, BINDING, &mut out).unwrap();
    assert_eq!(out, [0.75, 1.0]);
    fast.push(14, stamp(2, 4), 2, &[0.5, 0.125]).unwrap();
    assert_eq!(fast.pop(31, BINDING, &mut out), Err(Error::Expired));
    assert_eq!(out, [0.0; 2]);
}

#[test]
fn stale_generation_replay_clock_and_gap_reject_without_replay_or_restart() {
    let mut stale = queue(1, 1);
    stale.push(10, stamp(0, 10), 2, &[0.5, 0.25]).unwrap();
    let mut out = [9.0; 2];
    assert_eq!(
        stale.pop(
            11,
            Binding {
                generation: 9,
                ..BINDING
            },
            &mut out
        ),
        Err(Error::Binding)
    );
    assert_eq!(out, [0.0; 2]);
    assert_eq!(
        stale.push(12, stamp(1, 12), 2, &[0.5, 0.25]),
        Err(Error::Inactive)
    );
    for invalid in [
        Stamp {
            clock_id: 4,
            ..stamp(1, 12)
        },
        stamp(0, 12),
        stamp(1, 13),
    ] {
        let mut q = queue(1, 2);
        q.push(10, stamp(0, 10), 2, &[0.5, 0.25]).unwrap();
        assert!(q.push(11, invalid, 2, &[0.5, 0.25]).is_err());
        assert!(!q.active());
        assert_eq!(q.queued_blocks(), 0);
        assert_eq!(q.pop(12, BINDING, &mut out), Err(Error::Inactive));
        assert_eq!(out, [0.0; 2]);
    }
}

#[test]
fn revoke_invalid_samples_and_monotonic_faults_silence_pending_audio() {
    for mode in 0..4 {
        let mut q = queue(1, 2);
        q.push(10, stamp(0, 0), 2, &[0.5, 0.25]).unwrap();
        match mode {
            0 => q.revoke(),
            1 => assert_eq!(
                q.push(11, stamp(1, 2), 2, &[0.5, f64::NAN]),
                Err(Error::NonFinite)
            ),
            2 => assert_eq!(
                q.push(9, stamp(1, 2), 2, &[0.5, 0.25]),
                Err(Error::Timeline)
            ),
            _ => assert_eq!(q.push(11, stamp(1, 2), 2, &[0.5]), Err(Error::Samples)),
        }
        let mut out = [1.0; 2];
        assert_eq!(q.pop(12, BINDING, &mut out), Err(Error::Inactive));
        assert_eq!(out, [0.0; 2]);
    }
}

#[test]
fn construction_rejects_budget_precision_and_ambiguous_channels() {
    assert!(Queue::<f64>::new(format(2), BINDING, 3, 2, 2, 63).is_err());
    assert!(Queue::<f32>::new(format(2), BINDING, 3, 2, 2, 1024).is_err());
    let mut f = format(2);
    f.channels[1] = f.channels[0].clone();
    assert!(Queue::<f64>::new(f, BINDING, 3, 2, 2, 1024).is_err());
}

#[test]
fn transfer_empty_fault_and_revoke_do_not_allocate_or_free() {
    let mut q = queue(1, 2);
    let mut out = [1.0; 2];
    HEAP.with(|n| n.set(Some(0)));
    for sequence in 0..128 {
        q.push(10, stamp(sequence, sequence * 2), 2, &[0.5, -0.0])
            .unwrap();
        q.pop(10, BINDING, &mut out).unwrap();
        assert_eq!(q.pop(10, BINDING, &mut out), Err(Error::Empty));
    }
    assert_eq!(
        q.push(10, stamp(128, 256), 2, &[f64::INFINITY, 0.0]),
        Err(Error::NonFinite)
    );
    q.revoke();
    assert_eq!(q.pop(10, BINDING, &mut out), Err(Error::Inactive));
    let operations = HEAP.with(|n| n.replace(None).unwrap());
    assert_eq!(operations, 0);
}

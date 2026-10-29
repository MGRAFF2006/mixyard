// The production module is std-only so this same test target can run with rustc
// under a small explicit artifact budget, without building unrelated binaries.
#[allow(dead_code)]
#[path = "../src/audio/clock.rs"]
mod clock;

use clock::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;
thread_local! { static OPERATIONS: Cell<Option<usize>> = const { Cell::new(None) }; }
fn count() {
    let _ = OPERATIONS.try_with(|counter| {
        if let Some(n) = counter.get() {
            counter.set(Some(n + 1));
        }
    });
}
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        count();
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count();
        System.realloc(ptr, layout, size)
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count();
        System.alloc_zeroed(layout)
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;

const BINDING: Binding = Binding {
    generation: 8,
    revision: 17,
};
fn limits() -> Limits {
    Limits {
        max_channels: 32,
        max_channel_label_bytes: 64,
        max_input_frames: 32,
        max_output_frames: 32,
        capacity_frames: 64,
        max_buffer_bytes: 128 * 1024,
        max_correction_ppm: 10_000,
    }
}
fn format(rate: u32, labels: &[&str]) -> Format {
    Format {
        sample_rate_hz: rate,
        channels: labels.iter().map(|s| (*s).to_owned()).collect(),
    }
}
fn stamp(sequence: u64, first_frame: u64) -> Stamp {
    Stamp {
        binding: BINDING,
        clock_id: 11,
        sequence,
        first_frame,
        captured_ns: 100,
        deadline_ns: 200,
    }
}
fn adapter<S: ClockSample>(
    source_rate: u32,
    destination_rate: u32,
    labels: &[&str],
    method: Method,
) -> RateAdapter<S> {
    RateAdapter::new(
        format(source_rate, labels),
        format(destination_rate, labels),
        BINDING,
        11,
        if method == Method::Bypass { 11 } else { 22 },
        method,
        limits(),
    )
    .unwrap()
}

#[test]
fn bypass_preserves_order_precision_signed_zero_and_subnormals_without_rate_ceiling() {
    let mut a = adapter::<f64>(
        768_001,
        768_001,
        &["FL", "FR", "C", "LFE", "SL", "SR"],
        Method::Bypass,
    );
    let samples = [
        -0.0,
        f64::from_bits(1),
        0.5 + 2.0f64.powi(-40),
        -0.5,
        f64::MAX,
        -f64::MAX,
    ];
    a.push(100, stamp(0, 0), 1, &samples).unwrap();
    let block = a.render(101, BINDING, 1).unwrap();
    assert_eq!(block.stamp.clock_id, 11);
    assert_eq!(
        a.source_format().channels,
        ["FL", "FR", "C", "LFE", "SL", "SR"]
    );
    assert_eq!(a.destination_format().sample_rate_hz, 768_001);
    for (got, want) in a.output().iter().zip(samples) {
        assert_eq!(got.to_bits(), want.to_bits());
    }
    let mut b = adapter::<f32>(1, 1, &["custom"], Method::Bypass);
    b.push(100, stamp(0, 0), 2, &[-0.0, f32::from_bits(1)])
        .unwrap();
    b.render(100, BINDING, 2).unwrap();
    assert_eq!(
        b.output().iter().map(|s| s.to_bits()).collect::<Vec<_>>(),
        [0x8000_0000, 1]
    );
}

#[test]
fn exact_descriptor_rejects_reordering_guessing_and_implicit_rate_or_clock_conversion() {
    for (src, dst, input_clock, output_clock, method, error) in [
        (
            format(96_000, &["L", "R"]),
            format(96_000, &["R", "L"]),
            11,
            11,
            Method::Bypass,
            Error::ChannelOrder,
        ),
        (
            format(96_000, &["L", "L"]),
            format(96_000, &["L", "L"]),
            11,
            11,
            Method::Bypass,
            Error::InvalidFormat,
        ),
        (
            format(0, &["L"]),
            format(0, &["L"]),
            11,
            11,
            Method::Bypass,
            Error::InvalidFormat,
        ),
        (
            format(96_000, &[""]),
            format(96_000, &[""]),
            11,
            11,
            Method::Bypass,
            Error::InvalidFormat,
        ),
        (
            format(96_000, &["L"]),
            format(48_000, &["L"]),
            11,
            11,
            Method::Bypass,
            Error::ConversionRequired,
        ),
        (
            format(96_000, &["L"]),
            format(96_000, &["L"]),
            11,
            22,
            Method::Bypass,
            Error::ConversionRequired,
        ),
    ] {
        assert!(
            matches!(RateAdapter::<f64>::new(src, dst, BINDING, input_clock, output_clock, method, limits()), Err(e) if e == error)
        );
    }
}

#[test]
fn budgets_reject_before_allocation_including_overflow_and_exact_storage_cost() {
    let mut l = limits();
    l.capacity_frames = usize::MAX;
    assert!(matches!(
        RateAdapter::<f64>::new(
            format(3, &["A", "B"]),
            format(4, &["A", "B"]),
            BINDING,
            11,
            22,
            Method::Linear,
            l
        ),
        Err(Error::Budget)
    ));
    l = limits();
    l.max_buffer_bytes = (64 + 32) * 8 + 64 * 16 - 1;
    assert!(matches!(
        RateAdapter::<f64>::new(
            format(3, &["A"]),
            format(4, &["A"]),
            BINDING,
            11,
            22,
            Method::Linear,
            l
        ),
        Err(Error::Budget)
    ));
    l.max_buffer_bytes += 1;
    assert!(RateAdapter::<f64>::new(
        format(3, &["A"]),
        format(4, &["A"]),
        BINDING,
        11,
        22,
        Method::Linear,
        l
    )
    .is_ok());
    l.max_channels = 1;
    assert!(matches!(
        RateAdapter::<f32>::new(
            format(3, &["A", "B"]),
            format(4, &["A", "B"]),
            BINDING,
            11,
            22,
            Method::Linear,
            l
        ),
        Err(Error::Budget)
    ));
}

#[test]
fn multichannel_fractional_positions_remain_contiguous_across_chunks_and_ring_wraps() {
    let mut l = limits();
    l.capacity_frames = 5;
    let mut a = RateAdapter::<f64>::new(
        format(3, &["A", "B"]),
        format(4, &["A", "B"]),
        BINDING,
        11,
        22,
        Method::Linear,
        l,
    )
    .unwrap();
    a.push(
        100,
        stamp(0, 0),
        4,
        &[0.0, 100.0, 1.0, 101.0, 2.0, 102.0, 3.0, 103.0],
    )
    .unwrap();
    let block = a.render(100, BINDING, 4).unwrap();
    assert_eq!(
        a.output(),
        [0.0, 100.0, 0.75, 100.75, 1.5, 101.5, 2.25, 102.25]
    );
    assert_eq!(block.stamp.first_frame, 0);
    assert_eq!(block.stamp.clock_id, 22);
    assert_eq!(a.buffered_frames(), 1);
    a.push(100, stamp(1, 4), 3, &[4.0, 104.0, 5.0, 105.0, 6.0, 106.0])
        .unwrap();
    let block = a.render(100, BINDING, 4).unwrap();
    assert_eq!(
        a.output(),
        [3.0, 103.0, 3.75, 103.75, 4.5, 104.5, 5.25, 105.25]
    );
    assert_eq!(block.stamp.first_frame, 4);
    assert_eq!(block.stamp.sequence, 1);
    a.push(100, stamp(2, 7), 3, &[7.0, 107.0, 8.0, 108.0, 9.0, 109.0])
        .unwrap();
    a.render(100, BINDING, 4).unwrap();
    assert_eq!(
        a.output(),
        [6.0, 106.0, 6.75, 106.75, 7.5, 107.5, 8.25, 108.25]
    );
}

#[test]
fn downsample_skip_carries_across_input_blocks_without_inventing_samples() {
    let mut a = adapter::<f64>(6, 2, &["A"], Method::Linear);
    a.push(100, stamp(0, 0), 1, &[0.0]).unwrap();
    a.render(100, BINDING, 1).unwrap();
    assert_eq!(a.output(), [0.0]);
    // The next destination position is source frame 3, not the next input frame.
    a.push(100, stamp(1, 1), 3, &[1.0, 2.0, 3.0]).unwrap();
    a.render(100, BINDING, 1).unwrap();
    assert_eq!(a.output(), [3.0]);
    a.push(100, stamp(2, 4), 3, &[4.0, 5.0, 6.0]).unwrap();
    a.render(100, BINDING, 1).unwrap();
    assert_eq!(a.output(), [6.0]);
}

#[test]
fn rational_44100_to_48000_has_no_block_boundary_phase_drift() {
    let mut l = limits();
    l.max_input_frames = 148;
    l.max_output_frames = 160;
    l.capacity_frames = 160;
    let mut a = RateAdapter::<f64>::new(
        format(44_100, &["A"]),
        format(48_000, &["A"]),
        BINDING,
        11,
        22,
        Method::Linear,
        l,
    )
    .unwrap();
    let mut input = [0.0; 148];
    for (i, value) in input.iter_mut().enumerate() {
        *value = i as f64;
    }
    a.push(100, stamp(0, 0), 148, &input).unwrap();
    for block in 0..200 {
        if block > 0 {
            for (i, value) in input[..147].iter_mut().enumerate() {
                *value = (block * 147 + 1 + i) as f64;
            }
            a.push(
                100,
                stamp(block as u64, (block * 147 + 1) as u64),
                147,
                &input[..147],
            )
            .unwrap();
        }
        let result = a.render(100, BINDING, 160).unwrap();
        assert_eq!(a.output()[0], (block * 147) as f64);
        assert!((a.output()[159] - ((block * 147) as f64 + 146.08125)).abs() < 1e-10);
        assert_eq!(result.stamp.first_frame, (block * 160) as u64);
        assert_eq!(a.buffered_frames(), 1);
    }
}

#[test]
fn measured_correction_is_explicit_bounded_and_keeps_fractional_phase() {
    let mut a = adapter::<f64>(1000, 1000, &["A"], Method::Linear);
    a.set_correction_ppm(BINDING, 1000).unwrap();
    a.push(100, stamp(0, 0), 4, &[0.0, 1.0, 2.0, 3.0]).unwrap();
    a.render(100, BINDING, 2).unwrap();
    assert_eq!(a.output()[0], 0.0);
    assert!((a.output()[1] - 1.001).abs() < 1e-14);
    a.set_correction_ppm(BINDING, -1000).unwrap();
    a.render(100, BINDING, 1).unwrap();
    assert!((a.output()[0] - 2.002).abs() < 1e-14);
    a.render(100, BINDING, 1).unwrap_err(); // Position 3.001 needs source frame 4.
    assert!(!a.is_active());
    let mut b = adapter::<f64>(1000, 1000, &["A"], Method::Bypass);
    assert_eq!(
        b.set_correction_ppm(BINDING, 1),
        Err(Error::ConversionRequired)
    );
    let mut c = adapter::<f64>(1000, 1000, &["A"], Method::Linear);
    assert_eq!(
        c.set_correction_ppm(BINDING, 10_001),
        Err(Error::Correction)
    );
    assert!(!c.is_active());
}

#[test]
fn expired_queued_audio_and_partial_underflow_clear_whole_output_terminally() {
    let mut a = adapter::<f64>(2, 4, &["A"], Method::Linear);
    a.push(100, stamp(0, 0), 2, &[1.0, 2.0]).unwrap();
    // Three output frames are possible, but a fourth requires future input.
    assert_eq!(a.render(100, BINDING, 4), Err(Error::Underflow));
    assert_eq!(a.output(), [0.0; 4]);
    assert_eq!(a.buffered_frames(), 0);
    assert_eq!(a.push(100, stamp(1, 2), 1, &[3.0]), Err(Error::Closed));
    let mut b = adapter::<f64>(2, 2, &["A"], Method::Bypass);
    b.push(100, stamp(0, 0), 2, &[1.0, 2.0]).unwrap();
    assert_eq!(b.render(201, BINDING, 2), Err(Error::Expired));
    assert_eq!(b.output(), [0.0; 2]);
    assert!(!b.is_active());
}

#[test]
fn interpolation_preserves_oldest_capture_and_earliest_deadline_across_blocks() {
    let mut a = adapter::<f64>(2, 4, &["A"], Method::Linear);
    let mut first = stamp(0, 0);
    first.deadline_ns = 150;
    a.push(100, first, 1, &[0.0]).unwrap();
    let mut second = stamp(1, 1);
    second.captured_ns = 110;
    a.push(110, second, 1, &[1.0]).unwrap();
    let block = a.render(120, BINDING, 2).unwrap();
    assert_eq!(block.stamp.captured_ns, 100);
    assert_eq!(block.stamp.deadline_ns, 150);
    assert_eq!(a.output(), [0.0, 0.5]);
}

#[test]
fn stale_binding_clock_sequence_and_frame_gap_cannot_resume_or_replay_history() {
    for (bad, error) in [
        (
            Stamp {
                binding: Binding {
                    generation: 7,
                    ..BINDING
                },
                ..stamp(1, 1)
            },
            Error::Binding,
        ),
        (
            Stamp {
                binding: Binding {
                    revision: 16,
                    ..BINDING
                },
                ..stamp(1, 1)
            },
            Error::Binding,
        ),
        (
            Stamp {
                clock_id: 12,
                ..stamp(1, 1)
            },
            Error::Clock,
        ),
        (stamp(0, 1), Error::Discontinuity),
        (stamp(1, 2), Error::Discontinuity),
        (
            Stamp {
                captured_ns: 201,
                ..stamp(1, 1)
            },
            Error::Expired,
        ),
        (
            Stamp {
                deadline_ns: 99,
                ..stamp(1, 1)
            },
            Error::Expired,
        ),
    ] {
        let mut a = adapter::<f64>(3, 3, &["A"], Method::Bypass);
        a.push(100, stamp(0, 0), 1, &[0.75]).unwrap();
        assert_eq!(a.push(100, bad, 1, &[0.5]), Err(error));
        assert_eq!(a.buffered_frames(), 0);
        assert_eq!(a.render(100, BINDING, 1), Err(Error::Closed));
        assert_eq!(a.output(), [0.0]);
    }
}

#[test]
fn time_reversal_revoke_and_restart_fail_closed_without_cross_destination_failure() {
    let mut a = adapter::<f64>(3, 3, &["A"], Method::Bypass);
    let mut sibling = adapter::<f64>(3, 3, &["A"], Method::Bypass);
    a.push(110, stamp(0, 0), 1, &[0.75]).unwrap();
    sibling.push(110, stamp(0, 0), 1, &[0.75]).unwrap();
    assert_eq!(a.render(109, BINDING, 1), Err(Error::TimeReversed));
    sibling.render(110, BINDING, 1).unwrap();
    assert_eq!(sibling.output(), [0.75]);
    sibling.revoke();
    assert_eq!(sibling.output(), [0.0]);
    assert_eq!(sibling.render(110, BINDING, 1), Err(Error::Closed));
    let fresh = Binding {
        generation: 9,
        ..BINDING
    };
    let mut restarted = RateAdapter::<f64>::new(
        format(3, &["A"]),
        format(3, &["A"]),
        fresh,
        11,
        11,
        Method::Bypass,
        limits(),
    )
    .unwrap();
    assert_eq!(
        restarted.push(110, stamp(0, 0), 1, &[0.75]),
        Err(Error::Binding)
    );
    let mut restarted = RateAdapter::<f64>::new(
        format(3, &["A"]),
        format(3, &["A"]),
        fresh,
        11,
        11,
        Method::Bypass,
        limits(),
    )
    .unwrap();
    restarted
        .push(
            110,
            Stamp {
                binding: fresh,
                ..stamp(0, 0)
            },
            1,
            &[0.25],
        )
        .unwrap();
    restarted.render(110, fresh, 1).unwrap();
    assert_eq!(restarted.output(), [0.25]);
}

#[test]
fn malformed_nonfinite_and_queue_overflow_terminally_erase_destination() {
    let mut a = adapter::<f64>(3, 3, &["A", "B"], Method::Bypass);
    assert_eq!(a.push(100, stamp(0, 0), 1, &[0.5]), Err(Error::Length));
    let mut a = adapter::<f64>(3, 3, &["A", "B"], Method::Bypass);
    assert_eq!(
        a.push(100, stamp(0, 0), 1, &[0.5, f64::NAN]),
        Err(Error::NonFinite)
    );
    let mut l = limits();
    l.capacity_frames = 1;
    let mut a = RateAdapter::<f64>::new(
        format(3, &["A"]),
        format(3, &["A"]),
        BINDING,
        11,
        11,
        Method::Bypass,
        l,
    )
    .unwrap();
    a.push(100, stamp(0, 0), 1, &[0.5]).unwrap();
    assert_eq!(a.push(100, stamp(1, 1), 1, &[0.25]), Err(Error::Overflow));
    assert_eq!(a.buffered_frames(), 0);
    assert_eq!(a.render(100, BINDING, 1), Err(Error::Closed));
    assert_eq!(a.output(), [0.0]);
}

#[test]
fn narrow_destination_does_not_reduce_sibling_source_precision() {
    let source = [0.5 + 2.0f64.powi(-40), -0.0, f64::from_bits(1)];
    let mut narrow = [9.0f32; 3];
    assert_eq!(
        narrow_f64(&source, &mut narrow, Narrowing::Exact),
        Err(Error::NotRepresentable)
    );
    assert_eq!(narrow, [0.0; 3]);
    narrow_f64(&source, &mut narrow, Narrowing::RoundToNearest).unwrap();
    assert_eq!(narrow[0], 0.5);
    assert_eq!(narrow[1].to_bits(), (-0.0f32).to_bits());
    let mut sibling = adapter::<f64>(192_000, 192_000, &["A"], Method::Bypass);
    sibling.push(100, stamp(0, 0), 3, &source).unwrap();
    sibling.render(100, BINDING, 3).unwrap();
    for (got, want) in sibling.output().iter().zip(source) {
        assert_eq!(got.to_bits(), want.to_bits());
    }
    let mut widened = [0.0; 3];
    widen_f32(&[0.5, -0.0, f32::from_bits(1)], &mut widened).unwrap();
    assert_eq!(widened[2], f32::from_bits(1) as f64);
    assert_eq!(widened[1].to_bits(), (-0.0f64).to_bits());
    assert_eq!(
        narrow_f64(&[f64::MAX], &mut narrow[..1], Narrowing::RoundToNearest),
        Err(Error::NonFinite)
    );
    assert_eq!(narrow[0], 0.0);
    assert_eq!(
        widen_f32(&[f32::INFINITY; 3], &mut widened),
        Err(Error::NonFinite)
    );
    assert_eq!(widened, [0.0; 3]);
}

#[test]
fn raw_bypass_retains_integer_low_bits_endianness_and_every_lane() {
    for (encoding, width) in [
        (RawEncoding::S16Le, 2),
        (RawEncoding::S16Be, 2),
        (RawEncoding::S24Le, 3),
        (RawEncoding::S24Be, 3),
        (RawEncoding::S32Le, 4),
        (RawEncoding::S32Be, 4),
        (RawEncoding::S64Le, 8),
        (RawEncoding::S64Be, 8),
        (RawEncoding::F32Le, 4),
        (RawEncoding::F32Be, 4),
        (RawEncoding::F64Le, 8),
        (RawEncoding::F64Be, 8),
    ] {
        let f = RawFormat {
            format: format(384_000, &["A", "B", "C", "D", "E", "F"]),
            encoding,
        };
        let mut a = RawBypass::new(f.clone(), f.clone(), BINDING, 11, limits()).unwrap();
        let payload: Vec<u8> = (0..width * 6 * 2).map(|n| (n * 73 + 1) as u8).collect();
        a.copy(100, stamp(0, 0), 2, &payload).unwrap();
        assert_eq!(a.format(), &f);
        assert_eq!(a.output(), payload);
        assert_eq!(a.copy(201, stamp(1, 2), 2, &payload), Err(Error::Expired));
        assert!(a.output().iter().all(|b| *b == 0));
        assert_eq!(a.copy(100, stamp(1, 2), 2, &payload), Err(Error::Closed));
        let mut changed = f.clone();
        changed.format.channels.swap(0, 1);
        assert!(matches!(
            RawBypass::new(f.clone(), changed, BINDING, 11, limits()),
            Err(Error::ConversionRequired)
        ));
        let mut changed = f.clone();
        changed.encoding = if encoding == RawEncoding::S16Le {
            RawEncoding::S16Be
        } else {
            RawEncoding::S16Le
        };
        assert!(matches!(
            RawBypass::new(f, changed, BINDING, 11, limits()),
            Err(Error::ConversionRequired)
        ));
    }
}

#[test]
fn callback_paths_have_no_heap_operations_for_success_correction_and_terminal_errors() {
    let mut a = adapter::<f64>(2, 4, &["A"], Method::Linear);
    let mut bad = adapter::<f64>(2, 4, &["A"], Method::Linear);
    let f = RawFormat {
        format: format(192_000, &["A"]),
        encoding: RawEncoding::S32Le,
    };
    let mut raw = RawBypass::new(f.clone(), f, BINDING, 11, limits()).unwrap();
    let mut narrow = [0.0; 2];
    let mut wide = [0.0; 2];
    a.push(100, stamp(0, 0), 1, &[0.0]).unwrap();
    OPERATIONS.with(|counter| counter.set(Some(0)));
    for i in 0..512 {
        a.set_correction_ppm(BINDING, 0).unwrap();
        a.push(100, stamp(i + 1, i * 2 + 1), 2, &[1.0, 0.0])
            .unwrap();
        a.render(100, BINDING, 4).unwrap();
        raw.copy(100, stamp(i, i * 2), 2, &[1, 2, 3, 4, 5, 6, 7, 8])
            .unwrap();
        narrow_f64(&[0.5, -0.5], &mut narrow, Narrowing::Exact).unwrap();
        widen_f32(&narrow, &mut wide).unwrap();
        assert_eq!(
            bad.render(100, BINDING, 1),
            Err(if i == 0 {
                Error::Underflow
            } else {
                Error::Closed
            })
        );
    }
    a.revoke();
    raw.revoke();
    assert_eq!(
        narrow_f64(&[f64::NAN, 0.0], &mut narrow, Narrowing::Exact),
        Err(Error::NonFinite)
    );
    assert_eq!(
        widen_f32(&[f32::NAN, 0.0], &mut wide),
        Err(Error::NonFinite)
    );
    let operations = OPERATIONS.with(|counter| counter.replace(None).unwrap());
    assert_eq!(operations, 0, "callback heap alloc/realloc/free operations");
}

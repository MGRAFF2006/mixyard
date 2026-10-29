//! A scoped allocation observation of the actual single-owner renderer only.
//! Worker/service/TLS and hardware callback/thread behavior are outside this check.
mod support;
use mixyard_network::*;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    time::{Duration, Instant},
};
use support::*;

struct ObservedAllocator;
thread_local! {
    static OBSERVING: Cell<bool> = const { Cell::new(false) };
    static OPERATIONS: Cell<usize> = const { Cell::new(0) };
}
fn observe() {
    if OBSERVING.try_with(Cell::get).unwrap_or(false) {
        let _ = OPERATIONS.try_with(|n| n.set(n.get() + 1));
    }
}
// Test-only instrumentation delegates unchanged allocation semantics to System.
unsafe impl GlobalAlloc for ObservedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        observe();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        observe();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, n: usize) -> *mut u8 {
        observe();
        unsafe { System.realloc(p, layout, n) }
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        observe();
        unsafe { System.dealloc(p, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: ObservedAllocator = ObservedAllocator;

#[test]
fn render_copy_silence_skip_error_and_invalidation_do_not_use_heap_on_test_thread() {
    let s = spec(
        1,
        "a",
        Codec::RawF64Le,
        44_117,
        &["FR", "FL", "FC", "LFE"],
        7,
    );
    let mut rx = Receiver::new(
        permissions("b", "a", &[], std::slice::from_ref(&s)),
        limits(),
    )
    .unwrap();
    rx.admit(offer(s.clone(), 1)).unwrap();
    let anchor = Instant::now();
    let mut p = rx
        .create_playout(
            1,
            PlayoutConfig {
                anchor,
                target_buffer_frames: 1,
            },
        )
        .unwrap();
    for seq in [0, 2, 3] {
        rx.ingest(
            &Frame {
                stream: 1,
                generation: 1,
                sequence: seq,
                sample_timestamp: seq * 7,
                payload: sample_bytes(&s, seq as usize),
            }
            .encode(1200)
            .unwrap(),
        );
    }
    rx.service_playout(&mut p).unwrap();
    let mut output = [0xff; 5 * 32];
    let frame_time = |frame: u64| {
        anchor
            .checked_add(Duration::from_nanos(
                (frame * 1_000_000_000).div_ceil(44_117),
            ))
            .expect("short fixture time")
    };
    let mut reports = [PlayoutReport::default(); 4];
    OPERATIONS.with(|n| n.set(0));
    OBSERVING.with(|x| x.set(true));
    // Prebuffer prefix, packet split, overlap, hole, then bounded catch-up.
    reports[0] = p.render_into(anchor, &mut output).unwrap();
    reports[1] = p.render_into(frame_time(4), &mut output).unwrap();
    reports[2] = p.render_into(frame_time(9), &mut output).unwrap();
    reports[3] = p.render_into(frame_time(24), &mut output).unwrap();
    let error = p.render_into(frame_time(25), &mut output[..31]);
    OBSERVING.with(|x| x.set(false));
    let first_operations = OPERATIONS.with(Cell::get);
    assert_eq!(
        first_operations, 0,
        "renderer allocation/reallocation/deallocation operations"
    );
    assert_eq!(reports[0].prebuffer_frames, 1);
    assert_eq!(reports[1].early_frames, 1);
    assert_eq!(reports[2].missing_frames, 5);
    assert_eq!(reports[3].skipped_frames, 10);
    assert_eq!(error, Err(PlayoutError::UnalignedBlock));
    let receipt = rx.revoke(1).unwrap(); // Worker call intentionally outside observation.
    assert_eq!(receipt.status(), RevocationStatus::Complete);
    OBSERVING.with(|x| x.set(true));
    let report = p.render_into(frame_time(30), &mut output).unwrap();
    OBSERVING.with(|x| x.set(false));
    assert_eq!(OPERATIONS.with(Cell::get), 0);
    assert_eq!(report.inactive_frames, 5);
    assert_eq!(output, [0; 160]);
}

#[path = "audio/support.rs"]
#[allow(dead_code)]
mod support;

use mixyard::audio::{Processor, RenderError};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use support::*;

struct TrackedAllocator;
thread_local! {
    // Const TLS prevents the instrumentation itself from allocating. Only count
    // this render thread, not unrelated work by Rust's test harness.
    static OPERATIONS: Cell<Option<usize>> = const { Cell::new(None) };
}

fn record() {
    let _ = OPERATIONS.try_with(|count| {
        if let Some(n) = count.get() {
            count.set(Some(n + 1));
        }
    });
}

unsafe impl GlobalAlloc for TrackedAllocator {
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
static ALLOCATOR: TrackedAllocator = TrackedAllocator;

#[test]
fn render_success_silence_and_rejection_do_not_allocate_or_free_heap_storage() {
    let (config, inventory) = example();
    let mut p = Processor::compile(&config, &inventory, 48_000, limits()).unwrap();
    let mic = source(&p, "mic");
    let music = source(&p, "spotify");
    let mut inputs = vec![None; p.plan().sources.len()];
    let mono = [0.125; 32];
    let stereo = [0.25; 64];
    let invalid = [f32::NAN; 64];
    OPERATIONS.with(|count| count.set(Some(0)));
    for _ in 0..512 {
        inputs[mic] = pcm(1, &mono);
        inputs[music] = pcm(2, &stereo);
        assert_eq!(p.render(32, &inputs), Ok(()));
        inputs[mic] = pcm(1, &mono[..1]);
        inputs[music] = pcm(2, &stereo[..2]);
        assert_eq!(p.render(1, &inputs), Ok(()));
        inputs.fill(None);
        assert_eq!(p.render(32, &inputs), Ok(()));
        inputs[music] = pcm(2, &invalid);
        assert_eq!(
            p.render(32, &inputs),
            Err(RenderError::NonFinite {
                source: music,
                sample: 0
            })
        );
        assert_eq!(p.render(33, &inputs), Err(RenderError::FrameCount));
        assert_eq!(p.render(1, &[]), Err(RenderError::SourceCount));
    }
    let operations = OPERATIONS.with(|count| count.replace(None).unwrap());
    assert_eq!(operations, 0, "heap operations during render");
    silent(&p);
}

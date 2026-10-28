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

#[test]
fn float64_render_and_integer_codecs_do_not_allocate_or_free_heap_storage() {
    use mixyard::audio::pcm::{decode, encode_exact, PcmError, SignedPcm};
    use mixyard::audio::{Pcm, Processor64};
    use mixyard::AudioFormat;
    let (config, inventory) = example();
    let mut p = Processor64::compile(&config, &inventory, 48_000, limits()).unwrap();
    let slot = source(&p, "spotify");
    let mut inputs = vec![None; p.plan().sources.len()];
    let samples = [0.5 + 1.0 / 2147483648.0; 64];
    let invalid = [f64::NAN; 64];
    let format = AudioFormat {
        channels: 2,
        sample_rate_hz: 48_000,
    };
    let (overflow_config, overflow_inventory) = overflow_example();
    let mut overflow =
        Processor64::compile(&overflow_config, &overflow_inventory, 48_000, limits()).unwrap();
    let mut overflow_inputs = vec![None; overflow.plan().sources.len()];
    for i in 0..17 {
        overflow_inputs[source(&overflow, &format!("overflow-{i:02}"))] = Some(Pcm {
            format,
            samples: &[f64::MAX; 2],
        });
    }
    let mut bytes = [0u8; 256];
    let mut decoded = [0.0; 64];
    OPERATIONS.with(|count| count.set(Some(0)));
    for _ in 0..512 {
        inputs[slot] = Some(Pcm {
            format,
            samples: &samples,
        });
        assert_eq!(p.render(32, &inputs), Ok(()));
        inputs[slot] = Some(Pcm {
            format,
            samples: &samples[..2],
        });
        assert_eq!(p.render(1, &inputs), Ok(()));
        inputs.fill(None);
        assert_eq!(p.render(32, &inputs), Ok(()));
        inputs[slot] = Some(Pcm {
            format,
            samples: &invalid,
        });
        assert_eq!(
            p.render(32, &inputs),
            Err(RenderError::NonFinite {
                source: slot,
                sample: 0
            })
        );
        inputs[slot] = Some(Pcm {
            format,
            samples: &samples[..1],
        });
        assert_eq!(
            p.render(1, &inputs),
            Err(RenderError::SampleCount { source: slot })
        );
        assert_eq!(p.render(33, &inputs), Err(RenderError::FrameCount));
        assert_eq!(p.render(1, &[]), Err(RenderError::SourceCount));
        assert_eq!(
            overflow.render(1, &overflow_inputs),
            Err(RenderError::NonFiniteResult)
        );
        for encoding in [
            SignedPcm::S16Le,
            SignedPcm::S16Be,
            SignedPcm::S24Le,
            SignedPcm::S24Be,
            SignedPcm::S32Le,
            SignedPcm::S32Be,
        ] {
            let size = encoding.bytes_per_sample() * 64;
            assert_eq!(
                encode_exact(encoding, &[0.5; 64], &mut bytes[..size]),
                Ok(())
            );
            assert_eq!(decode(encoding, &bytes[..size], &mut decoded), Ok(()));
            assert_eq!(
                encode_exact(encoding, &invalid, &mut bytes[..size]),
                Err(PcmError::NonFinite { sample: 0 })
            );
            assert_eq!(
                encode_exact(encoding, &[1.0; 64], &mut bytes[..size]),
                Err(PcmError::OutOfRange { sample: 0 })
            );
            assert_eq!(
                encode_exact(
                    encoding,
                    &[1.0 / (1u64 << 40) as f64; 64],
                    &mut bytes[..size]
                ),
                Err(PcmError::NotRepresentable { sample: 0 })
            );
            assert_eq!(
                encode_exact(encoding, &samples[..1], &mut bytes[..size]),
                Err(PcmError::Length)
            );
            assert_eq!(
                decode(encoding, &bytes[..size - 1], &mut decoded),
                Err(PcmError::Length)
            );
        }
    }
    let operations = OPERATIONS.with(|count| count.replace(None).unwrap());
    assert_eq!(
        operations, 0,
        "heap operations during Float64 render/codecs"
    );
    silent(&p);
}

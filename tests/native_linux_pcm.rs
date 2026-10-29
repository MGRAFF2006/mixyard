#![cfg(target_os = "linux")]

use mixyard::audio::{Limits, Pcm, Precision, Processor64};
use mixyard::runtime::linux_audio::{
    Consumer, ConsumerError, Encoding, Endpoint, Input, LinuxAudio,
};
use mixyard::runtime::worker::{Binding, Format, Stamp};
use mixyard::{AudioFormat, Config, Inventory, OutputObservation, Target};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::PathBuf;

struct Allocator;
thread_local! { static OPS: Cell<Option<usize>> = const { Cell::new(None) }; }
fn count() {
    let _ = OPS.try_with(|v| {
        if let Some(n) = v.get() {
            v.set(Some(n + 1));
        }
    });
}
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        count();
        System.alloc(l)
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        count();
        System.alloc_zeroed(l)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        count();
        System.dealloc(p, l);
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        count();
        System.realloc(p, l, n)
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let root = std::env::var_os("MIXYARD_LINUX_TEST_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let path = root.join(format!(
            "mixyard-linux-pcm-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn fixtures() -> (Config, Inventory) {
    let mut c: Config = serde_json::from_str(include_str!("../examples/config.json")).unwrap();
    let mut i: Inventory =
        serde_json::from_str(include_str!("../examples/inventory.json")).unwrap();
    c.sources
        .retain(|s| ["spotify", "return"].contains(&s.id.as_str()));
    c.mixes
        .retain(|m| ["desk-a", "record"].contains(&m.id.as_str()));
    c.sends.retain(|s| {
        c.sources.iter().any(|v| v.id == s.source) && c.mixes.iter().any(|v| v.id == s.mix)
    });
    c.outputs
        .retain(|o| ["speaker-a", "record-input"].contains(&o.id.as_str()));
    let mut sibling = c
        .outputs
        .iter()
        .find(|o| o.id == "speaker-a")
        .unwrap()
        .clone();
    sibling.id = "narrow-input".into();
    sibling.target = Target::VirtualInput {
        endpoint_id: "owned:narrow".into(),
    };
    i.outputs.push(OutputObservation {
        observed_id: "owned:narrow".into(),
        target: sibling.target.clone(),
        format: Some(AudioFormat {
            channels: 2,
            sample_rate_hz: 48_000,
        }),
    });
    c.outputs.push(sibling);
    (c, i)
}
fn limits() -> Limits {
    Limits {
        max_frames: 8,
        max_sources: 2,
        max_mixes: 2,
        max_outputs: 3,
        max_sends: 4,
        max_buffer_samples: 256,
    }
}
fn format() -> Format {
    Format {
        sample_rate_hz: 48_000,
        channels: vec!["FL".into(), "FR".into()],
        precision: Precision::Float64,
    }
}
fn binding(generation: u64) -> Binding {
    Binding {
        generation,
        revision: 1,
    }
}
fn stamp(generation: u64, sequence: u64, first_frame: u64) -> Stamp {
    Stamp {
        binding: binding(generation),
        clock_id: 7,
        sequence,
        first_frame,
        captured_ns: 100,
        deadline_ns: 1_000,
    }
}
fn engine(generation: u64, slots: usize) -> (LinuxAudio, Vec<Endpoint>, usize, usize, usize) {
    let (c, i) = fixtures();
    let p = Processor64::compile(&c, &i, 48_000, limits()).unwrap();
    let sources = p
        .plan()
        .sources
        .iter()
        .map(|s| Endpoint {
            observed_id: s.observed_ids[0].clone(),
            format: format(),
        })
        .collect::<Vec<_>>();
    let outputs = p
        .plan()
        .outputs
        .iter()
        .map(|s| {
            Some(Endpoint {
                observed_id: s.observed_ids[0].clone(),
                format: format(),
            })
        })
        .collect();
    let slot = |id: &str| p.plan().outputs.iter().position(|o| o.id == id).unwrap();
    let full = slot("speaker-a");
    let record = slot("record-input");
    let narrow = slot("narrow-input");
    let engine = LinuxAudio::new(
        p,
        binding(generation),
        7,
        sources.iter().cloned().map(Some).collect(),
        outputs,
        slots,
        8 * 2 * 8 * slots * 3,
    )
    .unwrap();
    (engine, sources, full, record, narrow)
}
fn input<'a>(bound: &'a Endpoint, stamp: Stamp) -> Option<Input<'a>> {
    Some(Input {
        observed_id: &bound.observed_id,
        format: &bound.format,
        stamp,
    })
}
fn pcm(samples: &[f64]) -> Option<Pcm<'_, f64>> {
    Some(Pcm {
        format: AudioFormat {
            channels: 2,
            sample_rate_hz: 48_000,
        },
        samples,
    })
}

#[test]
fn generated_sources_reach_independent_exact_and_explicitly_narrowed_files() {
    let dir = Directory::new();
    let (mut engine, sources, full, record, narrow) = engine(1, 2);
    let full_path = dir.0.join("full.pcm");
    let record_path = dir.0.join("record.pcm");
    let narrow_path = dir.0.join("narrow.pcm");
    let mut a =
        Consumer::create_file(&mut engine, full, &full_path, Encoding::Float64Le, 8, 1024).unwrap();
    let mut b = Consumer::create_file(
        &mut engine,
        record,
        &record_path,
        Encoding::Float64Le,
        8,
        1024,
    )
    .unwrap();
    let mut n = Consumer::create_file(
        &mut engine,
        narrow,
        &narrow_path,
        Encoding::Float32Le,
        8,
        1024,
    )
    .unwrap();
    let d = 2f64.powi(-40);
    // Stable planner order is return, spotify, never Config vector position.
    let data = [
        [0.25, -0.25, 0.0, 0.0],
        [0.5 + d, -0.5 - d, -0.0, f64::from_bits(1)],
    ];
    let pcm = [pcm(&data[0]), pcm(&data[1])];
    let metadata = [
        input(&sources[0], stamp(1, 0, 0)),
        input(&sources[1], stamp(1, 0, 0)),
    ];
    let rendered = engine
        .render(100, stamp(1, 0, 0), 2, &pcm, &metadata)
        .unwrap();
    assert_eq!((rendered.queued, rendered.failed), (3, 0));
    for consumer in [&mut a, &mut b, &mut n] {
        assert_eq!(consumer.drain(&mut engine, 100).unwrap().unwrap().frames, 2);
    }
    let expected = data[1]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect::<Vec<_>>();
    assert_eq!(std::fs::read(&full_path).unwrap(), expected);
    let mixed: [f64; 4] = [0.1875 + d / 4.0, -0.1875 - d / 4.0, 0.0, 0.0];
    assert_eq!(
        std::fs::read(&record_path).unwrap(),
        mixed
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>()
    );
    let rounded: [f32; 4] = [0.5, -0.5, -0.0, 0.0];
    assert_eq!(
        std::fs::read(&narrow_path).unwrap(),
        rounded
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>()
    );
    assert_eq!(std::fs::metadata(full_path).unwrap().mode() & 0o777, 0o600);
    assert_eq!(a.frames(), 2);
    assert_eq!(a.format(), &format());
    assert_eq!(n.encoding(), Encoding::Float32Le);
    assert!(a.drain(&mut engine, 100).unwrap().is_none());
    eprintln!("owned in-process generated F64 PCM -> authoritative independent mixes -> exact F64/explicit F32 files: verified bytes; native backend flags unchanged");
}

#[test]
fn stalled_output_does_not_stall_null_consumer_or_allocate_in_render() {
    let (mut engine, sources, full, _, _) = engine(1, 1);
    let mut consumer = Consumer::null(&mut engine, full, 1_000, 1024).unwrap();
    let samples = [0.125, -0.25];
    let pcm = [None, pcm(&samples)];
    OPS.with(|v| v.set(Some(0)));
    for sequence in 0..32 {
        let stamp = stamp(1, sequence, sequence);
        let metadata = [None, input(&sources[1], stamp)];
        let result = engine.render(100, stamp, 1, &pcm, &metadata).unwrap();
        assert_eq!(result.queued, if sequence == 0 { 3 } else { 1 });
        assert_eq!(result.failed, if sequence == 0 { 0 } else { 2 });
        assert!(consumer.drain(&mut engine, 100).unwrap().is_some());
    }
    let operations = OPS.with(|v| v.replace(None).unwrap());
    assert_eq!(
        operations, 0,
        "render, queue handoff and null drain heap operations"
    );
    assert_eq!(consumer.frames(), 32);
}

#[test]
fn stale_source_and_revocation_erase_pending_output_and_restart_needs_new_consumer() {
    let (mut engine, sources, full, _, _) = engine(1, 2);
    assert_eq!(engine.binding(), binding(1));
    let mut consumer = Consumer::null(&mut engine, full, 8, 1024).unwrap();
    let samples = [0.5, -0.5];
    let pcm = [None, pcm(&samples)];
    let metadata = [None, input(&sources[1], stamp(1, 0, 0))];
    engine
        .render(100, stamp(1, 0, 0), 1, &pcm, &metadata)
        .unwrap();
    // Previous source block cannot authorize a new engine-timeline frame.
    assert!(engine
        .render(100, stamp(1, 1, 1), 1, &pcm, &metadata)
        .is_err());
    assert!(consumer.drain(&mut engine, 100).is_err());
    assert_eq!(consumer.frames(), 0);
    let (mut next, _, full, _, _) = self::engine(2, 2);
    let mut stale = Consumer::null(&mut next, full, 8, 1024).unwrap();
    next.revoke();
    assert!(stale.drain(&mut next, 100).is_err());
    let (mut next, sources, full, _, _) = self::engine(3, 2);
    assert!(consumer.drain(&mut next, 100).is_err());
    let mut fresh = Consumer::null(&mut next, full, 8, 1024).unwrap();
    let metadata = [None, input(&sources[1], stamp(3, 0, 0))];
    next.render(100, stamp(3, 0, 0), 1, &pcm, &metadata)
        .unwrap();
    assert!(fresh.drain(&mut next, 100).unwrap().is_some());
}

#[test]
fn consumer_rejects_overwrite_conversion_and_frame_budget_without_harming_sibling() {
    let dir = Directory::new();
    let (mut engine, sources, full, _, narrow) = engine(1, 2);
    let path = dir.0.join("exact.pcm");
    std::fs::write(&path, b"preserve").unwrap();
    assert!(Consumer::create_file(&mut engine, full, &path, Encoding::Float64Le, 8, 1024).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"preserve");
    let link = dir.0.join("link.pcm");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert!(Consumer::create_file(&mut engine, full, &link, Encoding::Float64Le, 8, 1024).is_err());
    let exact = dir.0.join("fractional.pcm");
    let mut strict = Consumer::create_file(
        &mut engine,
        narrow,
        &exact,
        Encoding::SignedExact(mixyard::audio::pcm::SignedPcm::S32Le),
        8,
        1024,
    )
    .unwrap();
    let mut sibling = Consumer::null(&mut engine, full, 1, 1024).unwrap();
    let samples = [0.5 + 2f64.powi(-40), -0.5];
    let pcm = [None, pcm(&samples)];
    let metadata = [None, input(&sources[1], stamp(1, 0, 0))];
    engine
        .render(100, stamp(1, 0, 0), 1, &pcm, &metadata)
        .unwrap();
    assert!(matches!(
        strict.drain(&mut engine, 100),
        Err(ConsumerError::Exact(_))
    ));
    assert_eq!(std::fs::metadata(exact).unwrap().len(), 0);
    assert!(sibling.drain(&mut engine, 100).unwrap().is_some());
    let metadata = [None, input(&sources[1], stamp(1, 1, 1))];
    engine
        .render(100, stamp(1, 1, 1), 1, &pcm, &metadata)
        .unwrap();
    assert!(matches!(
        sibling.drain(&mut engine, 100),
        Err(ConsumerError::Budget)
    ));
    assert_eq!(sibling.frames(), 1);
}

#[test]
fn binding_rejects_wrong_identity_rate_precision_and_channel_order() {
    for change in 0..4 {
        let (c, i) = fixtures();
        let p = Processor64::compile(&c, &i, 48_000, limits()).unwrap();
        let mut sources = p
            .plan()
            .sources
            .iter()
            .map(|s| {
                Some(Endpoint {
                    observed_id: s.observed_ids[0].clone(),
                    format: format(),
                })
            })
            .collect::<Vec<_>>();
        let outputs = p
            .plan()
            .outputs
            .iter()
            .map(|o| {
                Some(Endpoint {
                    observed_id: o.observed_ids[0].clone(),
                    format: format(),
                })
            })
            .collect();
        let source = sources[0].as_mut().unwrap();
        match change {
            0 => source.observed_id = "other-source".into(),
            1 => source.format.sample_rate_hz = 44_100,
            2 => source.format.precision = Precision::Float32,
            _ => source.format.channels.reverse(),
        }
        assert!(LinuxAudio::new(p, binding(1), 7, sources, outputs, 1, 4096).is_err());
    }
}

#[test]
fn source_deadline_cannot_be_extended_and_expired_output_is_never_recorded() {
    let (mut engine, sources, full, _, _) = engine(1, 2);
    let mut consumer = Consumer::null(&mut engine, full, 8, 1024).unwrap();
    let samples = [0.5, -0.5];
    let pcm = [None, pcm(&samples)];
    let mut short = stamp(1, 0, 0);
    short.deadline_ns = 110;
    let metadata = [None, input(&sources[1], short)];
    assert!(engine
        .render(100, stamp(1, 0, 0), 1, &pcm, &metadata)
        .is_err());
    assert!(consumer.drain(&mut engine, 100).is_err());
    assert_eq!(consumer.frames(), 0);

    let (mut engine, sources, full, _, _) = self::engine(2, 2);
    let mut consumer = Consumer::null(&mut engine, full, 8, 1024).unwrap();
    let mut short = stamp(2, 0, 0);
    short.deadline_ns = 110;
    let metadata = [None, input(&sources[1], short)];
    engine.render(100, short, 1, &pcm, &metadata).unwrap();
    assert!(consumer.drain(&mut engine, 111).is_err());
    assert_eq!(consumer.frames(), 0);
}

#[test]
fn old_generation_consumer_cannot_revoke_replacement_and_bad_pcm_clears_pending_audio() {
    let (mut old, _, full, _, _) = engine(1, 2);
    let mut old_consumer = Consumer::null(&mut old, full, 8, 1024).unwrap();
    let (mut new, sources, full, _, _) = engine(2, 2);
    let mut fresh = Consumer::null(&mut new, full, 8, 1024).unwrap();
    assert!(matches!(
        old_consumer.drain(&mut new, 100),
        Err(ConsumerError::Binding)
    ));
    let samples = [0.5, -0.5];
    let pcm = [None, pcm(&samples)];
    let metadata = [None, input(&sources[1], stamp(2, 0, 0))];
    new.render(100, stamp(2, 0, 0), 1, &pcm, &metadata).unwrap();
    assert!(fresh.drain(&mut new, 100).unwrap().is_some());
    let metadata = [None, input(&sources[1], stamp(2, 1, 1))];
    new.render(100, stamp(2, 1, 1), 1, &pcm, &metadata).unwrap();
    let invalid = [None, self::pcm(&[f64::NAN, 0.0])];
    let metadata = [None, input(&sources[1], stamp(2, 2, 2))];
    OPS.with(|v| v.set(Some(0)));
    assert!(new
        .render(100, stamp(2, 2, 2), 1, &invalid, &metadata)
        .is_err());
    assert!(fresh.drain(&mut new, 100).is_err());
    let operations = OPS.with(|v| v.replace(None).unwrap());
    assert_eq!(operations, 0, "rejection/revocation heap operations");
    assert_eq!(fresh.frames(), 1);
}

#[test]
fn float32_overflow_and_duplicate_consumer_leave_float64_sibling_exact() {
    let dir = Directory::new();
    let (mut engine, sources, full, _, narrow) = engine(1, 2);
    let full_path = dir.0.join("wide.pcm");
    let narrow_path = dir.0.join("overflow.pcm");
    let mut wide =
        Consumer::create_file(&mut engine, full, &full_path, Encoding::Float64Le, 8, 1024).unwrap();
    assert!(matches!(
        Consumer::null(&mut engine, full, 8, 1024),
        Err(ConsumerError::Binding)
    ));
    let mut narrow = Consumer::create_file(
        &mut engine,
        narrow,
        &narrow_path,
        Encoding::Float32Le,
        8,
        1024,
    )
    .unwrap();
    let samples = [f64::MAX, -f64::MAX];
    let pcm = [None, pcm(&samples)];
    let metadata = [None, input(&sources[1], stamp(1, 0, 0))];
    engine
        .render(100, stamp(1, 0, 0), 1, &pcm, &metadata)
        .unwrap();
    assert!(matches!(
        narrow.drain(&mut engine, 100),
        Err(ConsumerError::Conversion)
    ));
    assert!(wide.drain(&mut engine, 100).unwrap().is_some());
    assert_eq!(
        std::fs::read(full_path).unwrap(),
        samples
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>()
    );
    assert_eq!(std::fs::metadata(narrow_path).unwrap().len(), 0);
}

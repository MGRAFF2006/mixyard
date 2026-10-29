// Compile the production module directly so the focused portable harness needs
// only rustc, including on hosts where a full Cargo build exceeds its budget.
#[path = "../src/runtime/recording.rs"]
mod recording;

use recording::*;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let root = std::env::var_os("MIXYARD_RECORDING_TEST_TMP")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let path = root.join(format!(
            "mixyard-recording-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn context() -> Context {
    Context {
        source_id: "synthetic-mix".into(),
        revision: 7,
        generation: 3,
        clock_id: 11,
    }
}
fn format(encoding: Encoding) -> Format {
    Format {
        sample_rate_hz: 96_000,
        channels: vec!["FR".into(), "FL".into(), "Aux".into()],
        encoding,
    }
}
fn limits() -> Limits {
    Limits {
        max_block_bytes: 1024,
        max_file_bytes: 8192,
        max_blocks: 8,
    }
}
fn create(temp: &Temp, name: &str, format: &Format, limits: Limits) -> (Authority, Recorder) {
    let mut authority = Authority::new(context()).unwrap();
    let consent = authority
        .grant(temp.path(name), format.clone(), limits)
        .unwrap();
    let recorder = Recorder::create(&authority, consent).unwrap();
    (authority, recorder)
}
fn inspect_path(temp: &Temp, name: &str) -> Report {
    inspect(&mut File::open(temp.path(name)).unwrap(), limits()).unwrap()
}
fn payload_start(bytes: &[u8]) -> usize {
    16 + u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize + 20
}

#[test]
fn exact_persistence_and_clean_recovery_preserve_all_pcm_encodings() {
    for encoding in [
        Encoding::S16Le,
        Encoding::S16Be,
        Encoding::S24Le,
        Encoding::S24Be,
        Encoding::S32Le,
        Encoding::S32Be,
        Encoding::F32Le,
        Encoding::F32Be,
        Encoding::F64Le,
        Encoding::F64Be,
    ] {
        let temp = Temp::new();
        let format = format(encoding);
        let (authority, mut recorder) = create(&temp, "archive", &format, limits());
        let width = encoding.bytes_per_sample() * 3;
        let payload: Vec<_> = (0..width * 5).map(|i| (i * 37 + 131) as u8).collect();
        recorder
            .append(&authority, &context(), &format, 0, &payload)
            .unwrap();
        // The acknowledged chunk exists before finalization; no buffered fake write.
        let live = inspect_path(&temp, "archive");
        assert_eq!(
            (live.frames, live.blocks, live.ending),
            (5, 1, Ending::Interrupted)
        );
        recorder
            .append(&authority, &context(), &format, 5, &payload)
            .unwrap();
        let report = recorder.finish().unwrap();
        assert_eq!(
            (report.frames, report.blocks, report.ending),
            (10, 2, Ending::Complete)
        );
        assert_eq!(
            report.metadata,
            Metadata {
                context: context(),
                format: format.clone()
            }
        );
        let bytes = fs::read(temp.path("archive")).unwrap();
        let start = payload_start(&bytes);
        assert_eq!(&bytes[start..start + payload.len()], payload);
        assert_eq!(
            &bytes[start + payload.len() + 20..start + payload.len() * 2 + 20],
            payload
        );
        let recovered = recover(
            &mut File::open(temp.path("archive")).unwrap(),
            &temp.path("copy"),
            limits(),
        )
        .unwrap();
        assert_eq!(recovered, report);
        assert_eq!(fs::read(temp.path("copy")).unwrap(), bytes);
    }
}

#[test]
fn float64_low_bits_and_signed_zero_survive_without_float32_intermediate() {
    let temp = Temp::new();
    let format = format(Encoding::F64Le);
    let (authority, mut recorder) = create(&temp, "archive", &format, limits());
    let values = [1.0_f64 + f64::EPSILON, -0.0, f64::from_bits(1)];
    let payload: Vec<_> = values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    recorder
        .append(&authority, &context(), &format, 0, &payload)
        .unwrap();
    recorder.finish().unwrap();
    let bytes = fs::read(temp.path("archive")).unwrap();
    let start = payload_start(&bytes);
    let decoded: Vec<_> = bytes[start..start + 24]
        .as_chunks::<8>()
        .0
        .iter()
        .map(|part| u64::from_le_bytes(*part))
        .collect();
    assert_eq!(decoded, values.map(f64::to_bits));
}

#[test]
fn partial_last_chunk_salvages_only_complete_contiguous_audio() {
    let temp = Temp::new();
    let format = format(Encoding::S24Be);
    let (authority, mut recorder) = create(&temp, "archive", &format, limits());
    recorder
        .append(&authority, &context(), &format, 0, &[1; 27])
        .unwrap();
    recorder
        .append(&authority, &context(), &format, 3, &[2; 18])
        .unwrap();
    drop(recorder); // Simulate interrupted writer: no automatic footer on Drop.
    let file = OpenOptions::new()
        .write(true)
        .open(temp.path("archive"))
        .unwrap();
    file.set_len(file.metadata().unwrap().len() - 4).unwrap();
    drop(file);
    let before = fs::read(temp.path("archive")).unwrap();
    let report = recover(
        &mut File::open(temp.path("archive")).unwrap(),
        &temp.path("salvaged"),
        limits(),
    )
    .unwrap();
    assert_eq!(
        (report.frames, report.blocks, report.ending),
        (3, 1, Ending::Interrupted)
    );
    assert_eq!(
        &fs::read(temp.path("salvaged")).unwrap()[..report.recoverable_bytes as usize],
        &before[..report.recoverable_bytes as usize]
    );
    assert_eq!(inspect_path(&temp, "salvaged").ending, Ending::Complete);
    assert_eq!(fs::read(temp.path("archive")).unwrap(), before);
}

#[test]
fn corrupted_chunk_stops_recovery_without_skipping_to_later_audio() {
    let temp = Temp::new();
    let format = format(Encoding::S16Le);
    let (authority, mut recorder) = create(&temp, "archive", &format, limits());
    for first in 0..3 {
        recorder
            .append(&authority, &context(), &format, first, &[first as u8; 6])
            .unwrap();
    }
    recorder.finish().unwrap();
    let bytes = fs::read(temp.path("archive")).unwrap();
    let second_payload = payload_start(&bytes) + 6 + 20;
    let mut file = OpenOptions::new()
        .write(true)
        .open(temp.path("archive"))
        .unwrap();
    file.seek(SeekFrom::Start(second_payload as u64)).unwrap();
    file.write_all(&[0xff]).unwrap();
    drop(file);
    let report = recover(
        &mut File::open(temp.path("archive")).unwrap(),
        &temp.path("salvaged"),
        limits(),
    )
    .unwrap();
    assert_eq!(
        (report.frames, report.blocks, report.ending),
        (1, 1, Ending::Corrupt)
    );
    assert_eq!(inspect_path(&temp, "salvaged").frames, 1);
}

#[test]
fn revocation_new_grant_rebind_and_new_session_reject_old_consent() {
    for change in 0..4 {
        let temp = Temp::new();
        let format = format(Encoding::F32Be);
        let (mut authority, mut recorder) = create(&temp, "archive", &format, limits());
        recorder
            .append(&authority, &context(), &format, 0, &[7; 12])
            .unwrap();
        match change {
            0 => authority.revoke(),
            1 => {
                let _ = authority
                    .grant(temp.path("other"), format.clone(), limits())
                    .unwrap();
            }
            2 => {
                let mut next = context();
                next.revision += 1;
                authority.rebind(next).unwrap();
            }
            3 => authority = Authority::new(context()).unwrap(),
            _ => unreachable!(),
        }
        let size = fs::metadata(temp.path("archive")).unwrap().len();
        assert!(matches!(
            recorder.append(&authority, &context(), &format, 1, &[8; 12]),
            Err(Error::Consent)
        ));
        assert!(matches!(
            recorder.append(&authority, &context(), &format, 1, &[8; 12]),
            Err(Error::Closed)
        ));
        assert!(matches!(recorder.finish(), Err(Error::Closed)));
        assert_eq!(fs::metadata(temp.path("archive")).unwrap().len(), size);
    }
}

#[test]
fn source_revision_generation_clock_rate_precision_and_channel_order_are_scoped() {
    for change in 0..8 {
        let temp = Temp::new();
        let original = format(Encoding::F64Le);
        let (authority, mut recorder) = create(&temp, "archive", &original, limits());
        let mut changed_context = context();
        let mut changed_format = original.clone();
        match change {
            0 => changed_context.source_id = "another-source".into(),
            1 => changed_context.revision += 1,
            2 => changed_context.generation += 1,
            3 => changed_context.clock_id += 1,
            4 => changed_format.sample_rate_hz = 48_000,
            5 => changed_format.encoding = Encoding::F32Le,
            6 => changed_format.channels.swap(0, 1),
            7 => changed_format.channels[0] = "other-channel".into(),
            _ => unreachable!(),
        }
        assert!(matches!(
            recorder.append(&authority, &changed_context, &changed_format, 0, &[1; 24]),
            Err(Error::Consent)
        ));
        assert_eq!(inspect_path(&temp, "archive").frames, 0);
    }
}

#[test]
fn frame_gaps_replays_and_partial_samples_are_terminal() {
    for (first, len, gap) in [(2, 6, true), (0, 6, true), (1, 5, false), (1, 0, false)] {
        let temp = Temp::new();
        let format = format(Encoding::S16Le);
        let (authority, mut recorder) = create(&temp, "archive", &format, limits());
        recorder
            .append(&authority, &context(), &format, 0, &[1; 6])
            .unwrap();
        let result = recorder.append(&authority, &context(), &format, first, &vec![2; len]);
        assert!(if gap {
            matches!(result, Err(Error::Gap))
        } else {
            matches!(result, Err(Error::Invalid(_)))
        });
        assert!(matches!(
            recorder.append(&authority, &context(), &format, 1, &[2; 6]),
            Err(Error::Closed)
        ));
        assert_eq!(inspect_path(&temp, "archive").frames, 1);
    }
}

#[test]
fn block_byte_and_file_budgets_fail_without_extra_audio_and_reserve_footer() {
    for mode in 0..3 {
        let temp = Temp::new();
        let format = format(Encoding::S16Le);
        let mut admitted = limits();
        match mode {
            0 => admitted.max_blocks = 1,
            1 => admitted.max_block_bytes = 6,
            2 => {
                let (authority, recorder) = create(&temp, "sizing", &format, limits());
                drop(authority);
                drop(recorder);
                admitted.max_file_bytes =
                    fs::metadata(temp.path("sizing")).unwrap().len() + 20 + 6 + 20;
            }
            _ => unreachable!(),
        }
        let (authority, mut recorder) = create(&temp, "archive", &format, admitted);
        recorder
            .append(&authority, &context(), &format, 0, &[1; 6])
            .unwrap();
        let next = if mode == 1 { vec![2; 12] } else { vec![2; 6] };
        assert!(matches!(
            recorder.append(&authority, &context(), &format, 1, &next),
            Err(Error::Limit)
        ));
        assert_eq!(inspect_path(&temp, "archive").frames, 1);
    }
    let temp = Temp::new();
    let format = format(Encoding::S16Le);
    let (authority, recorder) = create(&temp, "sizing", &format, limits());
    drop((authority, recorder));
    let exact = fs::metadata(temp.path("sizing")).unwrap().len() + 20 + 6 + 20;
    let (authority, mut recorder) = create(
        &temp,
        "exact",
        &format,
        Limits {
            max_file_bytes: exact,
            ..limits()
        },
    );
    recorder
        .append(&authority, &context(), &format, 0, &[1; 6])
        .unwrap();
    recorder.finish().unwrap();
    assert_eq!(fs::metadata(temp.path("exact")).unwrap().len(), exact);
}

#[test]
fn no_overwrite_on_create_or_recovery_and_no_implicit_rearming() {
    let temp = Temp::new();
    let format = format(Encoding::F64Le);
    fs::write(temp.path("existing"), b"keep existing data").unwrap();
    let mut authority = Authority::new(context()).unwrap();
    let consent = authority
        .grant(temp.path("existing"), format.clone(), limits())
        .unwrap();
    assert!(matches!(
        Recorder::create(&authority, consent),
        Err(Error::Io(_))
    ));
    assert_eq!(
        fs::read(temp.path("existing")).unwrap(),
        b"keep existing data"
    );
    let (_, recorder) = create(&temp, "archive", &format, limits());
    drop(recorder);
    assert!(matches!(
        recover(
            &mut File::open(temp.path("archive")).unwrap(),
            &temp.path("existing"),
            limits()
        ),
        Err(Error::Io(_))
    ));
    recover(
        &mut File::open(temp.path("archive")).unwrap(),
        &temp.path("recovered"),
        limits(),
    )
    .unwrap();
    let consent = authority
        .grant(temp.path("recovered"), format, limits())
        .unwrap();
    assert!(matches!(
        Recorder::create(&authority, consent),
        Err(Error::Io(_))
    ));
    assert_eq!(inspect_path(&temp, "recovered").frames, 0);
}

#[test]
fn malformed_metadata_and_insufficient_scan_limits_are_errors() {
    let temp = Temp::new();
    let format = format(Encoding::F32Le);
    let (authority, mut recorder) = create(&temp, "archive", &format, limits());
    recorder
        .append(&authority, &context(), &format, 0, &[1; 24])
        .unwrap();
    recorder.finish().unwrap();
    assert!(matches!(
        inspect(
            &mut File::open(temp.path("archive")).unwrap(),
            Limits {
                max_block_bytes: 12,
                ..limits()
            }
        ),
        Err(Error::Limit)
    ));
    assert!(matches!(
        inspect(
            &mut File::open(temp.path("archive")).unwrap(),
            Limits {
                max_file_bytes: 40,
                ..limits()
            }
        ),
        Err(Error::Limit)
    ));
    let mut raw = fs::read(temp.path("archive")).unwrap();
    raw[16] ^= 1;
    fs::write(temp.path("bad-header"), raw).unwrap();
    assert!(matches!(
        recover(
            &mut File::open(temp.path("bad-header")).unwrap(),
            &temp.path("should-not-exist"),
            limits()
        ),
        Err(Error::Invalid(_))
    ));
    assert!(!temp.path("should-not-exist").exists());
}

#[test]
fn invalid_scope_or_budget_never_creates_a_file() {
    let temp = Temp::new();
    let mut authority = Authority::new(context()).unwrap();
    for mode in 0..6 {
        let mut format = format(Encoding::F64Le);
        let mut admitted = limits();
        let mut path = temp.path("invalid");
        match mode {
            0 => format.channels[1] = format.channels[0].clone(),
            1 => format.sample_rate_hz = 0,
            2 => format.channels.clear(),
            3 => admitted.max_block_bytes = MAX_BLOCK_BYTES + 1,
            4 => admitted.max_blocks = 0,
            5 => path = PathBuf::from("relative-file"),
            _ => unreachable!(),
        }
        assert!(authority.grant(path, format, admitted).is_err());
        assert!(!temp.path("invalid").exists());
    }
}

#[test]
fn invalid_footer_and_trailing_data_do_not_claim_clean_completion() {
    let temp = Temp::new();
    let format = format(Encoding::S32Le);
    let (authority, mut recorder) = create(&temp, "archive", &format, limits());
    recorder
        .append(&authority, &context(), &format, 0, &[9; 12])
        .unwrap();
    recorder.finish().unwrap();
    let mut file = OpenOptions::new()
        .append(true)
        .open(temp.path("archive"))
        .unwrap();
    file.write_all(b"unexpected bytes").unwrap();
    drop(file);
    assert_eq!(inspect_path(&temp, "archive").ending, Ending::Corrupt);
    let report = recover(
        &mut File::open(temp.path("archive")).unwrap(),
        &temp.path("salvaged"),
        limits(),
    )
    .unwrap();
    assert_eq!((report.frames, report.ending), (1, Ending::Corrupt));
    assert_eq!(inspect_path(&temp, "salvaged").ending, Ending::Complete);
}

#[cfg(unix)]
#[test]
fn owned_files_are_private_and_existing_symlinks_are_not_followed() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let temp = Temp::new();
    let format = format(Encoding::S16Le);
    let (_, recorder) = create(&temp, "archive", &format, limits());
    drop(recorder);
    assert_eq!(
        fs::metadata(temp.path("archive"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    symlink(temp.path("archive"), temp.path("link")).unwrap();
    let mut authority = Authority::new(context()).unwrap();
    let consent = authority
        .grant(temp.path("link"), format, limits())
        .unwrap();
    assert!(matches!(
        Recorder::create(&authority, consent),
        Err(Error::Io(_))
    ));
    let mut bytes = Vec::new();
    File::open(temp.path("archive"))
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(
        bytes.len() as u64,
        inspect_path(&temp, "archive").recoverable_bytes
    );
}

#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
#[test]
fn partial_os_write_failure_is_terminal_and_salvageable() {
    // Confine the file-size limit and signal disposition to an owned child.
    // This induces a genuine short file write without filling any filesystem.
    if std::env::var_os("MIXYARD_RECORDING_RLIMIT_CHILD").is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "partial_os_write_failure_is_terminal_and_salvageable",
            ])
            .env("MIXYARD_RECORDING_RLIMIT_CHILD", "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    #[repr(C)]
    struct Rlimit {
        current: u64,
        maximum: u64,
    }
    unsafe extern "C" {
        fn setrlimit(resource: i32, limit: *const Rlimit) -> i32;
        fn signal(signal: i32, handler: usize) -> usize;
    }
    let temp = Temp::new();
    let format = format(Encoding::S16Le);
    let (authority, mut recorder) = create(&temp, "archive", &format, limits());
    recorder
        .append(&authority, &context(), &format, 0, &[1; 6])
        .unwrap();
    let first_len = fs::metadata(temp.path("archive")).unwrap().len();
    let max_len = first_len + 20 + 6 - 1;
    // Linux RLIMIT_FSIZE=1, SIGXFSZ=25 and SIG_IGN=1. The child exits after this test.
    unsafe {
        assert_ne!(signal(25, 1), usize::MAX);
        assert_eq!(
            setrlimit(
                1,
                &Rlimit {
                    current: max_len,
                    maximum: max_len
                }
            ),
            0
        );
    }
    let error = recorder
        .append(&authority, &context(), &format, 1, &[2; 6])
        .unwrap_err();
    assert!(matches!(error, Error::Io(ref os) if os.raw_os_error() == Some(27)));
    assert!(matches!(
        recorder.append(&authority, &context(), &format, 1, &[2; 6]),
        Err(Error::Closed)
    ));
    drop(recorder);
    assert_eq!(fs::metadata(temp.path("archive")).unwrap().len(), max_len);
    let report = recover(
        &mut File::open(temp.path("archive")).unwrap(),
        &temp.path("salvaged"),
        limits(),
    )
    .unwrap();
    assert_eq!(
        (report.frames, report.blocks, report.ending),
        (1, 1, Ending::Interrupted)
    );
    assert_eq!(inspect_path(&temp, "salvaged").ending, Ending::Complete);
}

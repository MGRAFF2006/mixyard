//! Synthetic-only TCP loopback. No devices, microphone, speakers or system routing.
use mixyard_network_codecs::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
fn process_cpu_seconds() -> Option<f64> {
    use std::ffi::c_long;
    #[repr(C)]
    struct Timespec {
        seconds: c_long,
        nanoseconds: c_long,
    }
    unsafe extern "C" {
        fn clock_gettime(clock: i32, time: *mut Timespec) -> i32;
    }
    let mut time = Timespec {
        seconds: 0,
        nanoseconds: 0,
    };
    // SAFETY: valid writable POSIX timespec. Linux CLOCK_PROCESS_CPUTIME_ID=2.
    (unsafe { clock_gettime(2, &mut time) } == 0)
        .then_some(time.seconds as f64 + time.nanoseconds as f64 / 1e9)
}
#[cfg(not(target_os = "linux"))]
fn process_cpu_seconds() -> Option<f64> {
    None
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    println!(
        "linked_runtime_zstd={:?} linked_runtime_opus={:?}",
        zstd_version(),
        opus_version()
    );
    let raw = Format {
        rate: 44117,
        sample: Sample::F64Le,
        channels: vec![
            "FR".into(),
            "FL".into(),
            "FC".into(),
            "LFE".into(),
            "BL".into(),
            "BR".into(),
        ],
        frames: 128,
    };
    run(Plan {
        format: raw.clone(),
        encoding: Encoding::Raw,
    })?;
    #[cfg(feature = "system-zstd")]
    run(Plan {
        format: raw,
        encoding: Encoding::Zstd { level: 3 },
    })?;
    #[cfg(feature = "system-opus")]
    for (rate, channels) in [
        (16000, vec!["M".into()]),
        (48000, vec!["FL".into(), "FR".into()]),
    ] {
        run(Plan {
            format: Format {
                rate,
                sample: Sample::F32Le,
                channels,
                frames: rate / 100,
            },
            encoding: Encoding::Opus(OpusParameters {
                bitrate: 64000,
                complexity: 5,
                vbr: true,
                conversion: Conversion::NormalizedF32Clamp,
            }),
        })?;
    }
    Ok(())
}
fn run(plan: Plan) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let selected = negotiate(
        &plan,
        std::slice::from_ref(&plan),
        std::slice::from_ref(&plan),
        true,
        Budget {
            max_block_bytes: MAX_BLOCK_BYTES,
            max_encoded_bytes: MAX_ENCODED_BYTES,
        },
    )?;
    let binding = Binding {
        connection: [0x55; 16],
        stream: 7,
        generation: 1,
    };
    // Real parent integration passes its whole admitted canonical Spec as T.
    let canonical = (
        "fixture-source",
        vec!["fixture-private-group"],
        vec!["fixture-peer"],
    );
    let mut encoder = Encoder::new(canonical.clone(), selected.clone(), binding)?;
    let mut decoder = Decoder::new(canonical.clone(), selected.clone(), binding)?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let mut tx = TcpStream::connect_timeout(&listener.local_addr()?, Duration::from_secs(1))?;
    let (mut rx, _) = listener.accept()?;
    tx.set_write_timeout(Some(Duration::from_secs(1)))?;
    rx.set_read_timeout(Some(Duration::from_secs(1)))?;
    tx.set_nodelay(true)?;
    let lossy = matches!(plan.encoding, Encoding::Opus(_));
    let mut original = Vec::<f32>::new();
    let mut received = Vec::<f32>::new();
    let cpu_start = process_cpu_seconds();
    let start = Instant::now();
    let mut max_loopback = Duration::ZERO;
    let mut last = Vec::new();
    for sequence in 0..64u64 {
        let f = &plan.format;
        let raw: Vec<u8> = if lossy {
            (0..f.frames)
                .flat_map(|frame| {
                    let t = (sequence * u64::from(f.frames) + u64::from(frame)) as f64
                        / f64::from(f.rate);
                    (0..f.channels.len()).flat_map(move |channel| {
                        (0.25
                            * (t * std::f64::consts::TAU * (440.0 + channel as f64 * 180.0)).sin()
                                as f32)
                            .to_le_bytes()
                    })
                })
                .collect()
        } else {
            [
                0u64,
                0x8000000000000000,
                0x7ff8000000001234,
                0x7ff0000000000001,
                1,
                0x123456789abcdef0,
            ]
            .into_iter()
            .flat_map(u64::to_le_bytes)
            .cycle()
            .take(selected.block_bytes())
            .collect()
        };
        let sent = Instant::now();
        let packet = encoder.encode(&raw)?;
        tx.write_all(&(packet.len() as u32).to_be_bytes())?;
        tx.write_all(&packet)?;
        let mut length = [0; 4];
        rx.read_exact(&mut length)?;
        let length = u32::from_be_bytes(length) as usize;
        if !(HEADER_BYTES..=HEADER_BYTES + selected.budget().max_encoded_bytes).contains(&length) {
            return Err("loopback length budget".into());
        }
        let mut packet = vec![0; length];
        rx.read_exact(&mut packet)?;
        let output = decoder.decode(sequence, &packet)?;
        max_loopback = max_loopback.max(sent.elapsed());
        if lossy {
            original.extend(
                raw.chunks_exact(4)
                    .map(|s| f32::from_le_bytes(s.try_into().unwrap())),
            );
            received.extend(
                output
                    .chunks_exact(4)
                    .map(|s| f32::from_le_bytes(s.try_into().unwrap())),
            );
        } else {
            assert_eq!(output, raw);
        }
        last = packet;
    }
    let elapsed = start.elapsed();
    let cpu_seconds = cpu_start.zip(process_cpu_seconds()).map(|(a, b)| b - a);
    let snr_db = if lossy {
        let delay = encoder.lookahead_frames() as usize * plan.format.channels.len();
        let mut signal = 0f64;
        let mut noise = 0f64;
        for (reference, actual) in original.iter().zip(received.iter().skip(delay)) {
            signal += f64::from(*reference).powi(2);
            noise += (f64::from(*reference) - f64::from(*actual)).powi(2);
        }
        let snr = 10.0 * (signal / noise).log10();
        assert!(
            snr > 10.0,
            "fixture waveform SNR after codec lookahead: {snr}"
        );
        Some(snr)
    } else {
        None
    };
    assert_eq!(decoder.canonical(), &canonical);
    // Reconnect and restart have separate bindings, never infer new admission from media.
    let mut new_binding = binding;
    new_binding.generation += 1;
    assert!(
        Decoder::new((), selected.clone(), new_binding)?
            .decode(63, &last)
            .is_err()
    );
    new_binding.connection[0] ^= 1;
    assert!(
        Decoder::new((), selected.clone(), new_binding)?
            .decode(63, &last)
            .is_err()
    );
    decoder.revocation().revoke();
    encoder.revocation().revoke();
    assert_eq!(decoder.decode(63, &last), Err(Error::Inactive));
    assert_eq!(
        encoder.encode(&vec![0; selected.block_bytes()]),
        Err(Error::Inactive)
    );
    let m = encoder.metrics();
    println!(
        "{:?} rate={} channels={} blocks={} raw_bytes={} envelope_bytes={} ratio={:.4} encode_wall_us={} decode_wall_us={} max_loopback_us={} total_wall_us={} process_cpu_seconds={:?} encoder_context_bytes={} decoder_context_bytes={} lookahead_frames={} fixture_snr_db={:?}",
        plan.encoding,
        plan.format.rate,
        plan.format.channels.len(),
        m.blocks,
        m.raw_bytes,
        m.envelope_bytes,
        m.envelope_bytes as f64 / m.raw_bytes as f64,
        m.elapsed.as_micros(),
        decoder.metrics().elapsed.as_micros(),
        max_loopback.as_micros(),
        elapsed.as_micros(),
        cpu_seconds,
        encoder.context_bytes(),
        decoder.context_bytes(),
        encoder.lookahead_frames(),
        snr_db
    );
    #[cfg(target_os = "linux")]
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status
            .lines()
            .filter(|line| line.starts_with("VmHWM:") || line.starts_with("VmRSS:"))
        {
            println!("process_observation {line}");
        }
    }
    Ok(())
}

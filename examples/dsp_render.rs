//! Synthetic offline throughput demonstration; writes raw interleaved f64le PCM.
//! cargo run --release --example dsp_render -- /tmp/mixyard-dsp-render
use mixyard::dsp::*;
use serde::Serialize;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Timing {
    scenario: &'static str,
    format: Format,
    build: &'static str,
    rustc: String,
    host: String,
    cpu: String,
    os: &'static str,
    arch: &'static str,
    block_frames: usize,
    processed_frames: usize,
    measured_blocks: usize,
    process_time_ns: u128,
    represented_audio_time_ns: u128,
    maximum_block_process_ns: u128,
    maximum_block_budget_ratio: f64,
    observed_deadline_misses: usize,
    timing_status: &'static str,
    admission: Admission,
    output_file: String,
    output_samples: usize,
    output_peak: f64,
    qualification: &'static str,
}
fn effect(name: &str, effect: Effect) -> EffectConfig {
    EffectConfig {
        channels: None,
        name: name.into(),
        enabled: true,
        effect,
    }
}
fn run(
    directory: &Path,
    name: &'static str,
    config: ChainConfig,
    input: &[f64],
    block: usize,
    expected: Option<f64>,
) -> Result<Timing, Box<dyn std::error::Error>> {
    let channels = config.format.channels.len();
    let rate = config.format.sample_rate_hz;
    let limits = Limits {
        max_block_frames: block,
        max_channels: channels,
        max_effects: config.effects.len(),
        max_memory_bytes: 64 * 1024 * 1024,
        max_work_units_per_block: 1_000_000_000,
    };
    let mut chain = Chain::compile(config, limits)?;
    // Warm the executable path, then reset all state before the measured render.
    let warm = vec![0.0; block * channels];
    chain.process(block, &warm)?;
    chain.reset();
    let mut output = Vec::with_capacity(input.len());
    let mut total = Duration::ZERO;
    let mut max = Duration::ZERO;
    let mut misses = 0;
    let mut blocks = 0;
    let mut max_ratio = 0.0f64;
    let mut measured = true;
    for chunk in input.chunks(block * channels) {
        let frames = chunk.len() / channels;
        let start = Instant::now();
        chain.process(frames, std::hint::black_box(chunk))?;
        let elapsed = start.elapsed();
        let budget = Duration::from_secs_f64(frames as f64 / rate as f64);
        if elapsed.is_zero() || budget.is_zero() {
            measured = false;
        }
        total += elapsed;
        max = max.max(elapsed);
        blocks += 1;
        if elapsed > budget {
            misses += 1;
        }
        max_ratio = max_ratio.max(elapsed.as_secs_f64() / budget.as_secs_f64());
        // Collecting output and disk I/O are deliberately outside process timing.
        output.extend_from_slice(chain.output());
    }
    assert_eq!(
        output.len(),
        input.len(),
        "never truncate a slow offline render"
    );
    if let Some(coefficient) = expected {
        // Complete two-channel impulse response, including the entire FIR tail.
        for frame in output.chunks_exact(channels) {
            assert_eq!(frame[0], coefficient);
            assert_eq!(frame[1], coefficient * 0.5);
        }
    }
    let path = directory.join(format!("{name}.f64le"));
    let mut writer = BufWriter::new(File::create(&path)?);
    for sample in &output {
        writer.write_all(&sample.to_le_bytes())?;
    }
    writer.flush()?;
    let cpu = fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines().find_map(|line| {
                line.strip_prefix("model name")
                    .and_then(|s| s.split_once(':'))
                    .map(|(_, s)| s.trim().to_owned())
            })
        })
        .unwrap_or_else(|| "unknown (not measured)".into());
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .or_else(|_| fs::read_to_string("/etc/hostname").map(|s| s.trim().to_owned()))
        .unwrap_or_else(|_| "unknown".into());
    let rustc = std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|| "unknown".into());
    Ok(Timing {
        scenario:name,format:chain.config().format.clone(),build:if cfg!(debug_assertions) {"debug"} else {"release"},rustc,host,cpu,os:std::env::consts::OS,arch:std::env::consts::ARCH,
        block_frames:block,processed_frames:input.len()/channels,measured_blocks:blocks,process_time_ns:total.as_nanos(),represented_audio_time_ns:Duration::from_secs_f64((input.len()/channels) as f64/rate as f64).as_nanos(),maximum_block_process_ns:max.as_nanos(),maximum_block_budget_ratio:max_ratio,observed_deadline_misses:misses,
        timing_status:if !measured || blocks==0 {"unmeasured"} else if misses>0 {"exceeded-observed-live-budget"} else {"within-observed-block-budgets"},admission:chain.admission(),output_file:path.display().to_string(),output_samples:output.len(),output_peak:output.iter().map(|x|x.abs()).fold(0.0,f64::max),qualification:"Synthetic offline host throughput only; no hard realtime, native delivery, total route latency or physical quality qualification.",
    })
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: dsp_render OUTPUT_DIRECTORY")?;
    let directory = Path::new(&path);
    fs::create_dir_all(directory)?;
    let format = Format {
        sample_rate_hz: 48000,
        channels: vec!["left".into(), "right".into()],
    };
    let config = ChainConfig {
        name: "useful-chain".into(),
        format,
        effects: vec![
            effect(
                "gain",
                Effect::Gain {
                    gain: 1.1,
                    invert_polarity: false,
                },
            ),
            effect(
                "presence",
                Effect::Biquad {
                    filter: Filter::Peaking,
                    frequency_hz: 3000.0,
                    q: 0.707,
                    gain_db: 3.0,
                },
            ),
            effect(
                "compressor",
                Effect::Compressor {
                    parameters: Dynamics {
                        threshold_db: -12.0,
                        ratio: 3.0,
                        knee_db: 6.0,
                        attack_ms: 5.0,
                        release_ms: 80.0,
                        makeup_db: 1.0,
                        range_db: 80.0,
                    },
                },
            ),
            effect(
                "echo",
                Effect::Delay {
                    frames: 240,
                    wet: 0.2,
                    dry: 0.8,
                    feedback: 0.3,
                },
            ),
            effect(
                "short-IR",
                Effect::Fir {
                    coefficients: vec![1.0, 0.0, -0.1, 0.05],
                },
            ),
        ],
    };
    let input: Vec<_> = (0..48000)
        .flat_map(|n| {
            let x = 0.25 * (2.0 * std::f64::consts::PI * 1000.0 * n as f64 / 48000.0).sin();
            [x, x * 0.5]
        })
        .collect();
    let fast = run(directory, "useful-chain", config, &input, 128, None)?;
    // Deliberately expensive direct FIR at a negotiated high rate. Retain all
    // coefficients and the full tail even when processing is slower than input.
    let taps = 32768;
    let coefficient = 1.0 / taps as f64;
    let config = ChainConfig {
        name: "long-direct-FIR".into(),
        format: Format {
            sample_rate_hz: 192000,
            channels: vec!["left".into(), "right".into()],
        },
        effects: vec![effect(
            "long-IR",
            Effect::Fir {
                coefficients: vec![coefficient; taps],
            },
        )],
    };
    let mut impulse = vec![0.0; taps * 2];
    impulse[0] = 1.0;
    impulse[1] = 0.5;
    let slow = run(
        directory,
        "long-direct-fir",
        config,
        &impulse,
        64,
        Some(coefficient),
    )?;
    let reports = [fast, slow];
    let json = serde_json::to_string_pretty(&reports)?;
    fs::write(directory.join("timing.json"), &json)?;
    println!("{json}");
    Ok(())
}

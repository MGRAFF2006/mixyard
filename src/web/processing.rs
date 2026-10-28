use super::{error, json, Reply};
use hyper::StatusCode;
use mixyard::dsp::{Admission, BlockReport, Chain, ChainConfig, Limits, Meter};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::time::Instant;

const MAX_FRAMES: usize = 4096;
const MAX_CHANNELS: usize = 32;
const MAX_SAMPLES: usize = 32_768;
const MAX_ITERATIONS: usize = 64;
const MAX_WORK: usize = 250_000_000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Validation {
    config: ChainConfig,
    frames: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preview {
    config: ChainConfig,
    frames: usize,
    iterations: usize,
    bypassed: bool,
    signal: Signal,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum Signal {
    Impulse {
        amplitude: f64,
    },
    Constant {
        amplitude: f64,
    },
    Sine {
        amplitude: f64,
        #[serde(rename = "frequencyHz")]
        frequency_hz: f64,
    },
    Samples {
        samples: Vec<f64>,
    },
}

fn parse<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, Reply> {
    serde_json::from_slice(bytes)
        .map_err(|e| error(StatusCode::BAD_REQUEST, "invalid-json", &e.to_string()))
}

fn invalid(code: &str, message: &str) -> Reply {
    error(StatusCode::UNPROCESSABLE_ENTITY, code, message)
}

fn compile(config: ChainConfig, frames: usize) -> Result<Chain, Reply> {
    if frames == 0
        || frames > MAX_FRAMES
        || config.format.channels.len() > MAX_CHANNELS
        || frames.saturating_mul(config.format.channels.len()) > MAX_SAMPLES
    {
        return Err(invalid("dsp-preview-budget", "Web preview supports 1–4096 frames, up to 32 channels and 32768 scalar samples per block. Use the offline renderer for larger jobs; no stream or effect was truncated."));
    }
    Chain::compile(config, Limits {
        max_block_frames: frames,
        max_channels: MAX_CHANNELS,
        max_effects: 64,
        max_memory_bytes: 32 * 1024 * 1024,
        max_work_units_per_block: MAX_WORK,
    }).map_err(|e| invalid("invalid-dsp-config", &format!("DSP compile rejected: {e}. Web preview has a 32 MiB retained-memory and bounded-work budget; use the offline renderer for larger jobs.")))
}

pub(super) fn validate(bytes: &[u8]) -> Reply {
    let result = || -> Result<Reply, Reply> {
        let body: Validation = parse(bytes)?;
        let chain = compile(body.config, body.frames)?;
        Ok(json(
            &serde_json::json!({"valid":true,"precision":"float64","admission":chain.admission()}),
        ))
    };
    result().unwrap_or_else(|e| e)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Timing {
    measured: bool,
    iterations: usize,
    mean_ms: Option<f64>,
    max_ms: Option<f64>,
    block_budget_ms: f64,
    deadline_misses: usize,
    over_budget: bool,
    build: &'static str,
    os: &'static str,
    arch: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResultBody {
    mode: &'static str,
    precision: &'static str,
    frames: usize,
    sample_rate_hz: u32,
    channels: Vec<String>,
    output: Vec<f64>,
    report: BlockReport,
    meters: Vec<Meter>,
    admission: Admission,
    timing: Timing,
}

pub(super) fn preview(bytes: &[u8]) -> Reply {
    let result = || -> Result<Reply, Reply> {
        let body: Preview = parse(bytes)?;
        if !(1..=MAX_ITERATIONS).contains(&body.iterations) {
            return Err(invalid("dsp-preview-budget", "Choose 1–64 measurement repetitions. Lower repetitions or use the offline renderer for longer jobs."));
        }
        let format = body.config.format.clone();
        let mut chain = compile(body.config, body.frames)?;
        let admission = chain.admission();
        // Include the displayed first render as well as each timed repetition.
        if admission
            .work_units_per_block
            .saturating_mul(body.iterations + 1)
            > MAX_WORK
        {
            return Err(invalid("dsp-preview-budget", "Preview exceeds the total work budget. Lower repetitions/block size or use the full offline renderer; the effect and its quality remain unchanged."));
        }
        let channels = format.channels.len();
        let count = body.frames * channels; // compile admitted checked frame/channel bounds
        let samples = match body.signal {
            Signal::Samples { samples } => {
                if samples.len() != count || samples.iter().any(|x| !x.is_finite()) {
                    return Err(invalid(
                        "invalid-dsp-signal",
                        "Supply exactly frames × channels finite interleaved Float64 samples.",
                    ));
                }
                samples
            }
            signal => {
                let (amplitude, frequency) = match signal {
                    Signal::Impulse { amplitude } | Signal::Constant { amplitude } => {
                        (amplitude, None)
                    }
                    Signal::Sine {
                        amplitude,
                        frequency_hz,
                    } => (amplitude, Some(frequency_hz)),
                    Signal::Samples { .. } => unreachable!(),
                };
                if !amplitude.is_finite()
                    || frequency.is_some_and(|f| {
                        !f.is_finite() || f <= 0.0 || f >= f64::from(format.sample_rate_hz) * 0.5
                    })
                {
                    return Err(invalid("invalid-dsp-signal", "Use finite amplitude and a sine frequency strictly between zero and Nyquist."));
                }
                let mut data = vec![0.0; count];
                for frame in 0..body.frames {
                    let value = match signal {
                        Signal::Impulse { .. } => {
                            if frame == 0 {
                                amplitude
                            } else {
                                0.0
                            }
                        }
                        Signal::Constant { .. } => amplitude,
                        Signal::Sine { .. } => {
                            amplitude
                                * (std::f64::consts::TAU * frequency.unwrap() * frame as f64
                                    / f64::from(format.sample_rate_hz))
                                .sin()
                        }
                        Signal::Samples { .. } => unreachable!(),
                    };
                    data[frame * channels..(frame + 1) * channels].fill(value);
                }
                data
            }
        };
        chain.set_bypassed(body.bypassed);
        let process_error = |e| {
            invalid(
                "dsp-processing-failed",
                &format!("DSP processing rejected: {e}. No partial output or tail is returned."),
            )
        };
        let report = chain
            .process(body.frames, &samples)
            .map_err(process_error)?;
        let output = chain.output().to_vec();
        let meters = chain.meters().to_vec();
        let budget = body.frames as f64 * 1000.0 / f64::from(format.sample_rate_hz);
        let (mut total, mut maximum, mut misses) = (0.0f64, 0.0f64, 0usize);
        for _ in 0..body.iterations {
            chain.reset();
            let start = Instant::now();
            let processed = chain.process(body.frames, &samples);
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            processed.map_err(process_error)?;
            total += elapsed;
            maximum = maximum.max(elapsed);
            misses += usize::from(elapsed > budget);
        }
        let measured = maximum > 0.0;
        Ok(json(&ResultBody {
            mode: "offline-dsp-preview",
            precision: "float64",
            frames: body.frames,
            sample_rate_hz: format.sample_rate_hz,
            channels: format.channels,
            output,
            report,
            meters,
            admission,
            timing: Timing {
                measured,
                iterations: body.iterations,
                mean_ms: measured.then_some(total / body.iterations as f64),
                max_ms: measured.then_some(maximum),
                block_budget_ms: budget,
                deadline_misses: misses,
                over_budget: misses > 0,
                build: if cfg!(debug_assertions) {
                    "debug"
                } else {
                    "release"
                },
                os: std::env::consts::OS,
                arch: std::env::consts::ARCH,
            },
        }))
    };
    result().unwrap_or_else(|e| e)
}

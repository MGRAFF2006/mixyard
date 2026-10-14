#!/usr/bin/env python3
"""Offline tone fixtures and an independent captured-PCM oracle. No audio APIs."""

import argparse
from functools import lru_cache
import json
import math
from pathlib import Path
import random
import struct
import sys
import wave

RATE = 48000
LEVEL = 0.08  # peak, per tone; all fixtures and expected mixes stay below clipping
TONES = {"mic": (600,), "media": (1100, 1700), "private": (2300,), "callreturn": (2900,)}
FREQUENCIES = tuple(f for tones in TONES.values() for f in tones)
# Starter fixture labels only; custom mixes/outputs use external expectations.
CHANNELS = {"call": 1, "record": 2, "monitor": 2}
BASELINE = {"call": {"mic": 1.0},
            "record": {"mic": 0.5, "media": 0.25, "callreturn": 0.25},
            "monitor": {"media": 1.0}}
SCENARIOS = {
    "baseline": BASELINE,
    "call-gain": {**BASELINE, "call": {"mic": 0.25}},
    "record-mic-muted": {**BASELINE, "record": {"media": 0.25, "callreturn": 0.25}},
    "mic-missing": {bus: {s: g for s, g in sends.items() if s != "mic"}
                    for bus, sends in BASELINE.items()},
    "shared-media": {**BASELINE, "call": {"mic": 0.5, "media": 0.5}},
    "silence": {bus: {} for bus in CHANNELS},
}
DEFAULT_LIMITS = {"amplitude_absolute": 0.002, "amplitude_relative": 0.03,
                  "leak_peak": 0.001, "residual_rms": 0.002,
                  "silence_rms": 0.0005, "dc": 0.0005, "peak": 0.98}


def expected(scenario, bus):
    """Contract oracle: mono duplicates; stereo averages into mono (no sqrt(2))."""
    result = [{f: 0.0 for f in FREQUENCIES} for _ in range(CHANNELS[bus])]
    for source, gain in SCENARIOS[scenario][bus].items():
        tones = TONES[source]
        for channel, amplitudes in enumerate(result):
            selected = tones if len(result) == 1 else (tones[channel % len(tones)],)
            for frequency in selected:
                amplitudes[frequency] = LEVEL * gain / len(selected)
    return result


def output_expectations(path, output_id):
    """Independent measured-peak expectations, deliberately not a core config parser."""
    data = json.loads(path.read_text())
    if not isinstance(data, dict) or data.get("schema_version") != 1 or not isinstance(data.get("outputs"), list):
        raise ValueError("expectations require schema_version 1 and an outputs array")
    identifiers = set()
    selected = None
    for output in data["outputs"]:
        if not isinstance(output, dict) or not isinstance(output.get("id"), str) or not output["id"].strip():
            raise ValueError("expected output requires a nonempty stable id")
        if output["id"] in identifiers:
            raise ValueError("duplicate expected output id")
        identifiers.add(output["id"])
        for key in ("mix_id", "device_uid"):
            if not isinstance(output.get(key), str) or not output[key].strip():
                raise ValueError(f"expected output requires {key}")
        peaks = output.get("tone_peaks")
        if not isinstance(peaks, list) or len(peaks) not in (1, 2):
            raise ValueError("tone_peaks must contain one or two channel objects")
        targets = []
        for channel in peaks:
            if not isinstance(channel, dict) or set(channel) - {str(f) for f in FREQUENCIES}:
                raise ValueError("unknown tone frequency in expectation")
            if any(isinstance(v, bool) or not isinstance(v, (int, float)) or not math.isfinite(v) or v < 0
                   for v in channel.values()):
                raise ValueError("expected tone peaks must be finite and nonnegative")
            if sum(channel.values()) >= .98:
                raise ValueError("expected channel has insufficient nonclipping headroom")
            targets.append({f: channel.get(str(f), 0.0) for f in FREQUENCIES})
        if output["id"] == output_id:
            selected = (targets, {k: output[k] for k in ("id", "mix_id", "device_uid")})
    if selected is None:
        raise ValueError("expected output id not found; no truncation or fallback")
    return selected


def write_pcm(path, channels, rate=RATE, width=2):
    """Write normalized channel arrays; used for stimuli and synthetic self-checks."""
    if not channels or len({len(c) for c in channels}) != 1 or width not in (2, 3, 4):
        raise ValueError("invalid PCM channel arrays or sample width")
    scale = 1 << (width * 8 - 1)
    with wave.open(str(path), "wb") as output:
        output.setparams((len(channels), width, rate, 0, "NONE", "not compressed"))
        # Chunk writes keep long tone generation bounded apart from caller arrays.
        for start in range(0, len(channels[0]), rate):
            data = bytearray()
            for frame in zip(*(c[start:start + rate] for c in channels)):
                for sample in frame:
                    if not math.isfinite(sample) or abs(sample) >= 1:
                        raise ValueError("fixture would clip")
                    data.extend(round(sample * (scale - 1)).to_bytes(width, "little", signed=True))
            output.writeframes(data)


def generate(directory, seconds):
    if not math.isfinite(seconds) or not 1 <= seconds <= 60:
        raise ValueError("tone duration must be 1..60 seconds; loop stimuli for endurance")
    directory.mkdir(parents=True, exist_ok=True)
    paths = [directory / f"{name}.wav" for name in TONES]
    if any(path.exists() for path in paths):
        raise ValueError("refusing to overwrite existing source WAVs")
    for (source, frequencies), path in zip(TONES.items(), paths):
        channels = [[LEVEL * math.sin(2 * math.pi * f * i / RATE)
                     for i in range(round(seconds * RATE))] for f in frequencies]
        write_pcm(path, channels)
    return {"status": "generated", "evidence_kind": "synthetic-source-fixtures",
            "rate": RATE, "sample_bits": 16, "seconds": seconds,
            "files": [str(p) for p in paths], "tones_hz": TONES, "peak_per_tone": LEVEL}


def reference_waveform(path, seconds, channels, seed):
    if not math.isfinite(seconds) or not 0.5 <= seconds <= 60:
        raise ValueError("reference duration must be 0.5..60 seconds")
    if path.exists():
        raise ValueError("refusing to overwrite existing reference WAV")
    rng = random.Random(seed)
    samples = [[rng.uniform(-LEVEL, LEVEL) for _ in range(round(seconds * RATE))]
               for _ in range(channels)]
    write_pcm(path, samples)
    return {"status": "generated", "evidence_kind": "synthetic-nonrepeating-source",
            "file": str(path), "seed": seed, "channels": channels,
            "sample_rate": RATE, "sample_bits": 16, "seconds": seconds,
            "native_routing_verified": False}


@lru_cache(maxsize=4)
def basis(rate, count):
    return {f: ([math.cos(2 * math.pi * f * i / rate) for i in range(count)],
                [math.sin(2 * math.pi * f * i / rate) for i in range(count)])
            for f in FREQUENCIES}


def decode(raw, width, channel_count):
    if len(raw) % (width * channel_count):
        raise ValueError("partial PCM frame")
    if width in (2, 4):
        values = struct.unpack("<" + ("h" if width == 2 else "i") * (len(raw) // width), raw)
    else:
        values = [int.from_bytes(raw[i:i + width], "little", signed=True)
                  for i in range(0, len(raw), width)]
    scale = 1 << (width * 8 - 1)
    return [[v / scale for v in values[c::channel_count]] for c in range(channel_count)]


def analyze(path, scenario=None, bus=None, offset=0.0, duration=None, limits=None, targets=None):
    limits = DEFAULT_LIMITS if limits is None else limits
    if not math.isfinite(offset) or offset < 0:
        raise ValueError("offset must be finite and nonnegative")
    if duration is not None and (not math.isfinite(duration) or duration < 0.5):
        raise ValueError("duration must be at least 0.5 seconds")
    failures = []
    failure_count = 0

    def fail(message):
        nonlocal failure_count
        failure_count += 1
        if len(failures) < 20:
            failures.append(message)

    targets = expected(scenario, bus) if targets is None else targets
    ranges = [{str(f): [math.inf, 0.0] for f in FREQUENCIES} for _ in targets]
    max_peak = max_residual = 0.0
    with wave.open(str(path), "rb") as capture:
        rate, width, channels = capture.getframerate(), capture.getsampwidth(), capture.getnchannels()
        if capture.getcomptype() != "NONE" or width not in (2, 3, 4):
            raise ValueError("requires uncompressed signed integer PCM WAV, 16/24/32-bit")
        if channels != len(targets):
            raise ValueError(f"expected {len(targets)} channels, got {channels}; no implicit downmix")
        if not 8000 <= rate <= 192000 or rate % 10:
            raise ValueError("unsupported rate: require 8000..192000 Hz divisible by 10")
        start = round(offset * rate)
        available = capture.getnframes() - start
        wanted = available if duration is None else round(duration * rate)
        if wanted > available or wanted < rate // 2:
            raise ValueError("capture lacks requested interval (minimum 0.5 seconds)")
        capture.setpos(start)
        count = rate // 10  # coherent 100 ms windows, phase independent
        windows, trailing = divmod(wanted, count)
        if trailing:
            raise ValueError("interval must contain whole 100 ms windows; select an explicit --duration")
        trig = basis(rate, count)
        for window in range(windows):
            raw = capture.readframes(count)
            if len(raw) != count * width * channels:
                raise ValueError("truncated WAV data")
            for channel, samples in enumerate(decode(raw, width, channels)):
                label = f"window {window}, channel {channel}"
                dc = sum(samples) / count
                power = sum(x * x for x in samples) / count
                peak = max(abs(x) for x in samples)
                max_peak = max(max_peak, peak)
                if peak >= limits["peak"]:
                    fail(f"{label}: peak {peak:.6f} exceeds ceiling")
                if abs(dc) > limits["dc"]:
                    fail(f"{label}: DC {dc:.6f}")
                amplitudes = {}
                for frequency, (cosines, sines) in trig.items():
                    real = 2 * sum(x * c for x, c in zip(samples, cosines)) / count
                    imag = 2 * sum(x * s for x, s in zip(samples, sines)) / count
                    amplitude = math.hypot(real, imag)
                    amplitudes[frequency] = amplitude
                    observed = ranges[channel][str(frequency)]
                    observed[0], observed[1] = min(observed[0], amplitude), max(observed[1], amplitude)
                    target = targets[channel][frequency]
                    tolerance = (limits["amplitude_absolute"] + limits["amplitude_relative"] * target
                                 if target else limits["leak_peak"])
                    if abs(amplitude - target) > tolerance:
                        fail(f"{label}: {frequency} Hz peak {amplitude:.6f}, expected {target:.6f}")
                residual = math.sqrt(max(0, power - dc * dc - sum(a * a / 2 for a in amplitudes.values())))
                max_residual = max(max_residual, residual)
                if residual > limits["residual_rms"]:
                    fail(f"{label}: unknown content/corruption RMS {residual:.6f}")
                if not any(targets[channel].values()) and math.sqrt(power) > limits["silence_rms"]:
                    fail(f"{label}: silence RMS {math.sqrt(power):.6f}")
    return {"status": "pass" if failure_count == 0 else "fail",
            "evidence_kind": "captured-pcm-analysis-only", "capture": str(path),
            "scenario": scenario, "bus": bus, "sample_rate": rate, "sample_bits": width * 8,
            "channels": channels, "offset_seconds": offset, "windows_100ms": windows,
            "unanalyzed_tail_frames": trailing, "tone_peak_ranges": ranges,
            "max_peak": max_peak, "max_residual_rms": max_residual,
            "tolerances": limits, "failure_count": failure_count, "failures": failures,
            "native_routing_verified": False}


def compare(reference, capture, reference_offset=0, capture_offset=0, frames=None):
    """Aligned unity/bypass PCM check; no automatic gain, shift or resampling."""
    if min(reference_offset, capture_offset) < 0 or (frames is not None and frames <= 0):
        raise ValueError("offsets must be nonnegative and frames positive")
    with wave.open(str(reference), "rb") as ref, wave.open(str(capture), "rb") as got:
        if any(w.getcomptype() != "NONE" or w.getsampwidth() not in (2, 3, 4)
               or w.getnchannels() not in (1, 2) for w in (ref, got)):
            raise ValueError("compare requires mono/stereo integer PCM16/24/32 WAV")
        if (ref.getframerate(), ref.getnchannels()) != (got.getframerate(), got.getnchannels()):
            raise ValueError("reference/capture rates and channels must match; no hidden conversion")
        available = (ref.getnframes() - reference_offset, got.getnframes() - capture_offset)
        if frames is None:
            if available[0] != available[1]:
                raise ValueError("length mismatch; declare offsets and explicit frame interval")
            frames = available[0]
        if frames <= 0 or frames > min(available):
            raise ValueError("missing requested comparison frames")
        ref.setpos(reference_offset)
        got.setpos(capture_offset)
        # One normalized LSB of the coarser format: explicit quantization budget.
        tolerance = 1 / (1 << (8 * min(ref.getsampwidth(), got.getsampwidth()) - 1))
        max_error = squared_error = peak = 0.0
        violations = 0
        remaining = frames
        while remaining:
            count = min(remaining, ref.getframerate())
            arrays = []
            for source in (ref, got):
                raw = source.readframes(count)
                if len(raw) != count * source.getsampwidth() * source.getnchannels():
                    raise ValueError("truncated comparison WAV")
                arrays.append(decode(raw, source.getsampwidth(), source.getnchannels()))
            for left, right in zip(*arrays):
                for x, y in zip(left, right):
                    delta = abs(x - y)
                    max_error = max(max_error, delta)
                    squared_error += delta * delta
                    violations += delta > tolerance
                    peak = max(peak, abs(x), abs(y))
            remaining -= count
        return {"status": "pass" if violations == 0 and peak < .98 else "fail",
                "evidence_kind": "aligned-pcm-transparency-only", "reference": str(reference),
                "capture": str(capture), "frames": frames, "sample_rate": ref.getframerate(),
                "channels": ref.getnchannels(), "reference_bits": ref.getsampwidth() * 8,
                "capture_bits": got.getsampwidth() * 8,
                "reference_offset_frames": reference_offset, "capture_offset_frames": capture_offset,
                "quantization_tolerance": tolerance, "samples_outside_tolerance": violations,
                "max_error": max_error, "rms_error": math.sqrt(squared_error / (frames * ref.getnchannels())),
                "max_peak": peak, "native_routing_verified": False}


def analyze_set(expectations, captures):
    """Check every requested output independently; missing/failed siblings cannot vanish."""
    requested = json.loads(expectations.read_text())
    records = json.loads(captures.read_text())
    if not isinstance(requested, dict) or not isinstance(requested.get("outputs"), list) or not requested["outputs"]:
        raise ValueError("expected nonempty outputs array")
    ids = [o.get("id") if isinstance(o, dict) else None for o in requested["outputs"]]
    # Validate the entire expected output list even when captures are absent.
    output_expectations(expectations, ids[0])
    if not isinstance(records, dict) or set(records) - set(ids):
        raise ValueError("capture map must use only exact requested output ids")
    results = []
    for output in ids:
        try:
            entry = records.get(output)
            if not isinstance(entry, dict) or not isinstance(entry.get("path"), str):
                raise ValueError("missing capture record/path for requested output")
            targets, identity = output_expectations(expectations, output)
            result = analyze(captures.parent / entry["path"], offset=entry.get("offset", 0),
                             duration=entry.get("duration"), targets=targets)
            result["output"] = identity
        except (OSError, ValueError, EOFError, TypeError, wave.Error, struct.error) as error:
            result = {"status": "blocked", "reason": str(error)}
        result["output_id"] = output
        results.append(result)
    statuses = {result["status"] for result in results}
    return {"status": "fail" if "fail" in statuses else "blocked" if "blocked" in statuses else "pass",
            "evidence_kind": "per-output-captured-pcm-analysis-only", "native_routing_verified": False,
            "requested_outputs": ids, "results": results}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    tones = commands.add_parser("tones", help="write four source stimuli, never routed output")
    tones.add_argument("directory", type=Path)
    tones.add_argument("--seconds", type=float, default=3)
    signal = commands.add_parser("reference", help="write seeded nonrepeating source PCM for time-resolved tests")
    signal.add_argument("path", type=Path)
    signal.add_argument("--seconds", type=float, default=3)
    signal.add_argument("--channels", choices=(1, 2), type=int, default=2)
    signal.add_argument("--seed", type=int, default=42)
    check = commands.add_parser("analyze", help="check an externally supplied mix/output recording")
    check.add_argument("capture", type=Path)
    check.add_argument("--scenario", choices=SCENARIOS, help="starter fixture only")
    check.add_argument("--bus", choices=CHANNELS, help="starter fixture only")
    check.add_argument("--expectations", type=Path, help="independent arbitrary mix/output peak expectations")
    check.add_argument("--output", help="exact output id in expectations; one capture per device")
    check.add_argument("--offset", type=float, default=0)
    check.add_argument("--duration", type=float)
    for name, default in DEFAULT_LIMITS.items():
        check.add_argument("--" + name.replace("_", "-"), type=float, default=default)
    all_outputs = commands.add_parser("analyze-set", help="check all output captures without dropping missing/failed devices")
    all_outputs.add_argument("expectations", type=Path)
    all_outputs.add_argument("captures", type=Path, help="JSON output-id -> {path, offset?, duration?}")
    transparent = commands.add_parser("compare", help="compare externally aligned wired unity/bypass PCM")
    transparent.add_argument("reference", type=Path)
    transparent.add_argument("capture", type=Path)
    transparent.add_argument("--reference-offset", type=int, default=0, help="frames")
    transparent.add_argument("--capture-offset", type=int, default=0, help="frames")
    transparent.add_argument("--frames", type=int)
    args = parser.parse_args()
    try:
        if args.command == "tones":
            result = generate(args.directory, args.seconds)
        elif args.command == "reference":
            result = reference_waveform(args.path, args.seconds, args.channels, args.seed)
        elif args.command == "compare":
            result = compare(args.reference, args.capture, args.reference_offset, args.capture_offset, args.frames)
        elif args.command == "analyze-set":
            result = analyze_set(args.expectations, args.captures)
        else:
            limits = {name: getattr(args, name) for name in DEFAULT_LIMITS}
            if any(not math.isfinite(v) or v < 0 for v in limits.values()) or not 0 < limits["peak"] <= 1:
                raise ValueError("tolerances must be finite/nonnegative, peak in (0,1]")
            if args.expectations is not None:
                if args.scenario is not None or args.bus is not None or not args.output:
                    raise ValueError("use --expectations with --output, without starter --scenario/--bus")
                targets, identity = output_expectations(args.expectations, args.output)
                result = analyze(args.capture, offset=args.offset, duration=args.duration, limits=limits, targets=targets)
                result["output"] = identity
                result["expectations"] = str(args.expectations)
            else:
                if args.scenario is None or args.bus is None or args.output is not None:
                    raise ValueError("supply --expectations/--output or starter --scenario/--bus")
                result = analyze(args.capture, args.scenario, args.bus, args.offset, args.duration, limits)
    except (OSError, ValueError, EOFError, wave.Error, struct.error) as error:
        result = {"status": "blocked", "evidence_kind": "no-audio-verdict", "reason": str(error),
                  "native_routing_verified": False}
    print(json.dumps(result, indent=2, allow_nan=False))
    return {"pass": 0, "generated": 0, "fail": 1, "blocked": 2}[result["status"]]


if __name__ == "__main__":
    sys.exit(main())

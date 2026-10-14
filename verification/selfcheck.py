#!/usr/bin/env python3
"""Synthetic analyzer regression checks. Never a native routing test."""

import json
import math
import random
from pathlib import Path
import subprocess
import struct
import sys
import tempfile
import wave

import qualify as q


def require(condition, label):
    if not condition:
        raise AssertionError(label)


def main():
    checks = 0
    with tempfile.TemporaryDirectory(prefix="mixyard-qualification-") as directory:
        root = Path(directory)
        path = root / "synthetic.wav"
        q.generate(root / "sources", 1)
        for source, tones in q.TONES.items():
            with wave.open(str(root / "sources" / f"{source}.wav")) as stimulus:
                require(stimulus.getnchannels() == len(tones), "source channel layout")
                require(stimulus.getnframes() == q.RATE, "source duration")
            checks += 1

        # Deliberately hand-written targets, independent of expected() and scenario data.
        baseline = [({600: .08},),
                    ({600: .04, 1100: .02, 2900: .02}, {600: .04, 1700: .02, 2900: .02}),
                    ({1100: .08}, {1700: .08})]
        cases = {"baseline": baseline,
                 "call-gain": [({600: .02},), baseline[1], baseline[2]],
                 "record-mic-muted": [baseline[0], ({1100: .02, 2900: .02}, {1700: .02, 2900: .02}), baseline[2]],
                 "mic-missing": [({},), ({1100: .02, 2900: .02}, {1700: .02, 2900: .02}), baseline[2]],
                 "shared-media": [({600: .04, 1100: .02, 1700: .02},), baseline[1], baseline[2]],
                 "silence": [({},), ({}, {}), ({}, {})]}

        def samples(targets, rate=q.RATE, phase=.37):
            return [[sum(a * math.sin(2 * math.pi * f * i / rate + phase)
                         for f, a in target.items()) for i in range(rate // 2)] for target in targets]

        for scenario, outputs in cases.items():
            for bus, targets in zip(q.CHANNELS, outputs):
                q.write_pcm(path, samples(targets))
                require(q.analyze(path, scenario, bus)["status"] == "pass", (scenario, bus))
                checks += 1
        for rate, width in ((44100, 3), (96000, 4)):
            q.write_pcm(path, samples(baseline[1], rate), rate, width)
            require(q.analyze(path, "baseline", "record")["status"] == "pass", "PCM format")
            checks += 1

        defects = {
            "private leak": ({600: .08, 2300: .01},),
            "call-return leak": ({600: .08, 2900: .01},),
            "wrong gain": ({600: .04},),
            "unknown tone": ({600: .08, 3700: .02},),
            "missing expected mic": ({},),
        }
        for label, targets in defects.items():
            q.write_pcm(path, samples(targets))
            require(q.analyze(path, "baseline", "call")["status"] == "fail", label)
            checks += 1
        for label, data, scenario, bus in (
            ("channel swap", samples(baseline[1][::-1]), "baseline", "record"),
            ("mute leak", samples(({600: .01},)), "mic-missing", "call"),
            ("DC silence", [[.001] * (q.RATE // 2)], "silence", "call"),
            ("clipping", [[.999 if i % 2 else -.999 for i in range(q.RATE // 2)]], "silence", "call"),
        ):
            q.write_pcm(path, data)
            require(q.analyze(path, scenario, bus)["status"] == "fail", label)
            checks += 1
        for label in ("dropout", "impulse"):
            data = samples(baseline[0])
            if label == "dropout":
                data[0][4800:9600] = [0] * 4800
            else:
                data[0][100] = .9
            q.write_pcm(path, data)
            require(q.analyze(path, "baseline", "call")["status"] == "fail", label)
            checks += 1
        q.write_pcm(path, samples(baseline[1]))
        try:
            q.analyze(path, "baseline", "call")
        except ValueError:
            checks += 1
        else:
            raise AssertionError("stereo Call must be rejected")
        q.write_pcm(path, samples(baseline[0]))
        path.write_bytes(path.read_bytes()[:-100])
        try:
            q.analyze(path, "baseline", "call")
        except ValueError:
            checks += 1
        else:
            raise AssertionError("truncated WAV accepted")
        # Exercise the public CLI failure classifications, not just Python helpers.
        for capture, expected_code in ((root / "missing.wav", 2), (path, 2)):
            result = subprocess.run([sys.executable, str(Path(q.__file__)), "analyze", str(capture),
                                     "--scenario", "baseline", "--bus", "call"], capture_output=True, text=True)
            require(result.returncode == expected_code and json.loads(result.stdout)["status"] == "blocked",
                    "missing/truncated prerequisite must be blocked")
            checks += 1
        reference = root / "reference.wav"
        clean = samples(baseline[0])
        q.write_pcm(reference, clean)
        q.write_pcm(path, clean)
        require(q.compare(reference, path)["status"] == "pass", "transparent PCM")
        checks += 1
        # Independent RIFF bytes: data length excludes its pad byte. Include an
        # odd ancillary chunk so parsers must honor padding when locating data.
        def riff_pcm(width, channels, data_size, rate=q.RATE):
            fmt = struct.pack("<HHIIHH", 1, channels, rate, rate * width * channels,
                              width * channels, width * 8)
            body = (b"WAVEJUNK" + struct.pack("<I", 1) + b"x\0"
                    + b"fmt " + struct.pack("<I", len(fmt)) + fmt
                    + b"data" + struct.pack("<I", data_size) + bytes(data_size)
                    + bytes(data_size % 2))
            return b"RIFF" + struct.pack("<I", len(body)) + body

        def cli_blocked(arguments, label, reason):
            result = subprocess.run([sys.executable, "-B", str(Path(q.__file__)), *arguments],
                                    capture_output=True, text=True, timeout=5)
            report = json.loads(result.stdout)
            require(result.returncode == 2 and report["status"] == "blocked"
                    and report["native_routing_verified"] is False and reason in report["reason"], label)

        valid = root / "padded.wav"
        valid.write_bytes(riff_pcm(3, 1, (q.RATE // 2 + 1) * 3))
        require(q.analyze(valid, "silence", "call", duration=.5)["status"] == "pass", "valid RIFF padding")
        require(q.compare(valid, valid)["status"] == "pass", "valid padded comparison")
        checks += 2
        for width, channels, extra in ((2, 1, 1), (2, 2, 2), (3, 1, 1), (4, 2, 4)):
            malformed = root / "partial.wav"
            malformed.write_bytes(riff_pcm(width, channels, q.RATE // 2 * width * channels + extra))
            cli_blocked(["analyze", str(malformed), "--scenario", "silence",
                         "--bus", "call" if channels == 1 else "record"], "partial frame analyze blocked", "partial declared PCM frame")
            for left, right in ((malformed, reference), (reference, malformed), (malformed, malformed)):
                cli_blocked(["compare", str(left), str(right)], "partial frame compare blocked", "partial declared PCM frame")
            checks += 4
        zero_rate = root / "zero-rate.wav"
        zero_rate.write_bytes(riff_pcm(2, 1, 48000, rate=0))
        cli_blocked(["analyze", str(zero_rate), "--scenario", "silence", "--bus", "call"],
                    "zero rate analyze blocked", "sample rate must be positive")
        for left, right in ((zero_rate, reference), (reference, zero_rate), (zero_rate, zero_rate)):
            cli_blocked(["compare", str(left), str(right)], "zero rate compare blocked without hanging", "sample rate must be positive")
        checks += 4
        data = [clean[0].copy()]
        data[0][100] += .0002  # below routing tolerances, above PCM16 quantization
        q.write_pcm(path, data)
        require(q.compare(reference, path)["status"] == "fail", "subtle PCM corruption")
        checks += 1
        q.write_pcm(path, [[0] * 20 + clean[0]])
        require(q.compare(reference, path, capture_offset=20)["status"] == "pass", "declared frame alignment")
        checks += 1
        try:
            q.compare(reference, path)
        except ValueError:
            checks += 1
        else:
            raise AssertionError("unequal comparison lengths accepted")
        noise_path = root / "nonrepeating.wav"
        q.reference_waveform(noise_path, .5, 2, 42)
        with wave.open(str(noise_path)) as wav:
            noise = q.decode(wav.readframes(wav.getnframes()), 2, 2)
        for label in ("clean", "repeat", "drop", "brief-corruption", "brief-crossmix-leak"):
            changed = [channel.copy() for channel in noise]
            if label == "repeat":
                changed[0][200:210] = changed[0][190:200]
            elif label == "drop":
                changed[0] = changed[0][:200] + changed[0][201:] + [0]
            elif label == "brief-corruption":
                changed[1][200] += .0002
            elif label == "brief-crossmix-leak":
                # One millisecond of an independently seeded private waveform.
                private = random.Random(303)
                for i in range(200, 248):
                    changed[0][i] += private.uniform(-.01, .01)
            q.write_pcm(path, changed)
            want = "pass" if label == "clean" else "fail"
            require(q.compare(noise_path, path)["status"] == want, label)
            checks += 1
        expectations = Path(__file__).with_name("dynamic-audio.json")
        custom = {
            "headphones-a": ({600: .04, 1100: .02}, {600: .04, 1700: .02}),
            "speakers-b": ({600: .02, 1100: .01}, {600: .02, 1700: .01}),
            "virtual-copy": ({}, {}),
            "kitchen-c": ({1100: .04, 2900: .02}, {1700: .04, 2900: .02}),
            "archive-input": ({600: .08},), "quiet-e": ({}, {}),
        }
        for output, target in custom.items():
            targets, identity = q.output_expectations(expectations, output)
            require(identity["id"] == output, "exact custom output identity")
            for channel, wanted in zip(targets, target):
                require(channel == {f: wanted.get(f, 0.0) for f in q.FREQUENCIES}, "independent custom peaks")
            q.write_pcm(path, samples(target))
            require(q.analyze(path, targets=targets)["status"] == "pass", "custom output " + output)
            checks += 1
        capture_map = {}
        for output, target in custom.items():
            destination = root / (output + ".wav")
            q.write_pcm(destination, samples(target))
            capture_map[output] = {"path": destination.name}
        captures_path = root / "captures.json"
        captures_path.write_text(json.dumps(capture_map))
        require(q.analyze_set(expectations, captures_path)["status"] == "pass", "all requested outputs checked")
        checks += 1
        del capture_map["speakers-b"]
        captures_path.write_text(json.dumps(capture_map))
        report = q.analyze_set(expectations, captures_path)
        require(report["status"] == "blocked" and len(report["results"]) == 6
                and sum(r["status"] == "pass" for r in report["results"]) == 5,
                "missing output explicit, other output checks continue")
        checks += 1
        capture_map["speakers-b"] = {"path": "headphones-a.wav"}
        captures_path.write_text(json.dumps(capture_map))
        report = q.analyze_set(expectations, captures_path)
        require(report["status"] == "fail" and sum(r["status"] == "pass" for r in report["results"]) == 5,
                "bad device gain fails independently, siblings checked")
        checks += 1
        targets, _ = q.output_expectations(expectations, "speakers-b")
        q.write_pcm(path, samples(custom["headphones-a"]))
        require(q.analyze(path, targets=targets)["status"] == "fail", "fanout output gain not applied")
        checks += 1
        fixture = json.loads(expectations.read_text())
        # No parser count/name enum: exercise more arbitrary outputs than this fixture needs.
        for i in range(32):
            fixture["outputs"].append({"id": f"extra {i} / 音", "mix_id": f"custom-{i}",
                                       "device_uid": f"qualification:extra:{i}", "tone_peaks": [{}]})
        custom_path = root / "custom-expectations.json"
        custom_path.write_text(json.dumps(fixture))
        require(q.output_expectations(custom_path, "extra 31 / 音")[1]["mix_id"] == "custom-31", "no preset count cap")
        checks += 1
        for label in ("missing-output", "duplicate-output", "unknown-frequency"):
            changed = json.loads(expectations.read_text())
            if label == "duplicate-output":
                changed["outputs"].append(changed["outputs"][0])
            elif label == "unknown-frequency":
                changed["outputs"][0]["tone_peaks"][0]["999"] = .1
            custom_path.write_text(json.dumps(changed))
            try:
                q.output_expectations(custom_path, "absent" if label == "missing-output" else "headphones-a")
            except ValueError:
                checks += 1
            else:
                raise AssertionError(label + " accepted")
    print(json.dumps({"status": "pass", "evidence_kind": "synthetic-analyzer-selfcheck",
                      "checks": checks, "native_routing_verified": False}))


if __name__ == "__main__":
    main()

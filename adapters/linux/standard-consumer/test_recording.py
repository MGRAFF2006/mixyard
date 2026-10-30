"""Pure generated-file oracle checks. No SDK, Boat, native library or audio."""
from pathlib import Path
import struct
import sys
import tempfile
import unittest
from unittest.mock import patch
from boat_recording import collect_diagnostics, pcm_oracle, start_capture


def wav(frames, *, bits=32, rate=48000):
    payload = b"".join(struct.pack("<ff", *frame) for frame in frames)
    fmt = struct.pack("<HHIIHH", 3, 2, rate, rate * 8, 8, bits)
    chunks = b"fmt " + struct.pack("<I", len(fmt)) + fmt + b"data" + struct.pack("<I", len(payload)) + payload
    return b"RIFF" + struct.pack("<I", len(chunks) + 4) + b"WAVE" + chunks


class Recording(unittest.TestCase):
    def test_complete_riff_extent_headers_and_odd_metadata_padding(self):
        frames = [(0.125, 0.015625)] * 4096 + [(0.0625, 0.046875)] * 64 + [(0.0, 0.0)] * 1024
        valid = wav(frames)
        def extent(raw):
            return raw[:4] + struct.pack("<I", len(raw) - 8) + raw[8:]
        malformed = [valid[:4] + struct.pack("<I", 4) + valid[8:],
                     valid[:4] + struct.pack("<I", len(valid)) + valid[8:],
                     extent(valid + b"BAD"),
                     extent(valid + b"JUNK" + struct.pack("<I", 1) + b"x")]
        with tempfile.TemporaryDirectory(prefix="mixyard-riff-", dir="/dev/shm") as temporary:
            path = Path(temporary) / "owned-generated.wav"
            for raw in malformed:
                path.write_bytes(raw)
                with self.assertRaises(RuntimeError):
                    pcm_oracle(path, 1)
            # Odd metadata is valid when its complete pad is inside the RIFF.
            path.write_bytes(extent(valid + b"JUNK" + struct.pack("<I", 1) + b"x\0"))
            self.assertEqual(pcm_oracle(path, 1)["frames"], 5184)

    def test_actual_format_and_sample_rules_reject_narrowing_ambiguity_leakage_and_missing_silence(self):
        frames = [(0.125, 0.015625)] * 4096 + [(0.0625, 0.046875)] * 64 + [(0.0, 0.0)] * 1024
        with tempfile.TemporaryDirectory(prefix="mixyard-recording-oracle-", dir="/dev/shm") as temporary:
            path = Path(temporary) / "owned-generated.wav"
            path.write_bytes(wav(frames))
            result = pcm_oracle(path, 1)
            self.assertEqual(result["frames"], 5184)
            self.assertEqual(result["ordinaryFrames"], 4096)
            self.assertEqual(result["markerFrames"], 64)
            mutations = [wav(frames, bits=64), wav(frames, rate=44100),
                         wav([(0.875, 0.875)] + frames[1:]),
                         wav([(float("nan"), 0.0)] + frames[1:]),
                         wav(frames[:-1] + [(0.125, 0.015625)]), wav(frames)[:-1]]
            for changed in mutations:
                path.write_bytes(changed)
                with self.assertRaises((RuntimeError, KeyError, struct.error)):
                    pcm_oracle(path, 1)


class Diagnostics(unittest.TestCase):
    def test_graph_report_uses_the_existing_larger_bounded_client_role(self):
        capture = start_capture("graph", [sys.executable, "-c",
            "import sys;sys.stdout.write('x'*32768)"], None)
        try:
            capture.process.wait(timeout=3)
        finally:
            capture.finish()
        self.assertEqual(capture.role, "client")
        self.assertIsNone(capture.problem)
        self.assertEqual(len(capture.decoded()[0]), 32768)

    def test_live_stdout_and_stderr_floods_are_bounded_and_owned_child_stops(self):
        for channel, descriptor in (("stdout", 1), ("stderr", 2)):
            with self.subTest(channel=channel), tempfile.TemporaryDirectory(prefix="mixyard-capture-", dir="/dev/shm") as temporary:
                directory = Path(temporary)
                capture = start_capture("owned", [sys.executable, "-c",
                    "import os,time;os.write(" + str(descriptor) + ",b'x'*1048576);time.sleep(30)"],
                    None)
                try:
                    capture.process.wait(timeout=3)
                finally:
                    capture.finish()
                self.assertEqual(capture.problem, channel + "-limit")
                self.assertLessEqual(len(capture.buffers[channel]), capture.limits[channel] + 1)
                self.assertIsNotNone(capture.process.poll())
                self.assertEqual(list(directory.iterdir()), [])
                report = {"result": "failure", "failure": "original experiment failure"}
                with patch.object(Path, "read_bytes", side_effect=AssertionError("unbounded retained-log read")):
                    diagnostics = collect_diagnostics([capture], directory, report)
                self.assertEqual(report["result"], "failure")
                self.assertEqual(report["failure"], "original experiment failure")
                self.assertIn("owned diagnostic capture failed", report["diagnosticFailures"])
                self.assertEqual(diagnostics["owned." + channel]["captureProblem"], channel + "-limit")
                self.assertLessEqual((directory / ("owned." + channel)).stat().st_size,
                                     capture.limits[channel] + 1)

    def test_complete_report_and_single_stderr_byte_remain_distinct(self):
        for warning in (False, True):
            with self.subTest(warning=warning), tempfile.TemporaryDirectory(prefix="mixyard-capture-", dir="/dev/shm") as temporary:
                capture = start_capture("owned", [sys.executable, "-c",
                    "import os;os.write(1,b'{\"stage\":10}\\n');" + ("os.write(2,b'w')" if warning else "")],
                    None)
                try:
                    capture.process.wait(timeout=3)
                finally:
                    capture.finish()
                self.assertEqual(capture.decoded(), ('{"stage":10}\n', "w" if warning else ""))
                report = {"result": "success"}
                collect_diagnostics([capture], Path(temporary), report)
                self.assertEqual(report["result"], "failure" if warning else "success")
                if warning:
                    self.assertEqual(report["diagnosticFailures"], ["unexpected owned diagnostic"])


if __name__ == "__main__":
    unittest.main()

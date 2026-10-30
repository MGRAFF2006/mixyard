"""Pure generated-file oracle checks. No SDK, Boat, native library or audio."""
from pathlib import Path
import struct
import tempfile
import unittest
from boat_recording import pcm_oracle


def wav(frames, *, bits=32, rate=48000):
    payload = b"".join(struct.pack("<ff", *frame) for frame in frames)
    fmt = struct.pack("<HHIIHH", 3, 2, rate, rate * 8, 8, bits)
    chunks = b"fmt " + struct.pack("<I", len(fmt)) + fmt + b"data" + struct.pack("<I", len(payload)) + payload
    return b"RIFF" + struct.pack("<I", len(chunks) + 4) + b"WAVE" + chunks


class Recording(unittest.TestCase):
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


if __name__ == "__main__":
    unittest.main()

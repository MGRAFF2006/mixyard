"""CPU-only diagnostic parser/mapping checks; no PipeWire or native SDK."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


@unittest.skipUnless(sys.platform.startswith("linux"), "own Linux /proc mapping diagnostic")
class SharedMaps(unittest.TestCase):
    def test_bounded_parser_and_own_shared_private_lifetimes(self):
        base = Path(__file__).resolve().parent
        with tempfile.TemporaryDirectory(prefix="mixyard-shared-map-check-", dir="/dev/shm") as root:
            binary = Path(root) / "check"
            subprocess.run(["cc", "-D_GNU_SOURCE", "-std=c11", "-Wall", "-Wextra", "-Werror", "-pedantic",
                            str(base / "shared_map_check.c"), "-o", str(binary)],
                           env=dict(os.environ, TMPDIR=root), check=True, capture_output=True)
            for mode in ("model", "own-maps"):
                with self.subTest(mode=mode):
                    result = subprocess.run([str(binary), mode], check=True, capture_output=True, timeout=5)
                    self.assertEqual(result.stderr, b"")
                    self.assertLess(len(result.stdout), 1024)
                    report = json.loads(result.stdout)
                    self.assertEqual(report.pop("mode"), mode)
                    self.assertTrue(all(value is True for value in report.values()))

            result = subprocess.run([str(binary), "serialization"], check=True, capture_output=True, timeout=5)
            self.assertEqual(result.stderr, b"")
            self.assertLess(len(result.stdout), 20 * 1024)  # Bounded addition to unchanged 64KiB report cap.
            self.assertNotIn(b"PRIVATE_PATH", result.stdout)
            self.assertNotIn(b"address", result.stdout)
            self.assertNotIn(b"inode", result.stdout)
            self.assertNotIn(b"major", result.stdout)
            complete, changed, overflow = [json.loads(line) for line in result.stdout.splitlines()]
            self.assertEqual(complete["scope"], "snapshot-time-candidate-only")
            self.assertEqual(complete["laterCallbackBacking"], "unknown")
            self.assertEqual(complete["crossBufferCandidates"], [{"pcmRegistrationId": 1,
                "metadataRegistrationId": 2, "metadataRegion": 1, "cookie": 1, "fileOffset": 64, "length": 16}])
            self.assertEqual(complete["selectedCoverage"][2]["coordinate"]["kind"], "private-cow-unknown")
            self.assertEqual(complete["selectedCoverage"][3]["coordinate"]["kind"], "anonymous-unknown")
            self.assertFalse(changed["targetAssociationRechecked"])
            self.assertEqual(changed["crossBufferCandidates"], [])
            self.assertTrue(all(t["coordinate"]["cookie"] is None for t in changed["selectedCoverage"]))
            self.assertTrue(overflow["candidateOverflow"])
            self.assertFalse(overflow["sharedFilePairScanComplete"])
            self.assertEqual(overflow["crossBufferCandidates"], [])


if __name__ == "__main__":
    unittest.main()

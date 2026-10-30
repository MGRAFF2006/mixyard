"""Bounded metadata/qualification controls only. No SDK archive/build/native API."""
import copy
import io
import json
import os
from pathlib import Path
import struct
import tarfile
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

import comparison_sdk as sdk


def elf_fixture():
    """Small owned ELF identity fixture, never executable or loaded."""
    header = bytearray(64)
    header[:6] = b"\x7fELF\x02\x01"
    struct.pack_into("<Q", header, 32, 64)
    struct.pack_into("<HH", header, 54, 56, 1)
    note = struct.pack("<III", 4, 4, 3) + b"GNU\0" + b"\x01\x23\x45\x67"
    return header + struct.pack("<IIQQQQQQ", 4, 4, 120, 0, 0, len(note), len(note), 4) + note


def fixture(root):
    work = root / "sdk"
    work.mkdir(mode=0o700)
    prefix = work / "prefix"
    library = prefix / "lib/libpipewire-0.3.so.0.test"
    library.parent.mkdir(parents=True)
    library.write_bytes(elf_fixture())
    (prefix / "lib/libpipewire-0.3.so.0").symlink_to(library.name)
    sdk.write_json(work / "prefix-manifest.json", sdk.manifest(prefix))
    sdk.write_json(work / "source-inputs.json", {"source": sdk.SOURCE, "fixtureOnly": True})
    proof = root / "fixed-sdk-source-provenance.json"
    sdk.write_json(proof, {"fixtureOnly": True})
    descriptor = {"schema": 1, "scope": "hosted-only-fixed-sdk-comparison", "source": sdk.SOURCE,
                  "prefix": str(prefix), "prefixManifestSha256": sdk.digest(work / "prefix-manifest.json"),
                  "sourceInputsSha256": sdk.digest(work / "source-inputs.json"), "projectProvenanceSha256": sdk.digest(proof),
                  "buildOptions": [], "tools": {}}
    sdk.write_json(work / "descriptor.json", descriptor)
    return sdk.Comparison(work / "descriptor.json"), library


class Comparison(unittest.TestCase):
    def test_pins_descriptors_objects_and_incomplete_observations_fail_closed(self):
        with tempfile.TemporaryDirectory(prefix="mixyard-sdk-metadata-check-", dir="/dev/shm") as temporary:
            root = Path(temporary)
            with mock.patch.dict(os.environ, RUNNER_TEMP=str(root)):
                comparison, library = fixture(root)
                self.assertEqual(sdk.build_id(library), "01234567")
                line = f"1000-2000 r-xp 0000 00:01 1 {library}\n".encode()
                objects = comparison.objects_from_maps(line)
                self.assertEqual(set(objects), {"lib/libpipewire-0.3.so.0.test"})
                self.assertEqual(objects[next(iter(objects))]["buildId"], "01234567")
                self.assertEqual(comparison.summary()["shortLivedObjects"]["pw-dump"], "unknown-not-observed")
                with self.assertRaisesRegex(RuntimeError, "incomplete"):
                    comparison.objects_from_maps(line[:-1])
                with self.assertRaisesRegex(RuntimeError, "outside"):
                    comparison.objects_from_maps(b"1000-2000 r-xp 0 00:01 1 /outside/libpipewire-0.3.so.0\n")
                damaged = bytearray(library.read_bytes()); damaged[-1] ^= 1
                library.write_bytes(damaged)
                with self.assertRaisesRegex(RuntimeError, "identity mismatch"):
                    comparison.objects_from_maps(line)
                with self.assertRaisesRegex(RuntimeError, "installed files changed"):
                    sdk.Comparison(comparison.path)
                library.write_bytes(elf_fixture())
                descriptor = json.loads(comparison.path.read_bytes())
                descriptor["source"]["commit"] = "0" * 40
                comparison.path.write_text(json.dumps(descriptor))
                with self.assertRaisesRegex(RuntimeError, "unpinned"):
                    sdk.Comparison(comparison.path)

    def test_identical_eight_readbacks_required_and_mismatches_not_accepted(self):
        names = ["zero-fresh", "page-offset-fresh", "page-offset-cover-zero", "nonzero-origin-fresh",
                 "nonzero-origin-same-page", "nonzero-origin-later-page", "observed-4224-fresh", "observed-4224-cover"]
        shapes = [(0,0,0),(4224,0,0),(4224,0,16384),(12416,0,0),(8320,8192,16384),
                  (12416,8192,16384),(4224,0,0),(4224,0,16384)]
        cases = []
        for name, shape in zip(names, shapes):
            cases.append(dict(name=name, requestedOffset=shape[0], requestedBytes=32, coverOrigin=shape[1], coverBytes=shape[2],
                preadComplete=True, returnedMetadataMatches=True, withinOwnedMapping=True, readbackMatchesPread=True, cleanupComplete=True))
        report = dict(publicMappingReadback=True, headerVersion="1.4.8", runtimeVersion="1.4.8", resolvedApiLibraryBuildId="01234567",
                      pageSize=4096, ownedAllocationBytes=32768, observed4224ShapeIncluded=True, allCasesPassed=True, cases=cases)
        with tempfile.TemporaryDirectory(prefix="mixyard-sdk-api-metadata-", dir="/dev/shm") as temporary:
            root = Path(temporary)
            with mock.patch.dict(os.environ, RUNNER_TEMP=str(root)):
                comparison, _ = fixture(root)
                file = root / "readback.json"
                file.write_text(json.dumps(report)); comparison.verify_api(file)
                mutations = [(["cases", 2, "readbackMatchesPread"], False), (["runtimeVersion"], "1.0.5"),
                             (["resolvedApiLibraryBuildId"], "deadbeef"), (["cases", 3, "requestedOffset"], 128),
                             (["pageSize"], 8192), (["allCasesPassed"], False)]
                for keys, value in mutations:
                    changed = copy.deepcopy(report); target = changed
                    for key in keys[:-1]: target = target[key]
                    target[keys[-1]] = value
                    file.write_text(json.dumps(changed))
                    with self.assertRaises(RuntimeError): comparison.verify_api(file)

    def test_actual_owned_process_without_critical_sdk_binding_stays_unknown(self):
        with tempfile.TemporaryDirectory(prefix="mixyard-sdk-owned-point-", dir="/dev/shm") as temporary:
            root = Path(temporary)
            with mock.patch.dict(os.environ, RUNNER_TEMP=str(root)):
                comparison, _ = fixture(root)
                comparison.graph = Path(sys.executable).resolve()
                process = subprocess.Popen([sys.executable, "-I", "-c", "import time; time.sleep(5)"])
                try:
                    with self.assertRaisesRegex(RuntimeError, "critical SDK binding"):
                        comparison.observe_owned(process, "graph", "owned-python-negative-not-SDK-proof")
                    entry = comparison.summary()["snapshots"][0]
                    self.assertEqual(entry["status"], "unknown")
                    self.assertGreaterEqual(entry["afterMonotonicNs"], entry["beforeMonotonicNs"])
                    self.assertNotIn("objects", entry)
                finally:
                    process.terminate()
                    process.wait(timeout=2)

    def test_archive_hash_checked_before_extraction_and_local_preparation_refused(self):
        with tempfile.TemporaryDirectory(prefix="mixyard-sdk-archive-check-", dir="/dev/shm") as temporary:
            root = Path(temporary); archive = root / "not-the-pinned-archive"
            archive.write_bytes(b"not an archive")
            with mock.patch.object(tarfile, "open", side_effect=AssertionError("must not extract unverified bytes")):
                with self.assertRaisesRegex(RuntimeError, "before extraction"):
                    sdk.extract_verified(archive, root)
            with mock.patch.dict(os.environ, GITHUB_ACTIONS="false"):
                with self.assertRaisesRegex(RuntimeError, "hosted"):
                    sdk.prepare(root / "not-created")
            self.assertFalse((root / "not-created").exists())

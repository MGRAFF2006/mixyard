"""Optional conformance run against an independently built production Rust oracle.

Set MIXYARD_DESCRIPTOR_ORACLE to a local executable that reads one bounded JSON
{sender, receiver, spec} and returns {accepted: bool}. This test never builds Rust.
Session resource/consent policy and wire whitespace budgets are separate checks.
"""

import copy
import json
import os
import subprocess
import unittest

from mixyard_interop import Spec


def corpus():
    base = {"stream": 7, "name": "selected bus", "codec": "raw-f64-le",
            "sample_rate": 44117, "channels": ["FR", "FL", "AUX"],
            "frames_per_packet": 2,
            "provenance": {"sources": [{"peer": "origin", "source": "file",
                                         "groups": ["private"]}],
                           "visited_peers": ["origin", "sender"]}}
    cases = []

    def add(name, accepted, mutate=lambda s: None, sender="sender", receiver="sink"):
        spec = copy.deepcopy(base)
        mutate(spec)
        cases.append((name, {"sender": sender, "receiver": receiver, "spec": spec}, accepted))

    for codec in ["raw-s16-le", "raw-s24-le", "raw-s32-le", "raw-f32-le", "raw-f64-le"]:
        add(codec, True, lambda s, c=codec: s.update(codec=c))
    add("160ch above v1 MTU", True, lambda s: s.update(channels=[f"C{i}" for i in range(160)]))
    for rate in [1, 2**32 - 1]:
        add(f"u32 rate {rate}", True, lambda s, r=rate: s.update(sample_rate=r))
    add("u64 stream max", True, lambda s: s.update(stream=2**64 - 1))
    add("u16 frames max", True, lambda s: s.update(frames_per_packet=65535))
    add("UTF8 identity", True, lambda s: s.update(name="Hörsaal 混音"))
    add("128 byte name", True, lambda s: s.update(name="a" * 128))
    for key in ["stream", "sample_rate", "frames_per_packet"]:
        for value in [True, 1.0, "1", -1, 0]:
            add(f"{key} rejects {value!r}", False, lambda s, k=key, v=value: s.update({k: v}))
    add("rate u32 overflow", False, lambda s: s.update(sample_rate=2**32))
    add("stream u64 overflow", False, lambda s: s.update(stream=2**64))
    add("frames u16 overflow", False, lambda s: s.update(frames_per_packet=65536))
    add("unknown field", False, lambda s: s.update(unknown=True))
    add("missing field", False, lambda s: s.pop("name"))
    add("nonraw recognized opus", False, lambda s: s.update(codec="opus"))
    add("invented codec alias", False, lambda s: s.update(codec="pcm-s16le"))
    add("empty channels", False, lambda s: s.update(channels=[]))
    add("duplicate channels", False, lambda s: s.update(channels=["FL", "FL"]))
    add("control identity", False, lambda s: s.update(name="bad\x85name"))
    add("UTF8 bytes name bound", False, lambda s: s.update(name="é" * 65))
    add("unknown provenance field", False, lambda s: s["provenance"].update(other=True))
    add("unknown source field", False, lambda s: s["provenance"]["sources"][0].update(other=True))
    add("missing origin", False, lambda s: s["provenance"]["sources"][0].update(peer="absent"))
    add("duplicate groups", False, lambda s: s["provenance"]["sources"][0].update(groups=["x", "x"]))
    add("duplicate sources", False, lambda s: s["provenance"]["sources"].append(s["provenance"]["sources"][0]))
    add("empty sources", False, lambda s: s["provenance"].update(sources=[]))
    add("empty path", False, lambda s: s["provenance"].update(visited_peers=[]))
    add("duplicate traversal", False, lambda s: s["provenance"].update(visited_peers=["origin", "sender", "sender"]))
    add("sender mismatch", False, sender="other")
    add("receiver loop", False, receiver="origin")
    add("self route", False, receiver="sender")
    add("empty sender", False, sender="")
    for count, accepted in [(96, True), (110, False)]:
        add(f"canonical many-source byte boundary {count}", accepted,
            lambda s, n=count: s["provenance"].update(
                sources=[{"peer": "a", "source": str(i), "groups": []} for i in range(n)],
                visited_peers=["a"]), sender="a")
    return cases


@unittest.skipUnless(os.environ.get("MIXYARD_DESCRIPTOR_ORACLE"), "production Rust oracle not supplied")
class ProductionDescriptorConformance(unittest.TestCase):
    def test_canonical_semantics_match_independent_rust(self):
        for name, request, expected in corpus():
            with self.subTest(case=name):
                payload = json.dumps(request, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
                self.assertLessEqual(len(payload), 8192)
                process = subprocess.run([os.environ["MIXYARD_DESCRIPTOR_ORACLE"]], input=payload,
                                         capture_output=True, timeout=2, check=True)
                self.assertLessEqual(len(process.stdout), 128)
                rust = json.loads(process.stdout)["accepted"]
                try:
                    spec = Spec.from_dict(request["spec"])
                    spec.validate_path(request["sender"], request["receiver"])
                    python = True
                except (ValueError, PermissionError):
                    python = False
                self.assertEqual(rust, expected)
                self.assertEqual(python, rust)


if __name__ == "__main__":
    unittest.main()

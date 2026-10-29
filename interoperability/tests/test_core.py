from dataclasses import FrozenInstanceError, replace
import unittest
import json
from unittest.mock import patch

from mixyard_interop import Capability, Endpoint, Grant, Limits, Session, Spec
from mixyard_interop.core import Source, Provenance


def descriptor(codec="raw-f64-le", rate=44117, channels=None):
    return {"stream": 7, "name": "selected bus", "codec": codec,
            "sample_rate": rate, "channels": channels or ["FR", "FL", "AUX"],
            "frames_per_packet": 2,
            "provenance": {"sources": [{"peer": "origin", "source": "file",
                                         "groups": ["private"]}],
                           "visited_peers": ["origin", "sender"]}}


class CanonicalDescriptor(unittest.TestCase):
    def test_raw_roundtrip_preserves_precision_irregular_rate_order_and_provenance(self):
        for codec, width in [("raw-s16-le", 2), ("raw-s24-le", 3), ("raw-s32-le", 4),
                             ("raw-f32-le", 4), ("raw-f64-le", 8)]:
            wire = descriptor(codec)
            spec = Spec.from_dict(wire)
            self.assertEqual(spec.to_dict(), wire)
            self.assertEqual(Spec.from_json(spec.to_json()), spec)
            self.assertEqual(spec.frame_bytes, width * 3)
            # Snapshots and constructor inputs cannot mutate active grants.
            wire["channels"].reverse()
            wire["provenance"]["sources"][0]["groups"].clear()
            snapshot = spec.to_dict()
            snapshot["provenance"]["visited_peers"].clear()
            self.assertEqual(spec.channels, ("FR", "FL", "AUX"))
            self.assertEqual(spec.provenance.sources[0].groups, ("private",))
            with self.assertRaises(FrozenInstanceError):
                spec.sample_rate = 48000

    def test_descriptor_does_not_inherit_quic_mtu_or_rate_ceiling(self):
        wire = descriptor(rate=2**32 - 1, channels=[f"C{i}" for i in range(160)])
        spec = Spec.from_dict(wire)
        self.assertGreater(spec.frame_bytes, 1200)
        session = Session("sender", [Grant("live-send", Endpoint("127.0.0.1", 5555), "sink", spec)],
                          consent=True, limits=Limits(max_bytes_per_second=2**63 - 1))
        session.require("live-send", Endpoint("127.0.0.1", 5555), spec)

    def test_many_source_provenance_uses_exact_canonical_byte_budget(self):
        wire = {"stream": 1, "name": "mix", "codec": "raw-s16-le",
                "sample_rate": 48000, "channels": ["FC"], "frames_per_packet": 1,
                "provenance": {"sources": [{"peer": "a", "source": str(i), "groups": []}
                                             for i in range(96)], "visited_peers": ["a"]}}
        raw = json.dumps(wire, ensure_ascii=False, separators=(",", ":")).encode()
        self.assertEqual(len(raw), 3889)
        parsed = Spec.from_json(raw)
        self.assertEqual(parsed, Spec.from_dict(wire))
        direct = Provenance(tuple(Source("a", str(i)) for i in range(96)), ("a",))
        self.assertEqual(parsed.provenance, direct)
        parsed.validate_path("a", "sink")
        wire["provenance"]["sources"] = [{"peer": "a", "source": str(i), "groups": []}
                                              for i in range(110)]
        oversized = json.dumps(wire, ensure_ascii=False, separators=(",", ":")).encode()
        self.assertGreater(len(oversized), 4096)
        with self.assertRaises(ValueError):
            Spec.from_json(oversized)
        with self.assertRaises(ValueError):
            Spec.from_dict(wire)
        with self.assertRaises(ValueError):
            Spec(1, "mix", "raw-s16-le", 48000, ("FC",), 1,
                 Provenance(tuple(Source("a", str(i)) for i in range(110)), ("a",)))

    def test_rejects_unknown_or_ambiguous_wire_and_invalid_paths(self):
        mutations = [lambda w: w.update(extra=True), lambda w: w.update(stream=True),
                     lambda w: w.update(codec="pcm-s16le"), lambda w: w.update(codec="opus"),
                     lambda w: w.update(sample_rate=0), lambda w: w.update(channels=["FL", "FL"]),
                     lambda w: w["provenance"].update(visited_peers=["sender", "sender"]),
                     lambda w: w["provenance"]["sources"][0].update(peer="absent"),
                     lambda w: w["provenance"]["sources"][0].update(groups=["x", "x"]),
                     lambda w: w.update(name="bad\x00name")]
        for mutate in mutations:
            wire = descriptor()
            mutate(wire)
            with self.assertRaises(ValueError):
                Spec.from_dict(wire)
        with self.assertRaises(ValueError):
            Spec.from_json(b'{"stream":1,"stream":2}')
        with self.assertRaises(ValueError):
            Spec.from_json(b" " * 4097)
        spec = Spec.from_dict(descriptor())
        for sender, receiver in [("wrong", "sink"), ("sender", "origin"), ("sender", "sender")]:
            with self.assertRaises(ValueError):
                spec.validate_path(sender, receiver)


class Admission(unittest.TestCase):
    def setUp(self):
        self.endpoint = Endpoint("127.0.0.1", 5001)
        self.spec = Spec.from_dict(descriptor())
        self.grant = Grant("live-send", self.endpoint, "sink", self.spec)

    def test_roles_endpoint_and_exact_spec_are_independent(self):
        session = Session("sender", [self.grant], consent=True)
        session.require("live-send", self.endpoint, self.spec)
        attempts = [("live-receive", self.endpoint, self.spec), ("advertise", self.endpoint, None),
                    ("control", self.endpoint, None), ("media-serve", self.endpoint, self.spec),
                    ("live-send", replace(self.endpoint, port=5002), self.spec),
                    ("live-send", self.endpoint, replace(self.spec, sample_rate=48000)),
                    ("live-send", self.endpoint, replace(self.spec, channels=("FL", "FR", "AUX")))]
        for args in attempts:
            with self.assertRaises(PermissionError):
                session.require(*args)

    def test_control_and_advertisement_never_create_media_authority(self):
        session = Session("sender", [Grant("control", self.endpoint, "sink"),
                                     Grant("advertise", self.endpoint, "sink")], consent=True)
        session.require("control", self.endpoint)
        session.require("advertise", self.endpoint)
        with self.assertRaises(PermissionError):
            session.require("live-send", self.endpoint, self.spec)

    def test_sensitive_premix_exclusion_and_loop_reject_before_any_io(self):
        with self.assertRaises(PermissionError):
            Session("sender", [self.grant], consent=True, excluded_groups=["private"])
        with self.assertRaises(ValueError):
            Session("origin", [Grant("live-receive", self.endpoint, "sender", self.spec)], consent=True)
        received = Session("sink", [Grant("live-receive", self.endpoint, "sender", self.spec)], consent=True)
        received.require("live-receive", self.endpoint, self.spec)

    def test_fresh_consent_expiry_revoke_and_new_session_do_not_restore_old(self):
        with self.assertRaises(PermissionError):
            Session("sender", [self.grant])
        with patch("mixyard_interop.core.time.monotonic", return_value=100):
            session = Session("sender", [self.grant], consent=True, lifetime_seconds=1)
        with patch("mixyard_interop.core.time.monotonic", return_value=101):
            with self.assertRaises(PermissionError):
                session.require("live-send", self.endpoint, self.spec)
        revoked = Session("sender", [self.grant], consent=True)
        revoked.revoke()
        fresh = Session("sender", [self.grant], consent=True)
        fresh.require("live-send", self.endpoint, self.spec)
        with self.assertRaises(PermissionError):
            revoked.require("live-send", self.endpoint, self.spec)

    def test_transport_and_resource_limits_do_not_silently_convert(self):
        for host in ["localhost", "127.1", "127.0.0.2", "::1", "192.168.1.1", "127.0.0.1@evil"]:
            with self.assertRaises(ValueError):
                Endpoint(host, 80)
        for lifetime in [float("nan"), float("inf"), -1, True, 3601]:
            with self.assertRaises(ValueError):
                Session("sender", [], consent=True, lifetime_seconds=lifetime)
        with self.assertRaises(ValueError):
            Session("sender", [self.grant], consent=True, limits=Limits(max_frame_bytes=8))
        with self.assertRaises(ValueError):
            Session("sender", [self.grant], consent=True, limits=Limits(max_bytes_per_second=1))
        capability = Capability("raw-http", "live-send", [self.spec])
        self.assertTrue(capability.supports(self.spec))
        self.assertFalse(capability.supports(replace(self.spec, codec="raw-f32-le")))


if __name__ == "__main__":
    unittest.main()

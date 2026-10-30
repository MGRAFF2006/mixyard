import dataclasses
import unittest

from mixyard_interop import Endpoint, Grant, Session
from mixyard_interop.control import ManagementScope, RuntimeRegistry, Suggestion
from mixyard_interop.runtime import RuntimeLease
from test_runtime import fixture


class ControlBoundaryTests(unittest.TestCase):
    def setUp(self):
        self.registry = RuntimeRegistry("device-B", "authority-B")
        self.scope = ManagementScope("device-B", "authority-B", 1, frozenset({"view", "settings"}))
        self.endpoint = Endpoint("127.0.0.1", 19999, "http")
        self.suggestion = Suggestion("owned-peer", "http-pcm", "untrusted SSDP name", self.endpoint)
        self.registry.add_suggestion(self.suggestion)

    def test_inventory_settings_and_management_never_grant_media(self):
        spec, _, _, lease = fixture()
        snapshot = self.registry.snapshot(self.scope)
        self.assertTrue(snapshot["suggestions"][0]["untrusted"])
        self.assertTrue(all(not r["available"] for c in snapshot["capabilities"] for r in c["roles"]))
        selection = self.registry.select(self.scope, "owned-peer", "live-send", spec)
        new_scope = dataclasses.replace(self.scope, revision=2)
        self.assertTrue(all(not r["available"] for c in self.registry.snapshot(new_scope)["capabilities"]
                            for r in c["roles"]))
        empty = Session("source", [], consent=True)
        with self.assertRaises(PermissionError):
            self.registry.attach(selection, lease, empty)
        # Management control and endpoint control also cannot publish PCM.
        control = Session("source", [Grant("control", self.endpoint, "receiver")], consent=True)
        with self.assertRaises(PermissionError):
            self.registry.attach(selection, lease, control)
        session = Session("source", [Grant("live-send", self.endpoint, "receiver", spec)], consent=True)
        activation = self.registry.attach(selection, lease, session)
        called = []
        self.assertEqual(called, [])
        activation.invoke(lambda a: called.append(a.selection.role))
        self.assertEqual(called, ["live-send"])
        session.revoke()
        with self.assertRaises(PermissionError):
            activation.invoke(lambda _: called.append("late"))
        self.assertEqual(called, ["live-send"])

    def test_device_authority_revision_and_scope_switch_fail_closed(self):
        for field, value in (("device_id", "device-A"), ("authority_id", "restart-B"), ("revision", 2)):
            with self.subTest(field=field), self.assertRaises(PermissionError):
                self.registry.snapshot(dataclasses.replace(self.scope, **{field: value}))
        view = dataclasses.replace(self.scope, permissions=frozenset({"view"}))
        with self.assertRaises(PermissionError):
            self.registry.select(view, "owned-peer", "advertise")
        spec, _, _, lease = fixture()
        selection = self.registry.select(self.scope, "owned-peer", "live-send", spec)
        session = Session("source", [Grant("live-send", self.endpoint, "receiver", spec)], consent=True)
        activation = self.registry.attach(selection, lease, session)
        self.registry.select(dataclasses.replace(self.scope, revision=2), "owned-peer", "advertise")
        with self.assertRaises(PermissionError):
            activation.check()
        with self.assertRaises(PermissionError):
            self.registry.attach(selection, lease, session)

    def test_unintegrated_vendor_and_discovery_roles_remain_unavailable(self):
        self.registry.add_suggestion(Suggestion("vendor", "cast", "speaker", self.endpoint))
        spec, _, _, lease = fixture()
        selection = self.registry.select(self.scope, "vendor", "live-send", spec)
        session = Session("source", [Grant("live-send", self.endpoint, "receiver", spec)], consent=True)
        with self.assertRaises(PermissionError):
            self.registry.attach(selection, lease, session)
        with self.assertRaises(ValueError):
            dataclasses.replace(self.scope, permissions=frozenset({"audioPublish"}))

    def test_rtp_capability_cannot_advertise_unimplemented_float64_format(self):
        endpoint = Endpoint("127.0.0.1", 19998, "udp")
        self.registry.add_suggestion(Suggestion("rtp-peer", "rtp", "owned peer", endpoint))
        spec, _, _, lease = fixture()
        selection = self.registry.select(self.scope, "rtp-peer", "live-send", spec)
        session = Session("source", [Grant("live-send", endpoint, "receiver", spec)], consent=True)
        with self.assertRaises(ValueError):
            self.registry.attach(selection, lease, session)
        snapshot = self.registry.snapshot(dataclasses.replace(self.scope, revision=2))
        self.assertTrue(all(not r["available"] for c in snapshot["capabilities"] for r in c["roles"]))

    def test_owner_callback_revocation_or_config_switch_cannot_start_worker(self):
        for action in ("revoke", "configure"):
            with self.subTest(action=action):
                registry = RuntimeRegistry("device-B", "authority-B")
                registry.add_suggestion(self.suggestion)
                spec, binding, _, _ = fixture()
                holder, workers = {}, []
                def owner_check():
                    if holder.get("armed"):
                        if action == "revoke":
                            holder["activation"].revoke()
                        else:
                            registry.select(dataclasses.replace(self.scope, revision=2),
                                            "owned-peer", "advertise")
                    return binding
                lease = RuntimeLease(spec, observed_id=binding.observed_id, generation=7,
                    revision=3, source_epoch=binding.source_epoch, clock=binding.clock_evidence,
                    owner_check=owner_check, consent=True)
                selected = registry.select(self.scope, "owned-peer", "live-send", spec)
                session = Session("source", [Grant("live-send", self.endpoint, "receiver", spec)], consent=True)
                activation = registry.attach(selected, lease, session)
                holder.update(activation=activation, armed=True)
                with self.assertRaises(PermissionError):
                    activation.invoke(workers.append)
                self.assertEqual(workers, [])

    def test_batch_both_directions_and_invalid_batch_is_atomic(self):
        spec, _, _, _ = fixture()
        with self.assertRaises(ValueError):
            self.registry.configure(self.scope, (("owned-peer", "live-send", spec),
                                                 ("missing", "live-receive", spec)))
        self.assertEqual(self.registry.snapshot(self.scope)["revision"], 1)
        selections = self.registry.configure(self.scope, (("owned-peer", "live-send", spec),
                                                         ("owned-peer", "live-receive", spec)))
        self.assertEqual({s.role for s in selections}, {"live-send", "live-receive"})
        for s in selections:
            self.registry._current_selection(s)


if __name__ == "__main__":
    unittest.main()
